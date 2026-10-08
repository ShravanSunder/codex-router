//! The collaboration API served the way a Host serves it, for other crates' tests.
//!
//! A test publishes a version 3 manifest and serves the real router on the service socket,
//! then reaches it with the real client, or calls one tool and reads its answer as a
//! result-or-error envelope.
use crate::{
    CollaborationApiConfig, CollaborationApiListener, DEFAULT_CONCURRENT_REQUESTS,
    collaboration_api_router, serve_collaboration_api,
};
use collaboration_protocol::{
    ApiSelector, ApiSocketPath, ApiTransport, McpSelector, McpTransport, NonEmptyText,
    RouterExecutableRelation, SERVICE_MANIFEST_VERSION, ServiceManifest,
};
use collaboration_service::{
    CollaborationApplication, ManifestPublication, OwnerOnlySocket, SocketCleanup,
};
use futures_util::StreamExt;
use rmcp::{
    model::{
        CallToolRequest, CallToolRequestParams, ClientJsonRpcMessage, ClientRequest,
        NumberOrString, ServerJsonRpcMessage, ServerResult,
    },
    transport::{
        UnixSocketHttpClient,
        streamable_http_client::{StreamableHttpClient, StreamableHttpPostResponse},
    },
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// What the published manifest says beyond the application's own identity.
#[derive(Clone, Debug, Default)]
pub struct TestManifestOptions {
    pub router_proxy_endpoint: Option<SocketAddr>,
}

/// The API served on `<directory>/control.sock`, with its manifest published beside it.
pub struct ServedCollaborationApi {
    shutdown: CancellationToken,
    task: JoinHandle<io::Result<()>>,
    socket: String,
    directory: PathBuf,
    _socket_cleanup: SocketCleanup,
    _manifest: ManifestPublication,
}

impl ServedCollaborationApi {
    /// `directory` must be an owner-only (0700) directory the test owns.
    pub async fn start(
        directory: &Path,
        application: CollaborationApplication,
    ) -> io::Result<Self> {
        Self::start_with(directory, application, TestManifestOptions::default()).await
    }

    pub async fn start_with(
        directory: &Path,
        application: CollaborationApplication,
        options: TestManifestOptions,
    ) -> io::Result<Self> {
        let socket_path = directory.join(ApiSocketPath::ServiceSocket.file_name());
        let (listener, socket_cleanup) = OwnerOnlySocket::bind(&socket_path)?.into_parts();
        let manifest = ServiceManifest {
            version: SERVICE_MANIFEST_VERSION,
            service_id: application.service_id().clone(),
            service_epoch: application.service_epoch().clone(),
            machine_label: application.machine_label().clone(),
            service_version: NonEmptyText::try_from(env!("CARGO_PKG_VERSION").to_owned())
                .map_err(io::Error::other)?,
            api: ApiSelector {
                transport: ApiTransport::StreamableHttpUnix,
                path: ApiSocketPath::ServiceSocket,
            },
            mcp: McpSelector {
                transport: McpTransport::StreamableHttp,
                url: "http://127.0.0.1:0/mcp".to_owned(),
            },
            native_schema_digest: None,
            router_proxy_endpoint: options.router_proxy_endpoint,
        };
        let shutdown = CancellationToken::new();
        let (_sender, relation) = tokio::sync::watch::channel(RouterExecutableRelation::Match);
        let config = CollaborationApiConfig {
            application,
            service_directory: directory.to_owned(),
            native_definitions: None,
            router_executable_relation: relation,
            concurrent_requests: DEFAULT_CONCURRENT_REQUESTS,
            shutdown: shutdown.clone(),
        };
        let api = collaboration_api_router(&config, CollaborationApiListener::UnixSocket);
        let task = tokio::spawn(serve_collaboration_api(listener, api, shutdown.clone()));
        let manifest = ManifestPublication::publish(directory, &manifest)?;
        Ok(Self {
            shutdown,
            task,
            socket: socket_path
                .to_str()
                .ok_or_else(|| io::Error::other("API socket path must be UTF-8"))?
                .to_owned(),
            directory: directory.to_owned(),
            _socket_cleanup: socket_cleanup,
            _manifest: manifest,
        })
    }

    /// The service directory holding the manifest and the socket.
    #[must_use]
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// The CLIs' client, connected through the published manifest.
    pub async fn client(
        &self,
        client_name: &str,
    ) -> Result<collaboration_client::CollaborationClient, collaboration_client::ClientError> {
        collaboration_client::CollaborationClient::connect(&self.directory, client_name, "1").await
    }

    /// Calls one tool and answers `{"result": <structured result>}`, or `{"error": {"code",
    /// "message", "data"}}` carrying the rejection the Router published.
    pub async fn call(&self, tool: &str, arguments: Value) -> io::Result<Value> {
        let Value::Object(arguments) = arguments else {
            return Err(io::Error::other("tool arguments must be an object"));
        };
        let id = NumberOrString::Number(1);
        let request = ClientJsonRpcMessage::request(
            ClientRequest::CallToolRequest(CallToolRequest::new(
                CallToolRequestParams::new(tool.to_owned()).with_arguments(arguments),
            )),
            id,
        );
        let mut headers = HashMap::new();
        headers.insert(
            http::HeaderName::from_static("mcp-protocol-version"),
            http::HeaderValue::from_static("2025-11-25"),
        );
        let response = UnixSocketHttpClient::new(&self.socket, "http://localhost/mcp")
            .post_message(
                Arc::from("http://localhost/mcp"),
                request,
                None,
                None,
                headers,
            )
            .await
            .map_err(io::Error::other)?;
        let message = match response {
            StreamableHttpPostResponse::Json(message, _) => message,
            StreamableHttpPostResponse::Sse(mut events, _) => loop {
                let event = events
                    .next()
                    .await
                    .ok_or_else(|| io::Error::other("stream ended without an answer"))?
                    .map_err(io::Error::other)?;
                let Some(data) = event.data.filter(|data| !data.is_empty()) else {
                    continue;
                };
                let message: ServerJsonRpcMessage =
                    serde_json::from_str(&data).map_err(io::Error::other)?;
                if matches!(
                    message,
                    ServerJsonRpcMessage::Response(_) | ServerJsonRpcMessage::Error(_)
                ) {
                    break message;
                }
            },
            _ => {
                return Err(io::Error::other(
                    "the API accepted a call without answering",
                ));
            }
        };
        Ok(match message {
            ServerJsonRpcMessage::Response(response) => {
                let ServerResult::CallToolResult(result) = response.result else {
                    return Err(io::Error::other("not a tool result"));
                };
                let text = result
                    .content
                    .iter()
                    .find_map(|content| content.as_text().map(|text| text.text.clone()));
                match (result.is_error == Some(true), result.structured_content) {
                    (true, Some(structured)) => json!({"error": published_error(structured)}),
                    // A refusal before the tool ran, such as undecodable arguments, is text only.
                    (true, None) => {
                        json!({"error": {"code": -32050, "message": text, "data": null}})
                    }
                    (false, structured) => json!({"result": structured.unwrap_or(Value::Null)}),
                }
            }
            ServerJsonRpcMessage::Error(error) => json!({"error": {
                "code": error.error.code.0,
                "message": error.error.message,
                "data": error.error.data,
            }}),
            _ => return Err(io::Error::other("unexpected answer")),
        })
    }

    pub async fn stop(self) -> io::Result<()> {
        self.shutdown.cancel();
        self.task.await.map_err(io::Error::other)?
    }
}

/// The published rejection a tool error carries.
fn published_error(mut failure: Value) -> Value {
    if let Some(fields) = failure.as_object_mut() {
        fields.remove("mcpResult");
    }
    let message = failure.get("message").cloned().unwrap_or(Value::Null);
    if failure.get("kind").and_then(Value::as_str) == Some("rejected")
        && let Some(code) = failure.get("code").and_then(Value::as_i64)
    {
        return json!({"code": code, "message": message, "data": failure.get("data")});
    }
    json!({"code": -32050, "message": message, "data": failure})
}

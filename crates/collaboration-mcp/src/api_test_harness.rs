//! Serves the real collaboration API on real listeners for tests, and calls it the way models
//! do over localhost TCP and the CLIs will over the Unix socket.
use crate::{
    COLLABORATION_API_PATH, CollaborationApiConfig, CollaborationApiListener,
    DEFAULT_CONCURRENT_REQUESTS, collaboration_api_router, serve_collaboration_api,
};
use collaboration_protocol::{
    ApiSelector, ApiSocketPath, ApiTransport, McpSelector, McpTransport, NonEmptyText,
    RouterExecutableRelation, SERVICE_MANIFEST_VERSION, ServiceManifest,
};
use collaboration_service::{CollaborationApplication, ManifestPublication, ServiceIdentity};
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use rmcp::{
    RoleClient, ServiceExt as _,
    model::{CallToolRequestParams, CallToolResult},
    service::RunningService,
    transport::{
        StreamableHttpClientTransport, UnixSocketHttpClient,
        streamable_http_client::StreamableHttpClientTransportConfig,
    },
};
use serde_json::Value;
use std::{
    io,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::{TcpListener, UnixListener},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

pub(crate) const TEST_SERVICE_ID: &str = "00000000-0000-4000-8000-000000000001";
pub(crate) const TEST_SERVICE_EPOCH: &str = "00000000-0000-4000-8000-000000000002";
pub(crate) const TEST_PROTOCOL_VERSION: &str = "2025-11-25";

/// A Router identity with no stores; tests add the stores their operations need.
pub(crate) fn test_identity() -> ServiceIdentity {
    ServiceIdentity::new(TEST_SERVICE_ID, TEST_SERVICE_EPOCH).expect("test service identity")
}

/// The configuration a Host would hand every listener, over `application`.
pub(crate) fn api_config(
    application: CollaborationApplication,
    service_directory: &Path,
) -> CollaborationApiConfig {
    let (_sender, relation) = tokio::sync::watch::channel(RouterExecutableRelation::Match);
    CollaborationApiConfig {
        application,
        service_directory: service_directory.to_owned(),
        native_definitions: None,
        router_executable_relation: relation,
        concurrent_requests: DEFAULT_CONCURRENT_REQUESTS,
        shutdown: CancellationToken::new(),
    }
}

/// Publishes the version 3 manifest a Host writes for `application`, naming the API socket
/// `control.sock` in `directory`.
pub(crate) fn publish_manifest(
    directory: &Path,
    application: &CollaborationApplication,
) -> ManifestPublication {
    let manifest = ServiceManifest {
        version: SERVICE_MANIFEST_VERSION,
        service_id: application.service_id().clone(),
        service_epoch: application.service_epoch().clone(),
        machine_label: application.machine_label().clone(),
        service_version: NonEmptyText::try_from(env!("CARGO_PKG_VERSION").to_owned())
            .expect("service version"),
        api: ApiSelector {
            transport: ApiTransport::StreamableHttpUnix,
            path: ApiSocketPath::ServiceSocket,
        },
        mcp: McpSelector {
            transport: McpTransport::StreamableHttp,
            url: "http://127.0.0.1:0/mcp".to_owned(),
        },
        native_schema_digest: None,
        router_proxy_endpoint: None,
    };
    ManifestPublication::publish(directory, &manifest).expect("publish the version 3 manifest")
}

enum ServedEndpoint {
    LoopbackTcp(SocketAddr),
    UnixSocket(PathBuf),
}

/// One listener serving the API until stopped.
pub(crate) struct ServedApi {
    endpoint: ServedEndpoint,
    shutdown: CancellationToken,
    active_calls: Arc<AtomicUsize>,
    task: JoinHandle<io::Result<()>>,
    publication: Option<ManifestPublication>,
}

impl ServedApi {
    pub(crate) async fn tcp(config: &CollaborationApiConfig) -> Self {
        Self::tcp_at(config, "127.0.0.1:0").await
    }

    pub(crate) async fn tcp_at(config: &CollaborationApiConfig, address: &str) -> Self {
        let listener = TcpListener::bind(address).await.expect("bind loopback TCP");
        let address = listener.local_addr().expect("loopback address");
        let api = collaboration_api_router(config, CollaborationApiListener::LoopbackTcp(address));
        let active_calls = api.active_calls();
        let task = tokio::spawn(serve_collaboration_api(
            listener,
            api,
            config.shutdown.clone(),
        ));
        Self {
            endpoint: ServedEndpoint::LoopbackTcp(address),
            shutdown: config.shutdown.clone(),
            active_calls,
            task,
            publication: None,
        }
    }

    pub(crate) async fn unix(config: &CollaborationApiConfig, socket: &Path) -> Self {
        let listener = UnixListener::bind(socket).expect("bind Unix socket");
        let api = collaboration_api_router(config, CollaborationApiListener::UnixSocket);
        let active_calls = api.active_calls();
        let task = tokio::spawn(serve_collaboration_api(
            listener,
            api,
            config.shutdown.clone(),
        ));
        Self {
            endpoint: ServedEndpoint::UnixSocket(socket.to_owned()),
            shutdown: config.shutdown.clone(),
            active_calls,
            task,
            publication: None,
        }
    }

    /// Serves the API on the service directory's `control.sock` and publishes the manifest
    /// beside it, the way the CLIs find a Host.
    pub(crate) async fn service_socket(config: &CollaborationApiConfig) -> Self {
        let directory = &config.service_directory;
        let mut served = Self::unix(
            config,
            &directory.join(ApiSocketPath::ServiceSocket.file_name()),
        )
        .await;
        served.publication = Some(publish_manifest(directory, &config.application));
        served
    }

    /// The URL a client names; a Unix-socket client names localhost.
    pub(crate) fn url(&self) -> String {
        match &self.endpoint {
            ServedEndpoint::LoopbackTcp(address) => {
                format!("http://{address}{COLLABORATION_API_PATH}")
            }
            ServedEndpoint::UnixSocket(_) => format!("http://localhost{COLLABORATION_API_PATH}"),
        }
    }

    /// An rmcp client over this listener's transport.
    pub(crate) async fn client(&self) -> RunningService<RoleClient, ()> {
        match &self.endpoint {
            ServedEndpoint::LoopbackTcp(_) => {
                ().serve(StreamableHttpClientTransport::from_uri(self.url()))
                    .await
                    .expect("rmcp client over TCP")
            }
            ServedEndpoint::UnixSocket(socket) => ()
                .serve(StreamableHttpClientTransport::with_client(
                    UnixSocketHttpClient::new(&socket.to_string_lossy(), &self.url()),
                    StreamableHttpClientTransportConfig::with_uri(self.url()),
                ))
                .await
                .expect("rmcp client over the Unix socket"),
        }
    }

    /// Calls one tool through a fresh rmcp client.
    pub(crate) async fn call_tool(&self, name: &str, arguments: Value) -> CallToolResult {
        let client = self.client().await;
        let arguments = arguments
            .as_object()
            .expect("tool arguments are an object")
            .clone();
        let result = client
            .call_tool(CallToolRequestParams::new(name.to_owned()).with_arguments(arguments))
            .await
            .unwrap_or_else(|error| panic!("{name} call: {error}"));
        client.cancel().await.expect("close rmcp client");
        result
    }

    /// Posts one JSON-RPC message over TCP without a client handshake; stateless calls need
    /// none. Answers the JSON-RPC response, whether sent as JSON or as one SSE event.
    pub(crate) async fn post(&self, message: &Value) -> (reqwest::StatusCode, Value) {
        let response = reqwest::Client::new()
            .post(self.url())
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json, text/event-stream")
            .header("mcp-protocol-version", TEST_PROTOCOL_VERSION)
            .json(message)
            .send()
            .await
            .expect("POST to the collaboration API");
        let status = response.status();
        let body = response.text().await.expect("response body");
        let encoded = body
            .lines()
            .filter_map(|line| line.strip_prefix("data: "))
            .find(|data| !data.is_empty())
            .unwrap_or(body.as_str());
        (
            status,
            serde_json::from_str(encoded).unwrap_or(Value::String(body.clone())),
        )
    }

    /// Sends a tool call and then drops the connection before its answer, as a client that
    /// goes away does.
    pub(crate) async fn abandon_tool_call(&self, name: &str, arguments: Value) {
        let body = serde_json::to_vec(&serde_json::json!({
            "jsonrpc":"2.0","id":1,"method":"tools/call",
            "params":{"name":name,"arguments":arguments}
        }))
        .expect("tool call JSON");
        let head = format!(
            "POST {COLLABORATION_API_PATH} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nMCP-Protocol-Version: {TEST_PROTOCOL_VERSION}\r\nContent-Length: {}\r\n\r\n",
            body.len()
        );
        match &self.endpoint {
            ServedEndpoint::LoopbackTcp(address) => {
                let mut stream = tokio::net::TcpStream::connect(address)
                    .await
                    .expect("connect TCP");
                send_and_abandon(&mut stream, head.as_bytes(), &body, &self.active_calls).await;
            }
            ServedEndpoint::UnixSocket(socket) => {
                let mut stream = tokio::net::UnixStream::connect(socket)
                    .await
                    .expect("connect Unix socket");
                send_and_abandon(&mut stream, head.as_bytes(), &body, &self.active_calls).await;
            }
        }
    }

    pub(crate) fn active_calls(&self) -> usize {
        self.active_calls.load(Ordering::SeqCst)
    }

    /// Waits until the number of handlers in flight is `expected`, or fails at `deadline`.
    pub(crate) async fn await_active_calls(&self, expected: usize, deadline: Duration) {
        tokio::time::timeout(deadline, async {
            while self.active_calls() != expected {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| {
            panic!(
                "expected {expected} calls in flight, found {}",
                self.active_calls()
            )
        });
    }

    pub(crate) async fn stop(self) {
        self.shutdown.cancel();
        self.task
            .await
            .expect("API task join")
            .expect("API stops and its calls settle");
        drop(self.publication);
    }
}

async fn send_and_abandon<TStream>(
    stream: &mut TStream,
    head: &[u8],
    body: &[u8],
    active_calls: &AtomicUsize,
) where
    TStream: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    stream.write_all(head).await.expect("write request head");
    stream.write_all(body).await.expect("write request body");
    stream.flush().await.expect("flush request");
    tokio::time::timeout(Duration::from_secs(5), async {
        while active_calls.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the abandoned call starts");
    let mut early = [0_u8; 1];
    let answered = tokio::time::timeout(Duration::from_millis(200), stream.read(&mut early)).await;
    assert!(
        answered.is_err(),
        "the call answered before it was abandoned"
    );
}

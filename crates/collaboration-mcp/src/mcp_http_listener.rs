use crate::mcp_server::CollaborationMcpServer;
use bytes::Bytes;
use http::Request;
use http_body_util::{BodyExt, Empty};
use hyper::{body::Incoming, service::service_fn};
use hyper_util::rt::{TokioExecutor, TokioIo};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use std::{io, net::SocketAddr, path::PathBuf, str::FromStr};
use tokio::{net::TcpListener, task::JoinSet};
use tokio_util::sync::CancellationToken;

const MCP_ENDPOINT_PATH: &str = "/mcp";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoopbackBindAddress(SocketAddr);

#[derive(Debug, thiserror::Error, Eq, PartialEq)]
pub enum LoopbackBindAddressError {
    #[error("MCP listener address must be loopback")]
    NonLoopback,
    #[error("invalid MCP listener address")]
    InvalidAddress,
}

impl LoopbackBindAddress {
    pub fn new(address: SocketAddr) -> Result<Self, LoopbackBindAddressError> {
        if !address.ip().is_loopback() {
            return Err(LoopbackBindAddressError::NonLoopback);
        }
        Ok(Self(address))
    }
    pub fn parse(value: &str) -> Result<Self, LoopbackBindAddressError> {
        let address =
            SocketAddr::from_str(value).map_err(|_| LoopbackBindAddressError::InvalidAddress)?;
        Self::new(address)
    }

    #[must_use]
    pub const fn socket_addr(&self) -> SocketAddr {
        self.0
    }
}

#[derive(Clone, Debug)]
pub struct CollaborationMcpListenerConfig {
    pub bind_address: LoopbackBindAddress,
    pub service_directory: PathBuf,
    pub allowed_origins: Vec<String>,
}

pub struct CollaborationMcpListener {
    local_address: SocketAddr,
    shutdown: CancellationToken,
    accept_task: Option<tokio::task::JoinHandle<io::Result<()>>>,
}

impl CollaborationMcpListener {
    pub async fn start(config: CollaborationMcpListenerConfig) -> io::Result<Self> {
        let listener = TcpListener::bind(config.bind_address.socket_addr()).await?;
        let local_address = listener.local_addr()?;
        let shutdown = CancellationToken::new();
        let mut allowed_origins = config.allowed_origins;
        allowed_origins.extend([
            format!("http://{local_address}"),
            format!("http://localhost:{}", local_address.port()),
        ]);
        let service_config = StreamableHttpServerConfig::default()
            .with_allowed_hosts([local_address.ip().to_string(), "localhost".to_owned()])
            .with_allowed_origins(allowed_origins)
            .enforce_origin_validation()
            .with_cancellation_token(shutdown.clone());
        let service: StreamableHttpService<CollaborationMcpServer, LocalSessionManager> =
            StreamableHttpService::new(
                move || {
                    Ok(CollaborationMcpServer::new(
                        config.service_directory.clone(),
                    ))
                },
                Default::default(),
                service_config,
            );
        let accept_shutdown = shutdown.clone();
        let accept_task = tokio::spawn(async move {
            let mut connection_tasks = JoinSet::new();
            loop {
                tokio::select! {
                    biased;
                    _ = accept_shutdown.cancelled() => break,
                    accepted = listener.accept() => match accepted {
                        Ok((stream, _)) => {
                            let request_service = service.clone();
                            let connection_shutdown = accept_shutdown.clone();
                            connection_tasks.spawn(async move {
                                let service = service_fn(move |request: Request<Incoming>| {
                                    let mut request_service = request_service.clone();
                                    async move {
                                        if request.uri().path() != MCP_ENDPOINT_PATH {
                                            let mut response = http::Response::new(
                                                Empty::<Bytes>::new().boxed(),
                                            );
                                            *response.status_mut() = http::StatusCode::NOT_FOUND;
                                            return Ok(response);
                                        }
                                        tower_service::Service::call(&mut request_service, request)
                                            .await
                                    }
                                });
                                let connection_builder =
                                    hyper_util::server::conn::auto::Builder::new(
                                        TokioExecutor::new(),
                                    );
                                let connection = connection_builder
                                    .serve_connection_with_upgrades(TokioIo::new(stream), service);
                                tokio::select! {
                                    _ = connection_shutdown.cancelled() => {},
                                    _ = connection => {},
                                }
                            });
                        }
                        Err(error) => return Err(error),
                    },
                    Some(_) = connection_tasks.join_next(), if !connection_tasks.is_empty() => {},
                }
            }
            connection_tasks.abort_all();
            while connection_tasks.join_next().await.is_some() {}
            Ok(())
        });
        Ok(Self {
            local_address,
            shutdown,
            accept_task: Some(accept_task),
        })
    }

    #[must_use]
    pub const fn local_address(&self) -> SocketAddr {
        self.local_address
    }

    #[must_use]
    pub fn local_url(&self) -> String {
        format!("http://{}{}", self.local_address, MCP_ENDPOINT_PATH)
    }

    pub async fn shutdown(mut self) -> io::Result<()> {
        self.shutdown.cancel();
        match self.accept_task.take() {
            Some(task) => task
                .await
                .map_err(|error| io::Error::other(error.to_string()))?,
            None => Ok(()),
        }
    }

    pub async fn listener_failure(&mut self) -> io::Error {
        let Some(task) = &mut self.accept_task else {
            return io::Error::other("MCP listener task missing");
        };
        let result = task.await;
        self.accept_task.take();
        match result {
            Ok(Err(error)) => error,
            Ok(Ok(())) => io::Error::other("collaboration MCP listener stopped unexpectedly"),
            Err(error) => io::Error::other(error.to_string()),
        }
    }
}

impl Drop for CollaborationMcpListener {
    fn drop(&mut self) {
        self.shutdown.cancel();
        if let Some(task) = self.accept_task.take() {
            task.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CollaborationMcpListener, CollaborationMcpListenerConfig, LoopbackBindAddress};
    use reqwest::header::{ACCEPT, CONTENT_TYPE, HeaderMap, HeaderValue, ORIGIN};
    use serde_json::{Value, json};
    use tokio_util::sync::CancellationToken;

    #[test]
    fn bind_address_rejects_wildcard_and_non_loopback_interfaces() {
        assert!(LoopbackBindAddress::parse("127.0.0.1:0").is_ok());
        assert!(LoopbackBindAddress::parse("[::1]:0").is_ok());
        assert!(LoopbackBindAddress::parse("0.0.0.0:0").is_err());
        assert!(LoopbackBindAddress::parse("192.0.2.10:8080").is_err());
    }

    #[tokio::test]
    async fn dropping_owned_listener_releases_loopback_port() {
        let temporary = tempfile::tempdir().expect("temporary service directory");
        let listener = CollaborationMcpListener::start(CollaborationMcpListenerConfig {
            bind_address: LoopbackBindAddress::parse("127.0.0.1:0").expect("loopback bind"),
            service_directory: temporary.path().to_owned(),
            allowed_origins: Vec::new(),
        })
        .await
        .expect("MCP listener starts");
        let address = listener.local_address();
        drop(listener);
        tokio::task::yield_now().await;
        let rebound = tokio::net::TcpListener::bind(address)
            .await
            .expect("owned listener port released on drop");
        drop(rebound);
    }

    #[tokio::test]
    async fn reported_listener_failure_can_shutdown_without_repolling_completed_task() {
        let temporary = tempfile::tempdir().expect("temporary service directory");
        let mut listener = CollaborationMcpListener::start(CollaborationMcpListenerConfig {
            bind_address: LoopbackBindAddress::parse("127.0.0.1:0").expect("loopback bind"),
            service_directory: temporary.path().to_owned(),
            allowed_origins: Vec::new(),
        })
        .await
        .expect("MCP listener starts");
        let address = listener.local_address();
        listener.accept_task.as_ref().expect("accept task").abort();

        let failure = listener.listener_failure().await;
        assert!(failure.to_string().contains("cancelled"));
        listener
            .shutdown()
            .await
            .expect("shutdown treats the reported task as consumed");
        let rebound = tokio::net::TcpListener::bind(address)
            .await
            .expect("listener failure shutdown releases loopback port");
        drop(rebound);
    }

    #[tokio::test]
    async fn real_http_initialization_discovers_typed_tools_without_authentication() {
        let temporary = tempfile::tempdir().expect("temporary service directory");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(temporary.path(), std::fs::Permissions::from_mode(0o700))
                .expect("private service directory");
        }
        let digest = format!("sha256:{}", "a".repeat(64));
        let identity = collaboration_service::ServiceIdentity::new(
            "00000000-0000-4000-8000-000000000001",
            "00000000-0000-4000-8000-000000000002",
            &digest,
        )
        .expect("service identity");
        let control = collaboration_service::LocalControlService::bind(
            &temporary.path().join("control.sock"),
            identity,
        )
        .expect("control listener");
        let manifest = serde_json::from_value(serde_json::json!({
            "version": 2,
            "serviceId": "00000000-0000-4000-8000-000000000001",
            "serviceEpoch": "00000000-0000-4000-8000-000000000002",
            "control": {"transport": "unixJsonLines", "path": "control.sock"},
            "controlSchemaDigest": digest,
            "mcp":{"transport":"streamableHttp","url":"http://127.0.0.1:0/mcp"}
        }))
        .expect("service manifest");
        let _publication =
            collaboration_service::ManifestPublication::publish(temporary.path(), &manifest)
                .expect("manifest publication");
        let control_stop = CancellationToken::new();
        let control_task = tokio::spawn(control.run(control_stop.clone()));
        let listener = CollaborationMcpListener::start(CollaborationMcpListenerConfig {
            bind_address: LoopbackBindAddress::parse("127.0.0.1:0").expect("loopback bind"),
            service_directory: temporary.path().to_owned(),
            allowed_origins: vec!["http://localhost".to_owned()],
        })
        .await
        .expect("MCP listener starts");
        let client = reqwest::Client::new();
        let initialize = client
            .post(listener.local_url())
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json, text/event-stream")
            .json(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-11-25",
                    "capabilities": {},
                    "clientInfo": {"name": "collaboration-mcp-test", "version": "1"}
                }
            }))
            .send()
            .await
            .expect("initialize response");
        assert!(initialize.status().is_success());
        let session_id = initialize
            .headers()
            .get("mcp-session-id")
            .cloned()
            .expect("transport session id");
        let initialize_body = protocol_response_json(initialize).await;
        assert_eq!(
            initialize_body.pointer("/result/serverInfo/name"),
            Some(&json!("codex-router-collaboration"))
        );

        let initialized = client
            .post(listener.local_url())
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json, text/event-stream")
            .header("mcp-session-id", session_id.clone())
            .header("mcp-protocol-version", "2025-11-25")
            .json(&json!({
                "jsonrpc":"2.0",
                "method":"notifications/initialized",
                "params":{}
            }))
            .send()
            .await
            .expect("initialized notification response");
        assert!(initialized.status().is_success());

        let tools = client
            .post(listener.local_url())
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json, text/event-stream")
            .header("mcp-session-id", session_id.clone())
            .header("mcp-protocol-version", "2025-11-25")
            .json(&json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}))
            .send()
            .await
            .expect("tools response");
        assert!(tools.status().is_success());
        let tools_body = protocol_response_json(tools).await;
        let tool_names = tools_body
            .pointer("/result/tools")
            .and_then(Value::as_array)
            .expect("tool array")
            .iter()
            .filter_map(|tool| tool.get("name").and_then(Value::as_str))
            .collect::<Vec<_>>();
        assert!(tool_names.contains(&"endpoints_list"));
        assert_eq!(tool_names.len(), 90);
        let tools = tools_body
            .pointer("/result/tools")
            .and_then(Value::as_array)
            .expect("tool array");
        for (name, phrase) in [
            ("wake_send", "native input acceptance"),
            ("schedule_prepare", "Preparation mutates"),
            ("board_message_post", "Saving the message"),
            (
                "conversation_create_and_prompt",
                "same call-local ACP connection",
            ),
        ] {
            let description = tools
                .iter()
                .find(|tool| tool.get("name").and_then(Value::as_str) == Some(name))
                .and_then(|tool| tool.get("description"))
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("missing live description for {name}"));
            assert!(
                description.contains(phrase),
                "{name} description missing {phrase:?}: {description}"
            );
        }

        let endpoint_call = client
            .post(listener.local_url())
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json, text/event-stream")
            .header("mcp-session-id", session_id)
            .header("mcp-protocol-version", "2025-11-25")
            .json(&json!({
                "jsonrpc":"2.0",
                "id":3,
                "method":"tools/call",
                "params":{"name":"endpoints_list","arguments":{}}
            }))
            .send()
            .await
            .expect("endpoint tool response");
        assert!(endpoint_call.status().is_success());
        let endpoint_body = protocol_response_json(endpoint_call).await;
        assert_eq!(
            endpoint_body.pointer("/result/isError"),
            Some(&json!(false))
        );
        assert_eq!(
            endpoint_body.pointer("/result/structuredContent/serviceEpoch"),
            Some(&json!("00000000-0000-4000-8000-000000000002"))
        );
        listener.shutdown().await.expect("listener shutdown");
        control_stop.cancel();
        control_task
            .await
            .expect("control join")
            .expect("control shutdown");
    }

    async fn protocol_response_json(response: reqwest::Response) -> Value {
        let status = response.status();
        let body = response.text().await.expect("protocol response body");
        assert!(
            !body.is_empty(),
            "empty protocol response with status {status}"
        );
        let encoded = body
            .lines()
            .filter_map(|line| line.strip_prefix("data: "))
            .find(|data| !data.is_empty())
            .unwrap_or(body.as_str());
        serde_json::from_str(encoded)
            .unwrap_or_else(|error| panic!("protocol response JSON: {error}; body={body:?}"))
    }

    #[tokio::test]
    async fn invalid_origin_is_rejected_before_protocol_dispatch() {
        let temporary = tempfile::tempdir().expect("temporary service directory");
        let listener = CollaborationMcpListener::start(CollaborationMcpListenerConfig {
            bind_address: LoopbackBindAddress::parse("127.0.0.1:0").expect("loopback bind"),
            service_directory: temporary.path().to_owned(),
            allowed_origins: vec!["http://localhost".to_owned()],
        })
        .await
        .expect("MCP listener starts");
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/json, text/event-stream"),
        );
        headers.insert(ORIGIN, HeaderValue::from_static("https://invalid.example"));
        let response = reqwest::Client::new()
            .post(listener.local_url())
            .headers(headers)
            .json(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-11-25",
                    "capabilities": {},
                    "clientInfo": {"name": "invalid-origin", "version": "1"}
                }
            }))
            .send()
            .await
            .expect("origin response");
        assert_eq!(response.status(), reqwest::StatusCode::FORBIDDEN);
        listener.shutdown().await.expect("listener shutdown");
    }
}

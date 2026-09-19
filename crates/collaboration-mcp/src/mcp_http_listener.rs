use crate::mcp_server::CollaborationMcpServer;
use bytes::Bytes;
use http::Request;
use http_body_util::{BodyExt, Empty};
use hyper::{body::Incoming, service::service_fn};
use hyper_util::rt::{TokioExecutor, TokioIo};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService,
    session::{SessionManager, local::LocalSessionManager},
};
use std::{
    io,
    net::SocketAddr,
    path::PathBuf,
    str::FromStr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{net::TcpListener, task::JoinSet};
use tokio_util::sync::CancellationToken;

const MCP_ENDPOINT_PATH: &str = "/mcp";
// rmcp drains active handlers for up to five seconds after transport close;
// retain a distinct outer bound so its supported drain can finish first.
const SESSION_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

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
    session_manager: Arc<LocalSessionManager>,
    active_services: Arc<AtomicUsize>,
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
        let session_manager = Arc::new(LocalSessionManager::default());
        let active_services = Arc::new(AtomicUsize::new(0));
        let service_lifecycle = Arc::clone(&active_services);
        let service: StreamableHttpService<CollaborationMcpServer, LocalSessionManager> =
            StreamableHttpService::new(
                move || {
                    Ok(CollaborationMcpServer::with_lifecycle(
                        config.service_directory.clone(),
                        Arc::clone(&service_lifecycle),
                    ))
                },
                Arc::clone(&session_manager),
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
            session_manager,
            active_services,
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
        let listener_result = match self.accept_task.take() {
            Some(task) => match task.await {
                Ok(result) => result,
                Err(error) => Err(io::Error::other(error.to_string())),
            },
            None => Ok(()),
        };
        let retirement_result = retire_sessions(
            Arc::clone(&self.session_manager),
            Arc::clone(&self.active_services),
        )
        .await;
        listener_result.and(retirement_result)
    }

    pub async fn listener_failure(&mut self) -> io::Error {
        let Some(task) = &mut self.accept_task else {
            return io::Error::other("MCP listener task missing");
        };
        let result = task.await;
        self.accept_task.take();
        self.shutdown.cancel();
        let listener_error = match result {
            Ok(Err(error)) => error,
            Ok(Ok(())) => io::Error::other("collaboration MCP listener stopped unexpectedly"),
            Err(error) => io::Error::other(error.to_string()),
        };
        match retire_sessions(
            Arc::clone(&self.session_manager),
            Arc::clone(&self.active_services),
        )
        .await
        {
            Ok(()) => listener_error,
            Err(retirement_error) => io::Error::other(format!(
                "{listener_error}; MCP session retirement also failed: {retirement_error}"
            )),
        }
    }
}

impl Drop for CollaborationMcpListener {
    fn drop(&mut self) {
        self.shutdown.cancel();
        if let Some(task) = self.accept_task.take() {
            task.abort();
        }
        let session_manager = Arc::clone(&self.session_manager);
        let active_services = Arc::clone(&self.active_services);
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let _retired = retire_sessions(session_manager, active_services).await;
            });
        }
    }
}

async fn retire_sessions(
    session_manager: Arc<LocalSessionManager>,
    active_services: Arc<AtomicUsize>,
) -> io::Result<()> {
    let session_ids = session_manager
        .sessions
        .read()
        .await
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    let mut first_error = None;
    for session_id in session_ids {
        let result = tokio::time::timeout(
            SESSION_SHUTDOWN_TIMEOUT,
            session_manager.close_session(&session_id),
        )
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "MCP session shutdown timed out"))
        .and_then(|result| result.map_err(|error| io::Error::other(error.to_string())));
        if first_error.is_none() {
            first_error = result.err();
        }
    }
    if first_error.is_none() {
        let settled = tokio::time::timeout(SESSION_SHUTDOWN_TIMEOUT, async {
            while active_services.load(Ordering::SeqCst) != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await;
        if settled.is_err() {
            first_error = Some(io::Error::new(
                io::ErrorKind::TimedOut,
                "MCP services did not settle after session close",
            ));
        }
    }
    first_error.map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use super::{CollaborationMcpListener, CollaborationMcpListenerConfig, LoopbackBindAddress};
    use reqwest::header::{ACCEPT, CONTENT_TYPE, HeaderMap, HeaderValue, ORIGIN};
    use rmcp::transport::streamable_http_server::session::SessionManager;
    use serde_json::{Value, json};
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
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
        let session_manager = std::sync::Arc::clone(&listener.session_manager);
        let (_session_id, _transport) = session_manager
            .create_session()
            .await
            .expect("owned MCP session");
        assert_eq!(session_manager.sessions.read().await.len(), 1);
        drop(listener);
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while !session_manager.sessions.read().await.is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("drop retires owned MCP session within bounded shutdown");
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
        let session_manager = std::sync::Arc::clone(&listener.session_manager);
        let (_session_id, _transport) = session_manager
            .create_session()
            .await
            .expect("owned MCP session");
        listener.accept_task.as_ref().expect("accept task").abort();

        let failure = listener.listener_failure().await;
        assert!(failure.to_string().contains("cancelled"));
        assert!(
            session_manager.sessions.read().await.is_empty(),
            "listener failure retires owned MCP sessions"
        );
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
        let endpoint =
            json!({"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"});
        let identity = collaboration_service::ServiceIdentity::new(
            "00000000-0000-4000-8000-000000000001",
            "00000000-0000-4000-8000-000000000002",
            &digest,
        )
        .expect("service identity")
        .with_endpoints(vec![serde_json::from_value(json!({
            "endpoint":endpoint,
            "label":"MCP reconnect fixture",
            "availability":{"state":"available","observedAt":"2026-09-19T00:00:00Z"},
            "channels":[{"kind":"acp","transport":"unixJsonLines","path":"acp.sock","schemaDigest":format!("sha256:{}", collaboration_protocol::ACP_SCHEMA_DIGEST)}]
        })).expect("endpoint description")])
        .expect("endpoint directory");
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
        let acp = tokio::net::UnixListener::bind(temporary.path().join("acp.sock"))
            .expect("ACP listener");
        let (active_prompt_tx, active_prompt_rx) = tokio::sync::oneshot::channel();
        let acp_peer = tokio::spawn(async move {
            let mut active_prompt_tx = Some(active_prompt_tx);
            for connection_index in 0..3 {
                let (stream, _) = acp.accept().await.expect("ACP accept");
                let (reader, mut writer) = stream.into_split();
                let mut lines = BufReader::new(reader).lines();
                let initialize: Value = serde_json::from_str(
                    &lines
                        .next_line()
                        .await
                        .expect("ACP initialize read")
                        .expect("ACP initialize frame"),
                )
                .expect("ACP initialize JSON");
                writer.write_all(format!("{}\n", json!({"jsonrpc":"2.0","id":initialize["id"],"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true},"authMethods":[]}})).as_bytes()).await.expect("ACP initialize response");
                let operation: Value = serde_json::from_str(
                    &lines
                        .next_line()
                        .await
                        .expect("ACP operation read")
                        .expect("ACP operation frame"),
                )
                .expect("ACP operation JSON");
                if connection_index == 0 {
                    assert_eq!(operation["method"], "session/new");
                    writer.write_all(format!("{}\n", json!({"jsonrpc":"2.0","id":operation["id"],"result":{"sessionId":"reconnect-thread"}})).as_bytes()).await.expect("ACP create response");
                } else {
                    assert_eq!(operation["method"], "session/load");
                    assert_eq!(operation["params"]["sessionId"], "reconnect-thread");
                    writer
                        .write_all(
                            format!(
                                "{}\n",
                                json!({"jsonrpc":"2.0","id":operation["id"],"result":{}})
                            )
                            .as_bytes(),
                        )
                        .await
                        .expect("ACP load response");
                    let prompt: Value = serde_json::from_str(
                        &lines
                            .next_line()
                            .await
                            .expect("ACP prompt read")
                            .expect("ACP prompt frame"),
                    )
                    .expect("ACP prompt JSON");
                    assert_eq!(prompt["method"], "session/prompt");
                    if connection_index == 1 {
                        writer.write_all(format!("{}\n", json!({"jsonrpc":"2.0","id":prompt["id"],"result":{"stopReason":"end_turn"}})).as_bytes()).await.expect("ACP prompt response");
                    } else {
                        active_prompt_tx
                            .take()
                            .expect("active prompt signal")
                            .send(())
                            .expect("signal active prompt");
                        let cancel: Value = serde_json::from_str(
                            &lines
                                .next_line()
                                .await
                                .expect("ACP cancel read")
                                .expect("ACP cancel frame"),
                        )
                        .expect("ACP cancel JSON");
                        assert_eq!(cancel["method"], "session/cancel");
                        writer
                            .write_all(
                                format!(
                                    "{}\n",
                                    json!({"jsonrpc":"2.0","id":cancel["id"],"result":{}})
                                )
                                .as_bytes(),
                            )
                            .await
                            .expect("ACP cancel response");
                        writer.write_all(format!("{}\n", json!({"jsonrpc":"2.0","id":prompt["id"],"result":{"stopReason":"cancelled","_meta":{"codex-router/nativeInterruption":{"state":"confirmed"}}}})).as_bytes()).await.expect("ACP cancelled prompt response");
                    }
                }
            }
        });
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

        let mut idle_stream = tokio::net::TcpStream::connect(listener.local_address())
            .await
            .expect("standalone SSE connection");
        idle_stream
            .write_all(
                format!(
                    "GET /mcp HTTP/1.1\r\nHost: {}\r\nAccept: text/event-stream\r\nmcp-session-id: {}\r\nmcp-protocol-version: 2025-11-25\r\nConnection: close\r\n\r\n",
                    listener.local_address(),
                    session_id.to_str().expect("session header")
                )
                .as_bytes(),
            )
            .await
            .expect("standalone SSE request");
        let mut response_prefix = vec![0_u8; 1024];
        let prefix_bytes = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            idle_stream.read(&mut response_prefix),
        )
        .await
        .expect("SSE response starts")
        .expect("SSE response read");
        assert!(
            String::from_utf8_lossy(&response_prefix[..prefix_bytes]).contains("200 OK"),
            "standalone SSE request must reach the initialized rmcp service"
        );

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
            .header("mcp-session-id", session_id.clone())
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
        let create = client.post(listener.local_url()).header(CONTENT_TYPE,"application/json").header(ACCEPT,"application/json, text/event-stream").header("mcp-session-id",session_id.clone()).header("mcp-protocol-version","2025-11-25").json(&json!({
            "jsonrpc":"2.0","id":31,"method":"tools/call","params":{"name":"conversation_create","arguments":{
                "endpoint":endpoint,"cwd":temporary.path(),"session":null,"fork":null,
                "model":"gpt-5.6-luna","effort":"low","access":"workspace-write",
                "createdBy":null,"approver":null,"rootMessageId":null
            }}
        })).send().await.expect("conversation create response");
        let create_body = protocol_response_json(create).await;
        let created_target = create_body
            .pointer("/result/structuredContent/target")
            .cloned()
            .expect("created target");
        assert_eq!(created_target["sessionId"], "reconnect-thread");
        let reconnect_initialize = client
            .post(listener.local_url())
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json, text/event-stream")
            .json(&json!({
                "jsonrpc":"2.0",
                "id":4,
                "method":"initialize",
                "params":{
                    "protocolVersion":"2025-11-25",
                    "capabilities":{},
                    "clientInfo":{"name":"collaboration-mcp-reconnect-test","version":"1"}
                }
            }))
            .send()
            .await
            .expect("reconnect initialize response");
        assert!(reconnect_initialize.status().is_success());
        let reconnect_session_id = reconnect_initialize
            .headers()
            .get("mcp-session-id")
            .cloned()
            .expect("reconnect transport session id");
        assert_ne!(reconnect_session_id, session_id);
        let reconnect_body = protocol_response_json(reconnect_initialize).await;
        assert_eq!(
            reconnect_body.pointer("/result/serverInfo/name"),
            Some(&json!("codex-router-collaboration")),
            "a new MCP transport initialization remains a transport concern, not a Router conversation replacement"
        );
        let reconnect_initialized = client
            .post(listener.local_url())
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json, text/event-stream")
            .header("mcp-session-id", reconnect_session_id.clone())
            .header("mcp-protocol-version", "2025-11-25")
            .json(&json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}}))
            .send()
            .await
            .expect("reconnect initialized");
        assert!(reconnect_initialized.status().is_success());
        let resumed = client.post(listener.local_url()).header(CONTENT_TYPE,"application/json").header(ACCEPT,"application/json, text/event-stream").header("mcp-session-id",reconnect_session_id.clone()).header("mcp-protocol-version","2025-11-25").json(&json!({
            "jsonrpc":"2.0","id":41,"method":"tools/call","params":{"name":"conversation_prompt","arguments":{
                "target":created_target.clone(),"cwd":temporary.path(),"prompt":{"message":{"kind":"humanUser","text":"resume without replay"},"effort":null,"timeoutSeconds":3}
            }}
        })).send().await.expect("reconnect prompt response");
        let resumed_body = protocol_response_json(resumed).await;
        assert_eq!(
            resumed_body.pointer("/result/structuredContent/target/sessionId"),
            Some(&json!("reconnect-thread"))
        );
        let active_client = client.clone();
        let active_url = listener.local_url();
        let active_session_id = reconnect_session_id.clone();
        let active_target = created_target.clone();
        let active_cwd = temporary.path().to_owned();
        let active_prompt = tokio::spawn(async move {
            active_client.post(active_url).header(CONTENT_TYPE,"application/json").header(ACCEPT,"application/json, text/event-stream").header("mcp-session-id",active_session_id).header("mcp-protocol-version","2025-11-25").json(&json!({
                "jsonrpc":"2.0","id":42,"method":"tools/call","params":{"name":"conversation_prompt","arguments":{
                    "target":active_target,"cwd":active_cwd,"prompt":{"message":{"kind":"humanUser","text":"active shutdown proof"},"effort":null,"timeoutSeconds":60}
                }}
            })).send().await
        });
        active_prompt_rx
            .await
            .expect("active bounded operation starts");
        let session_manager = std::sync::Arc::clone(&listener.session_manager);
        assert_eq!(
            session_manager.sessions.read().await.len(),
            2,
            "each initialized HTTP transport owns separate in-memory MCP session state"
        );
        listener.shutdown().await.expect("listener shutdown");
        assert!(
            session_manager.sessions.read().await.is_empty(),
            "listener shutdown retires owned MCP sessions rather than waiting for idle expiry"
        );
        let mut remainder = Vec::new();
        let stream_end = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            idle_stream.read_to_end(&mut remainder),
        )
        .await;
        assert!(
            stream_end.is_ok(),
            "session close must terminate the actual rmcp service/SSE stream"
        );
        let active_result = tokio::time::timeout(std::time::Duration::from_secs(1), active_prompt)
            .await
            .expect("active bounded operation settles during shutdown")
            .expect("active request join");
        assert!(
            active_result.is_err()
                || active_result.is_ok_and(|response| response.status().is_success()),
            "active request either loses the closing transport or returns its cancellation settlement"
        );
        control_stop.cancel();
        control_task
            .await
            .expect("control join")
            .expect("control shutdown");
        acp_peer.await.expect("ACP peer join");
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

    #[tokio::test]
    async fn real_http_initialization_supports_ipv6_loopback() {
        let temporary = tempfile::tempdir().expect("temporary service directory");
        let listener = CollaborationMcpListener::start(CollaborationMcpListenerConfig {
            bind_address: LoopbackBindAddress::parse("[::1]:0").expect("IPv6 loopback bind"),
            service_directory: temporary.path().to_owned(),
            allowed_origins: Vec::new(),
        })
        .await
        .expect("IPv6 MCP listener starts");
        let response = reqwest::Client::new()
            .post(listener.local_url())
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json, text/event-stream")
            .json(&json!({
                "jsonrpc":"2.0",
                "id":1,
                "method":"initialize",
                "params":{
                    "protocolVersion":"2025-11-25",
                    "capabilities":{},
                    "clientInfo":{"name":"ipv6-proof","version":"1"}
                }
            }))
            .send()
            .await
            .expect("IPv6 initialize response");
        assert!(response.status().is_success());
        assert!(response.headers().contains_key("mcp-session-id"));
        listener.shutdown().await.expect("IPv6 listener shutdown");
    }
}

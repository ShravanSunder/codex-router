use super::{CollaborationMcpListener, CollaborationMcpListenerConfig, LoopbackBindAddress};
use futures_util::{SinkExt, StreamExt};
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use serde_json::{Value, json};
use std::os::unix::fs::PermissionsExt;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

const SERVICE_ID: &str = "00000000-0000-4000-8000-000000000001";
const SERVICE_EPOCH: &str = "00000000-0000-4000-8000-000000000002";

#[tokio::test]
async fn initialized_http_application_deadline_returns_timed_out_settlement() {
    let fixture = ConversationFixture::start("deadline-acp.sock").await;
    let acp_path = fixture.root.path().join("deadline-acp.sock");
    let acp = tokio::net::UnixListener::bind(&acp_path).expect("ACP listener");
    let peer = tokio::spawn(async move {
        let (stream, _) = acp.accept().await.expect("ACP accept");
        let (reader, mut writer) = stream.into_split();
        let mut lines = BufReader::new(reader).lines();
        let initialize = next_json_line(&mut lines, "initialize").await;
        writer.write_all(format!("{}\n", json!({"jsonrpc":"2.0","id":initialize["id"],"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true},"authMethods":[]}})).as_bytes()).await.expect("initialize response");
        let create = next_json_line(&mut lines, "create").await;
        assert_eq!(create["method"], "session/new");
        writer.write_all(format!("{}\n", json!({"jsonrpc":"2.0","id":create["id"],"result":{"sessionId":"deadline-thread"}})).as_bytes()).await.expect("create response");
        let prompt = next_json_line(&mut lines, "prompt").await;
        assert_eq!(prompt["method"], "session/prompt");
        let cancel = next_json_line(&mut lines, "cancel").await;
        assert_eq!(cancel["method"], "session/cancel");
        writer
            .write_all(
                format!(
                    "{}\n",
                    json!({"jsonrpc":"2.0","id":prompt["id"],"result":{"stopReason":"cancelled"}})
                )
                .as_bytes(),
            )
            .await
            .expect("deadline settlement");
    });
    let client = reqwest::Client::new();
    let session_id = initialize_mcp(&client, &fixture.listener).await;
    let endpoint = json!({"serviceId":SERVICE_ID,"endpointId":"codex-local"});
    let caller = json!({"endpoint":endpoint,"sessionId":"deadline-caller"});
    let response = client.post(fixture.listener.local_url()).header(CONTENT_TYPE,"application/json").header(ACCEPT,"application/json, text/event-stream").header("mcp-session-id",session_id).header("mcp-protocol-version","2025-11-25").json(&json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"conversation_create_and_prompt","arguments":{
            "create":{"endpoint":endpoint,"cwd":fixture.root.path(),"session":null,"fork":null,"model":"gpt-5.6-luna","effort":"low","access":"workspace-write","createdBy":caller,"approver":caller,"rootMessageId":null},
            "prompt":{"message":{"kind":"humanUser","text":"deadline proof"},"effort":"low","timeoutSeconds":1}
        }}
    })).send().await.expect("deadline response");
    let body = protocol_response_json(response).await;
    assert_eq!(body.pointer("/result/isError"), Some(&json!(false)));
    assert_eq!(
        body.pointer("/result/structuredContent/end"),
        Some(&json!("timedOut"))
    );
    assert_eq!(
        body.pointer("/result/structuredContent/result/stopReason"),
        Some(&json!("cancelled"))
    );
    peer.await.expect("ACP peer");
    fixture.shutdown().await;
}

#[derive(Clone, Copy)]
enum ObservationClosure {
    Clean,
    Malformed,
}

#[tokio::test]
async fn initialized_http_observation_distinguishes_malformed_frame_from_clean_eof() {
    for closure in [ObservationClosure::Clean, ObservationClosure::Malformed] {
        let fixture = NativeObservationFixture::start(closure).await;
        let client = reqwest::Client::new();
        let session_id = initialize_mcp(&client, &fixture.fixture.listener).await;
        let response = client.post(fixture.fixture.listener.local_url()).header(CONTENT_TYPE,"application/json").header(ACCEPT,"application/json, text/event-stream").header("mcp-session-id",session_id).header("mcp-protocol-version","2025-11-25").json(&json!({
            "jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"events_observe","arguments":{
                "target":{"endpoint":{"serviceId":SERVICE_ID,"endpointId":"codex-local"},"sessionId":"observe-thread"},
                "timeoutSeconds":2,"maxEvents":8,"maxBytes":4096
            }}
        })).send().await.expect("observation response");
        let body = protocol_response_json(response).await;
        match closure {
            ObservationClosure::Clean => {
                assert_eq!(body.pointer("/result/isError"), Some(&json!(false)));
                assert_eq!(
                    body.pointer("/result/structuredContent/endReason"),
                    Some(&json!("backendDisconnected"))
                );
            }
            ObservationClosure::Malformed => {
                assert_eq!(body.pointer("/result/isError"), Some(&json!(true)));
                assert_eq!(
                    body.pointer("/result/structuredContent/kind"),
                    Some(&json!("protocolViolation"))
                );
                assert_eq!(
                    body.pointer("/result/structuredContent/stage"),
                    Some(&json!("observation-collect"))
                );
            }
        }
        fixture.peer.await.expect("native peer");
        fixture.fixture.shutdown().await;
    }
}

struct ConversationFixture {
    root: tempfile::TempDir,
    listener: CollaborationMcpListener,
    stop: CancellationToken,
    control_task: tokio::task::JoinHandle<std::io::Result<()>>,
    publication: collaboration_service::ManifestPublication,
}

impl ConversationFixture {
    async fn start(channel_path: &str) -> Self {
        let root = tempfile::tempdir().expect("temporary service directory");
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))
            .expect("private directory");
        let digest = format!("sha256:{}", "a".repeat(64));
        let endpoint = serde_json::from_value(json!({
            "endpoint":{"serviceId":SERVICE_ID,"endpointId":"codex-local"},"label":"remediation fixture",
            "availability":{"state":"available","observedAt":"2026-09-19T00:00:00Z"},
            "channels":[{"kind":"acp","transport":"unixJsonLines","path":channel_path,"schemaDigest":format!("sha256:{}", collaboration_protocol::ACP_SCHEMA_DIGEST)}]
        })).expect("endpoint");
        Self::start_with_endpoint(root, digest, endpoint).await
    }

    async fn start_with_endpoint(
        root: tempfile::TempDir,
        digest: String,
        endpoint: collaboration_protocol::EndpointDescription,
    ) -> Self {
        let identity =
            collaboration_service::ServiceIdentity::new(SERVICE_ID, SERVICE_EPOCH, &digest)
                .expect("identity")
                .with_endpoints(vec![endpoint])
                .expect("endpoint directory");
        let control = collaboration_service::LocalControlService::bind(
            &root.path().join("control.sock"),
            identity,
        )
        .expect("control");
        let manifest = serde_json::from_value(json!({"version":2,"serviceId":SERVICE_ID,"serviceEpoch":SERVICE_EPOCH,"control":{"transport":"unixJsonLines","path":"control.sock"},"controlSchemaDigest":digest,"mcp":{"transport":"streamableHttp","url":"http://127.0.0.1:0/mcp"}})).expect("manifest");
        let publication =
            collaboration_service::ManifestPublication::publish(root.path(), &manifest)
                .expect("publication");
        let stop = CancellationToken::new();
        let control_task = tokio::spawn(control.run(stop.clone()));
        let listener = CollaborationMcpListener::start(CollaborationMcpListenerConfig {
            bind_address: LoopbackBindAddress::parse("127.0.0.1:0").expect("bind"),
            service_directory: root.path().to_owned(),
            allowed_origins: Vec::new(),
        })
        .await
        .expect("listener");
        Self {
            root,
            listener,
            stop,
            control_task,
            publication,
        }
    }

    async fn shutdown(self) {
        self.listener.shutdown().await.expect("listener shutdown");
        self.stop.cancel();
        self.control_task
            .await
            .expect("control join")
            .expect("control shutdown");
        drop(self.publication);
    }
}

struct NativeObservationFixture {
    fixture: ConversationFixture,
    peer: tokio::task::JoinHandle<()>,
}

impl NativeObservationFixture {
    async fn start(closure: ObservationClosure) -> Self {
        let root = tempfile::tempdir().expect("temporary service directory");
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))
            .expect("private directory");
        let native = tokio::net::UnixListener::bind(root.path().join("native.sock"))
            .expect("native listener");
        let peer = tokio::spawn(async move {
            let (stream, _) = native.accept().await.expect("native accept");
            let mut socket = tokio_tungstenite::accept_async(stream)
                .await
                .expect("native websocket");
            let initialize = next_ws_json(&mut socket).await;
            socket
                .send(Message::Text(
                    json!({"id":initialize["id"],"result":{"userAgent":"fixture"}})
                        .to_string()
                        .into(),
                ))
                .await
                .expect("initialize response");
            assert_eq!(next_ws_json(&mut socket).await["method"], "initialized");
            let resume = next_ws_json(&mut socket).await;
            socket
                .send(Message::Text(
                    json!({"id":resume["id"],"result":{"thread":{"id":"observe-thread"}}})
                        .to_string()
                        .into(),
                ))
                .await
                .expect("resume response");
            if matches!(closure, ObservationClosure::Malformed) {
                socket
                    .send(Message::Text("{".into()))
                    .await
                    .expect("malformed frame");
            }
            socket.close(None).await.expect("native close");
            while let Some(Ok(frame)) = socket.next().await {
                assert!(
                    !matches!(frame, Message::Text(_)),
                    "observer interrupted the running turn"
                );
            }
        });
        let digest = format!("sha256:{}", "a".repeat(64));
        let endpoint = serde_json::from_value(json!({"endpoint":{"serviceId":SERVICE_ID,"endpointId":"codex-local"},"label":"native remediation fixture","availability":{"state":"available","observedAt":"2026-09-19T00:00:00Z"},"channels":[{"kind":"nativeCodex","transport":"unixWebSocket","path":"native.sock","schemaDigest":null,"generation":{"serviceEpoch":SERVICE_EPOCH,"generation":1}}]})).expect("endpoint");
        let fixture = ConversationFixture::start_with_endpoint(root, digest, endpoint).await;
        Self { fixture, peer }
    }
}

async fn next_json_line(
    lines: &mut tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>,
    label: &str,
) -> Value {
    serde_json::from_str(&lines.next_line().await.expect(label).expect(label)).expect(label)
}

async fn next_ws_json(
    socket: &mut tokio_tungstenite::WebSocketStream<tokio::net::UnixStream>,
) -> Value {
    let frame = socket
        .next()
        .await
        .expect("native frame")
        .expect("native read");
    serde_json::from_str(frame.to_text().expect("native text")).expect("native JSON")
}

async fn initialize_mcp(
    client: &reqwest::Client,
    listener: &CollaborationMcpListener,
) -> reqwest::header::HeaderValue {
    let response = client.post(listener.local_url()).header(CONTENT_TYPE,"application/json").header(ACCEPT,"application/json, text/event-stream").json(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"remediation-proof","version":"1"}}})).send().await.expect("initialize");
    let session = response
        .headers()
        .get("mcp-session-id")
        .cloned()
        .expect("session ID");
    let _ = protocol_response_json(response).await;
    let initialized = client
        .post(listener.local_url())
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream")
        .header("mcp-session-id", session.clone())
        .header("mcp-protocol-version", "2025-11-25")
        .json(&json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}}))
        .send()
        .await
        .expect("initialized");
    assert!(initialized.status().is_success());
    session
}

async fn protocol_response_json(response: reqwest::Response) -> Value {
    let body = response.text().await.expect("response body");
    let encoded = body
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .find(|data| !data.is_empty())
        .unwrap_or(body.as_str());
    serde_json::from_str(encoded).unwrap_or_else(|error| panic!("response JSON: {error}; {body}"))
}

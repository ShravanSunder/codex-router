#![allow(clippy::unwrap_used, clippy::expect_used, dead_code)]

use super::adapter::{AcpSchemaCatalog, AcpSessionBinding, SessionSetupInputs};
use codex_native_integration::{
    NativePayloadSchemas, NativeProtocolConnection, NativeSchemaBundle,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::oneshot;
use tokio_tungstenite::{WebSocketStream, tungstenite::Message};

pub struct AcceptingConversationRecorder;
impl super::adapter::ConversationOperationRecorder for AcceptingConversationRecorder {
    fn admit_create<'a>(
        &'a self,
        _: &'a collaboration_protocol::OperationId,
        _: &'a collaboration_protocol::CodexGeneration,
    ) -> super::adapter::ConversationRecordFuture<'a> {
        Box::pin(async { Ok(()) })
    }
    fn before_native_dispatch<'a>(
        &'a self,
        _: &'a collaboration_protocol::OperationId,
    ) -> super::adapter::ConversationRecordFuture<'a> {
        Box::pin(async { Ok(()) })
    }
    fn record_created<'a>(
        &'a self,
        _: &'a collaboration_protocol::OperationId,
        _: &'a collaboration_protocol::SessionId,
    ) -> super::adapter::ConversationRecordFuture<'a> {
        Box::pin(async { Ok(()) })
    }
    fn record_failure<'a>(
        &'a self,
        _: &'a collaboration_protocol::OperationId,
        _: bool,
    ) -> super::adapter::ConversationRecordFuture<'a> {
        Box::pin(async { Ok(()) })
    }
}

pub const THREAD_ID: &str = "thread-actor-lifetime";
pub const TURN_ID: &str = "turn-actor-lifetime";
pub const BOUND: Duration = Duration::from_secs(3);
static NEXT_SOCKET: AtomicUsize = AtomicUsize::new(0);

pub fn generation() -> collaboration_protocol::CodexGeneration {
    serde_json::from_value(
        json!({"serviceEpoch":"00000000-0000-4000-8000-000000000001","generation":1}),
    )
    .unwrap()
}
pub fn schemas() -> Arc<NativePayloadSchemas> {
    let mut definitions = serde_json::Map::new();
    for name in [
        "ThreadRead",
        "ThreadResume",
        "ThreadStart",
        "ThreadLoadedList",
        "TurnStart",
        "TurnSteer",
        "TurnInterrupt",
    ] {
        definitions.insert(format!("{name}Params"), json!({"type":"object"}));
        definitions.insert(format!("{name}Response"), json!({"type":"object"}));
    }
    let bundle = NativeSchemaBundle::from_documents(BTreeMap::from([("codex_app_server_protocol.schemas.json".to_owned(), serde_json::to_vec(&json!({"definitions":{"v2":definitions,"ServerRequest":{"type":"object"},"ServerNotification":{"type":"object"}}})).unwrap())])).unwrap();
    Arc::new(NativePayloadSchemas::from_bundle(&bundle).unwrap())
}
pub fn prompt() -> Value {
    json!({"sessionId":THREAD_ID,"prompt":[{"type":"text","text":"keep serving"}]})
}
pub fn load() -> Value {
    json!({"sessionId":THREAD_ID,"cwd":"/work","mcpServers":[]})
}
async fn request(wire: &mut WebSocketStream<tokio::net::UnixStream>) -> Value {
    let frame = tokio::time::timeout(BOUND, wire.next())
        .await
        .expect("native request deadline")
        .expect("native actor remains connected")
        .unwrap();
    serde_json::from_str(frame.to_text().unwrap()).unwrap()
}
async fn reply(wire: &mut WebSocketStream<tokio::net::UnixStream>, request: &Value, result: Value) {
    wire.send(Message::Text(
        json!({"id":request["id"],"result":result})
            .to_string()
            .into(),
    ))
    .await
    .unwrap();
}

pub struct NativeActorFixture {
    pub path: PathBuf,
    pub started: oneshot::Receiver<()>,
    pub finish: oneshot::Sender<()>,
    pub backend: tokio::task::JoinHandle<()>,
}
impl NativeActorFixture {
    pub fn start(expect_retirement: bool) -> Self {
        let path = std::env::temp_dir().join(format!(
            "actor-life-{}-{}.sock",
            std::process::id(),
            NEXT_SOCKET.fetch_add(1, Ordering::Relaxed)
        ));
        let listener = tokio::net::UnixListener::bind(&path).unwrap();
        let (started_tx, started) = oneshot::channel();
        let (finish, finish_rx) = oneshot::channel();
        let backend = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut wire = tokio_tungstenite::accept_async(stream).await.unwrap();
            for method in [
                "initialize",
                "initialized",
                "thread/resume",
                "thread/read",
                "turn/start",
            ] {
                let incoming = request(&mut wire).await;
                assert_eq!(incoming.get("method"), Some(&json!(method)));
                if method == "initialized" {
                    continue;
                }
                if method == "turn/start" {
                    assert_eq!(
                        incoming.pointer("/params/threadId"),
                        Some(&json!(THREAD_ID))
                    );
                }
                let result = match method {
                    "initialize" => json!({}),
                    "thread/resume" => {
                        json!({"cwd":"/work","model":"gpt-5.6-sol","thread":{"id":THREAD_ID,"cwd":"/work","status":{"type":"idle"},"turns":[]}})
                    }
                    "thread/read" => {
                        json!({"thread":{"id":THREAD_ID,"status":{"type":"idle"},"turns":[]}})
                    }
                    _ => json!({"turn":{"id":TURN_ID,"status":"inProgress"}}),
                };
                reply(&mut wire, &incoming, result).await;
            }
            if expect_retirement {
                wire.send(Message::Text(json!({"method":"item/agentMessage/delta","params":{"threadId":THREAD_ID,"turnId":TURN_ID,"itemId":"message-before-retirement","delta":"actor entered native event loop"}}).to_string().into())).await.unwrap();
            }
            started_tx.send(()).unwrap();
            if expect_retirement {
                let frame = tokio::time::timeout(BOUND, wire.next())
                    .await
                    .expect("generation loss closes actor");
                assert!(
                    !matches!(frame, Some(Ok(Message::Text(_)))),
                    "retirement must not send turn/interrupt: {frame:?}"
                );
                return;
            }
            tokio::select! {
                result = finish_rx => { result.unwrap(); },
                frame = wire.next() => panic!("observer loss closed actor or sent an unsolicited request: {frame:?}"),
            }
            wire.send(Message::Text(json!({"method":"item/agentMessage/delta","params":{"threadId":THREAD_ID,"turnId":TURN_ID,"itemId":"message-lifetime","delta":"late original turn output"}}).to_string().into())).await.unwrap();
            wire.send(Message::Text(json!({"method":"turn/completed","params":{"threadId":THREAD_ID,"turn":{"id":TURN_ID,"status":"completed"}}}).to_string().into())).await.unwrap();
            let completed_read = request(&mut wire).await;
            assert_eq!(completed_read.get("method"), Some(&json!("thread/read")));
            assert_eq!(
                completed_read.get("params"),
                Some(&json!({"threadId":THREAD_ID,"includeTurns":false}))
            );
            reply(&mut wire, &completed_read, json!({"thread":{"id":THREAD_ID,"model":"gpt-5.6-sol","reasoningEffort":"medium","createdAt":1,"updatedAt":1}})).await;
        });
        Self {
            path,
            started,
            finish,
            backend,
        }
    }
    pub async fn binding(&self) -> AcpSessionBinding {
        let connection = NativeProtocolConnection::connect(&self.path).await.unwrap();
        AcpSessionBinding::load_existing(
            &mut AcpSchemaCatalog::load().unwrap(),
            SessionSetupInputs {
                connection,
                schemas: schemas(),
                generation: generation(),
                params: load(),
                operation_id: None,
                recorder: Arc::new(AcceptingConversationRecorder),
                approval_broker: Arc::new(super::adapter::RejectingApprovalBroker),
            },
        )
        .await
        .unwrap()
        .0
    }
}

#[path = "native_permission_echo.rs"]
mod native_permission_echo;

pub async fn unmaterialized_binding() -> (AcpSessionBinding, tokio::task::JoinHandle<()>) {
    use std::os::unix::fs::PermissionsExt;
    let scratch = "/tmp/router-acp-tests/scratch/session-00000000-0000-4000-8000-000000000098";
    std::fs::create_dir_all(scratch).unwrap();
    std::fs::set_permissions(scratch, std::fs::Permissions::from_mode(0o700)).unwrap();
    let (client, server) = tokio::net::UnixStream::pair().unwrap();
    let backend = tokio::spawn(async move {
        let mut wire = WebSocketStream::from_raw_socket(
            server,
            tokio_tungstenite::tungstenite::protocol::Role::Server,
            None,
        )
        .await;
        let create = request(&mut wire).await;
        assert_eq!(create.get("method"), Some(&json!("thread/start")));
        reply(&mut wire, &create, json!({"cwd":"/work","model":"gpt-5.6-sol","approvalPolicy":"on-request","approvalsReviewer":"auto_review","activePermissionProfile":{"id":"router-workspace-write","extends":":workspace"},"sandbox":native_permission_echo::applied_router_sandbox(&create),"thread":{"id":THREAD_ID,"cwd":"/work","status":{"type":"idle"},"turns":[]}})).await;
        let frame = tokio::time::timeout(BOUND, wire.next())
            .await
            .expect("held binding eventually released");
        assert!(
            !matches!(frame, Some(Ok(Message::Text(_)))),
            "early cancel must not dispatch native prompt: {frame:?}"
        );
    });
    let connection = NativeProtocolConnection::from_websocket(
        WebSocketStream::from_raw_socket(
            client,
            tokio_tungstenite::tungstenite::protocol::Role::Client,
            None,
        )
        .await,
    );
    let session = AcpSessionBinding::create(&mut AcpSchemaCatalog::load().unwrap(), SessionSetupInputs {
        connection, schemas: schemas(), generation: generation(), operation_id: Some(collaboration_protocol::OperationId::generate()),
        recorder: Arc::new(AcceptingConversationRecorder), approval_broker: Arc::new(super::adapter::RejectingApprovalBroker),
        params: json!({"cwd":"/work","mcpServers":[],"_meta":{"codexRouter":{"model":"gpt-5.6-sol","effort":"medium","access":"workspace-write","scratchScope":"session-00000000-0000-4000-8000-000000000098","scratchPath":scratch,"createdBy":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"sessionId":"creator"},"approver":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"sessionId":"creator"}}}}),
    }).await.unwrap();
    assert!(session.is_unmaterialized());
    (session, backend)
}

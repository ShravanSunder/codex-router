//! ACP frames and command admission meet at the Session actor's idle boundary.
#![allow(clippy::expect_used)]

use super::*;
use crate::{
    ApprovalPortOutcome, EventSinkClosed, HistoryReplayFuture, InteractionFuture,
    RefusedApprovalOffer, SessionEventSink,
};
use agent_client_protocol::schema::v1::ContentBlock;
use session_event_model::{ApprovalRequest, InputId, SessionEvent};
use std::{
    path::PathBuf,
    sync::{
        Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

const CANCELLED_PROMPT_ORDER_FIXTURE: &str = r#"
import json,socket,sys
def read(): return json.loads(sys.stdin.readline())
def send(value): print(json.dumps(value),flush=True)
def update(text):
    send({'jsonrpc':'2.0','method':'session/update','params':{'sessionId':'fixture-session',
        'update':{'sessionUpdate':'agent_message_chunk','content':{'type':'text','text':text}}}})
control=socket.socket(socket.AF_UNIX,socket.SOCK_STREAM)
control.connect(sys.argv[1])
request=read()
send({'jsonrpc':'2.0','id':request['id'],'result':{'protocolVersion':1,
    'agentCapabilities':{},'agentInfo':{'name':'cancel-order-fixture','version':'1'}}})
request=read()
send({'jsonrpc':'2.0','id':request['id'],'result':{'sessionId':'fixture-session'}})
first=read()
assert first['method']=='session/prompt'
update('x'*(1024*1024+1))
cancel=read()
assert cancel['method']=='session/cancel'
send({'jsonrpc':'2.0','id':first['id'],'result':{'stopReason':'cancelled'}})
# The test holds terminal publication while the successor Prompt is queued.
assert control.recv(1)==b'g'
update('LATE_OUTPUT')
send({'jsonrpc':'2.0','id':99,'method':'probe/unknown','params':{}})
barrier=read()
assert barrier['id']==99 and 'error' in barrier,barrier
control.sendall(b'r')
second=read()
assert second['method']=='session/prompt'
update('CURRENT_OUTPUT')
send({'jsonrpc':'2.0','id':second['id'],'result':{'stopReason':'end_turn'}})
sys.stdin.read()
"#;

struct NoopInteractionPort;
impl InteractionPort for NoopInteractionPort {
    type Context = ();
    type OperationId = u64;
    fn operation_id(_context: &Self::Context) -> Self::OperationId {
        1
    }
    fn binding_retirement(_context: &Self::Context) -> CancellationToken {
        CancellationToken::new()
    }
    fn request_approval(
        &self,
        _context: (),
        _request: ApprovalRequest,
        _turn_cancellation: CancellationToken,
        _agent_cancellation: CancellationToken,
    ) -> InteractionFuture<'_, ApprovalPortOutcome> {
        Box::pin(async { ApprovalPortOutcome::Cancelled })
    }
    fn request_question(
        &self,
        _context: (),
        _request: session_event_model::QuestionRequest,
        _turn_cancellation: CancellationToken,
        _agent_cancellation: CancellationToken,
    ) -> InteractionFuture<'_, session_event_model::QuestionResponse> {
        Box::pin(async { session_event_model::QuestionResponse::Cancelled })
    }
    fn record_refusal(
        &self,
        _context: (),
        _refusal: RefusedApprovalOffer,
    ) -> InteractionFuture<'_, ()> {
        Box::pin(async {})
    }
    fn cancel_all(&self, _context: (), _reason: &'static str) -> InteractionFuture<'_, ()> {
        Box::pin(async {})
    }
    fn cancel_retired(&self) -> InteractionFuture<'_, ()> {
        Box::pin(async {})
    }
}

struct TurnEndGate {
    entered: tokio::sync::Notify,
    blocked_once: AtomicBool,
    released: Mutex<bool>,
    release_cv: Condvar,
}
impl TurnEndGate {
    fn new() -> Self {
        Self {
            entered: tokio::sync::Notify::new(),
            blocked_once: AtomicBool::new(false),
            released: Mutex::new(false),
            release_cv: Condvar::new(),
        }
    }
    fn release(&self) {
        *self.released.lock().expect("gate lock") = true;
        self.release_cv.notify_one();
    }
}

struct GatedEventSink {
    events: tokio::sync::mpsc::UnboundedSender<SessionEvent>,
    gate: Arc<TurnEndGate>,
}
impl SessionEventSink for GatedEventSink {
    fn begin_history_replay(&self, _session_id: &str) -> HistoryReplayFuture<'_> {
        Box::pin(async { Ok(()) })
    }
    fn publish(&self, _session_id: &str, event: SessionEvent) -> Result<(), EventSinkClosed> {
        if matches!(event, SessionEvent::TurnEnded { .. })
            && !self.gate.blocked_once.swap(true, Ordering::SeqCst)
        {
            self.gate.entered.notify_one();
            let mut released = self.gate.released.lock().expect("gate lock");
            while !*released {
                released = self.gate.release_cv.wait(released).expect("gate lock");
            }
        }
        self.events.send(event).map_err(|_| EventSinkClosed)
    }
}

/// An update after a cancelled response, but ready before the queued next
/// Prompt is admitted, belongs to the old Turn. Updates arriving after the
/// next Prompt is admitted remain outside this guarantee.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ready_cancelled_turn_update_is_not_a_successor_turn_item() {
    let root = tempfile::tempdir().expect("fixture root");
    let control_path = root.path().join("control.sock");
    let control_listener = tokio::net::UnixListener::bind(&control_path).expect("control socket");
    let (event_sender, mut events) = tokio::sync::mpsc::unbounded_channel();
    let gate = Arc::new(TurnEndGate::new());
    let client = Arc::new(
        AgentSessionClient::initialize(
            ExternalProviderLaunch {
                executable: PathBuf::from("python3"),
                arguments: vec![
                    "-u".into(),
                    "-c".into(),
                    CANCELLED_PROMPT_ORDER_FIXTURE.into(),
                    control_path.to_string_lossy().into_owned(),
                ],
                environment: Vec::new(),
                persistence_target: crate::ProviderPersistenceTarget::Unspecified,
            },
            Arc::new(NoopInteractionPort),
            Arc::new(GatedEventSink {
                events: event_sender,
                gate: Arc::clone(&gate),
            }),
        )
        .await
        .expect("fixture initializes"),
    );
    let (mut control, _) = control_listener.accept().await.expect("control connection");
    let session_id = client
        .create_session(root.path().to_path_buf())
        .await
        .expect("session opens");
    let first_client = Arc::clone(&client);
    let first_session = session_id.clone();
    let first = tokio::spawn(async move {
        first_client
            .prompt_content(
                first_session,
                InputId::generate(),
                None,
                vec![text_block("overflow")],
                None,
            )
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), gate.entered.notified())
        .await
        .expect("first Turn reached terminal publication");
    let (second_reply, second) = tokio::sync::oneshot::channel();
    let capabilities = client.capability_report(&session_id).await;
    let second_prompt = ProviderPromptContent::new(vec![text_block("next")], &capabilities)
        .expect("successor prompt content");
    client
        .commands
        .send(ProviderCommand::Prompt {
            provider_session_id: session_id,
            input_id: InputId::generate(),
            operation_id: None,
            prompt: second_prompt,
            dispatch: None,
            reply: second_reply,
        })
        .await
        .expect("successor command queued");
    control.write_all(b"g").await.expect("release late update");
    let mut acknowledged = [0u8; 1];
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        control.read_exact(&mut acknowledged),
    )
    .await
    .expect("fixture sent late update")
    .expect("control reply");
    assert_eq!(acknowledged, *b"r");
    gate.release();
    assert!(matches!(
        first.await.expect("first task"),
        Err(ExternalProviderRuntimeError::PromptOutputLimitExceeded)
    ));
    let next = second
        .await
        .expect("second reply")
        .expect("successor prompt settles");
    assert_eq!(next.output, "CURRENT_OUTPUT");
    let observed = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        let mut collected = Vec::new();
        while collected
            .iter()
            .filter(|event| matches!(event, SessionEvent::TurnEnded { .. }))
            .count()
            < 2
        {
            collected.push(events.recv().await.expect("event sink remains open"));
        }
        collected
    })
    .await
    .expect("both Turns settle");
    let second_start = observed
        .iter()
        .rposition(|event| matches!(event, SessionEvent::TurnStarted { .. }))
        .expect("successor Turn started");
    assert!(
        !observed[second_start..].iter().any(|event| matches!(event,
        SessionEvent::ItemStarted { item } | SessionEvent::ItemUpdated { item }
            if item.text.as_deref() == Some("LATE_OUTPUT"))),
        "prior Turn output became a successor Turn item"
    );
    client.shutdown().await;
}

fn text_block(text: &str) -> ContentBlock {
    serde_json::from_value(serde_json::json!({"type":"text","text":text})).expect("text block")
}

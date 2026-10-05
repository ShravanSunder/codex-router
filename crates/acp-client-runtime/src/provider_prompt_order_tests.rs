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

#[path = "provider_turn_cursor_tests.rs"]
mod provider_turn_cursor_tests;

const CANCELLED_PROMPT_ORDER_FIXTURE: &str = r#"
import json,pathlib,queue,socket,sys,threading
requests=queue.Queue()
prompt_texts=[]
stdin_closed=threading.Event()
receipt_path=pathlib.Path(sys.argv[2])
def receive_wire():
    try:
        for line in sys.stdin:
            message=json.loads(line)
            if message.get('method')=='session/prompt':
                prompt_texts.append(message['params']['prompt'][0]['text'])
                receipt_path.write_text(json.dumps({'promptCount':len(prompt_texts),'promptTexts':prompt_texts}))
            requests.put(message)
    finally:
        stdin_closed.set()
threading.Thread(target=receive_wire,daemon=True).start()
def read(): return requests.get(timeout=5)
def send(value): print(json.dumps(value),flush=True)
def update(text):
    send({'jsonrpc':'2.0','method':'session/update','params':{'sessionId':'fixture-session',
        'update':{'sessionUpdate':'agent_message_chunk','content':{'type':'text','text':text}}}})
control=socket.socket(socket.AF_UNIX,socket.SOCK_STREAM)
control.settimeout(15)
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
assert stdin_closed.wait(timeout=15),'owner did not close fixture stdin'
"#;

const FAILED_PROMPT_ORDER_FIXTURE: &str = r#"
import json,pathlib,queue,socket,sys,threading
requests=queue.Queue()
prompt_texts=[]
receipt_path=pathlib.Path(sys.argv[2])
stdin_closed=threading.Event()
def receive_wire():
    try:
        for line in sys.stdin:
            message=json.loads(line)
            if message.get('method')=='session/prompt':
                prompt_texts.append(message['params']['prompt'][0]['text'])
                receipt_path.write_text(json.dumps({'promptCount':len(prompt_texts),'promptTexts':prompt_texts}))
            requests.put(message)
    finally:
        stdin_closed.set()
def read(): return requests.get(timeout=5)
def send(value): print(json.dumps(value),flush=True)
def update(text):
    send({'jsonrpc':'2.0','method':'session/update','params':{'sessionId':'failed-stream-session',
        'update':{'sessionUpdate':'agent_message_chunk','content':{'type':'text','text':text}}}})
threading.Thread(target=receive_wire,daemon=True).start()
control=socket.socket(socket.AF_UNIX,socket.SOCK_STREAM)
control.settimeout(15)
control.connect(sys.argv[1])
request=read()
assert request['method']=='initialize',request
send({'jsonrpc':'2.0','id':request['id'],'result':{'protocolVersion':1,
    'agentCapabilities':{},'agentInfo':{'name':'failed-stream-order-fixture','version':'1'}}})
request=read()
assert request['method']=='session/new',request
send({'jsonrpc':'2.0','id':request['id'],'result':{'sessionId':'failed-stream-session'}})
first=read()
assert first['method']=='session/prompt' and first['params']['prompt']==[{'type':'text','text':'FIRST_REQUEST'}],first
update('FIRST_PARTIAL')
# The owner test releases this only after the real item emitter publishes it.
assert control.recv(1)==b'e'
send({'jsonrpc':'2.0','id':first['id'],'error':{'code':-32603,'message':'first fixture prompt failed'}})
# Keep the connection alive; the next prompt is an explicit different input.
assert control.recv(1)==b's'
second=read()
assert second['method']=='session/prompt' and second['params']['prompt']==[{'type':'text','text':'SECOND_REQUEST'}],second
update('SECOND_ONLY')
send({'jsonrpc':'2.0','id':second['id'],'result':{'stopReason':'end_turn'}})
control.sendall(b'd')
assert control.recv(1)==b'q'
control.close()
assert stdin_closed.wait(timeout=15),'owner did not close fixture stdin'
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

struct CapturedEventSink(tokio::sync::mpsc::UnboundedSender<SessionEvent>);

impl SessionEventSink for CapturedEventSink {
    fn begin_history_replay(&self, _session_id: &str) -> HistoryReplayFuture<'_> {
        Box::pin(async { Ok(()) })
    }

    fn publish(&self, _session_id: &str, event: SessionEvent) -> Result<(), EventSinkClosed> {
        self.0.send(event).map_err(|_| EventSinkClosed)
    }
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
            let released = self.gate.released.lock().expect("gate lock");
            let (released, _) = self
                .gate
                .release_cv
                .wait_timeout_while(released, std::time::Duration::from_secs(15), |released| {
                    !*released
                })
                .expect("gate lock");
            if !*released {
                return Err(EventSinkClosed);
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
    let root = tempfile::tempdir_in("/tmp").expect("fixture root");
    let control_path = root.path().join("control.sock");
    let receipt_path = root.path().join("wire-prompts.json");
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
                    receipt_path.to_string_lossy().into_owned(),
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
    gate.release();
    let first_result = first.await.expect("first task");
    let next = second
        .await
        .expect("second reply")
        .expect("successor prompt settles");
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
    tokio::time::timeout(std::time::Duration::from_secs(5), client.shutdown())
        .await
        .expect("owned cancellation fixture shutdown");
    let wire: serde_json::Value =
        serde_json::from_slice(&std::fs::read(receipt_path).expect("wire prompt receipt"))
            .expect("wire receipt JSON");
    assert_eq!(acknowledged, *b"r");
    assert!(matches!(
        first_result,
        Err(ExternalProviderRuntimeError::PromptOutputLimitExceeded)
    ));
    assert_eq!(next.output, "CURRENT_OUTPUT");
    assert_eq!(
        wire,
        serde_json::json!({"promptCount":2,"promptTexts":["overflow","next"]})
    );
    let second_start = observed
        .iter()
        .rposition(|event| matches!(event, SessionEvent::TurnStarted { .. }))
        .expect("successor Turn started");
    let late_item = observed[..second_start]
        .iter()
        .find_map(|event| match event {
            SessionEvent::ItemStarted { item } if item.text.as_deref() == Some("LATE_OUTPUT") => {
                Some(item)
            }
            _ => None,
        })
        .expect("actual late item projected before successor admission");
    let successor_items = &observed[second_start..];
    assert!(successor_items.iter().any(|event| matches!(event,
        SessionEvent::ItemStarted { item }
            if item.text.as_deref() == Some("CURRENT_OUTPUT") && item.item_id != late_item.item_id)),
        "successor must start a new CURRENT_OUTPUT item; actual events: {successor_items:?}");
    assert!(!successor_items.iter().any(|event| matches!(event,
        SessionEvent::ItemStarted { item } | SessionEvent::ItemUpdated { item }
            if item.item_id == late_item.item_id || item.text.as_deref() != Some("CURRENT_OUTPUT"))),
        "late item identity or prefix crossed successor admission: {successor_items:?}");
}

fn text_block(text: &str) -> ContentBlock {
    serde_json::from_value(serde_json::json!({"type":"text","text":text})).expect("text block")
}

/// A structured prompt error must not leave its text cursor attached to the
/// next explicitly requested turn, even when neither chunk carries messageId.
#[tokio::test]
async fn failed_prompt_partial_item_is_not_a_successor_turn_item() {
    let root = tempfile::tempdir_in("/tmp").expect("private short-path fixture root");
    let control_path = root.path().join("control.sock");
    let receipt_path = root.path().join("wire-prompts.json");
    let control_listener = tokio::net::UnixListener::bind(&control_path).expect("control socket");
    let (event_sender, mut events) = tokio::sync::mpsc::unbounded_channel();
    let client = Arc::new(
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            AgentSessionClient::initialize(
                ExternalProviderLaunch {
                    executable: PathBuf::from("/usr/bin/python3"),
                    arguments: vec![
                        "-u".into(),
                        "-c".into(),
                        FAILED_PROMPT_ORDER_FIXTURE.into(),
                        control_path.to_string_lossy().into_owned(),
                        receipt_path.to_string_lossy().into_owned(),
                    ],
                    environment: Vec::new(),
                    persistence_target: crate::ProviderPersistenceTarget::Unspecified,
                },
                Arc::new(NoopInteractionPort),
                Arc::new(CapturedEventSink(event_sender)),
            ),
        )
        .await
        .expect("bounded fixture initialize")
        .expect("fixture initializes"),
    );
    let (mut control, _) =
        tokio::time::timeout(std::time::Duration::from_secs(2), control_listener.accept())
            .await
            .expect("bounded fixture control connection")
            .expect("control connection");
    let session_id = client
        .create_session(root.path().to_path_buf())
        .await
        .expect("session opens");
    let first_input = InputId::generate();
    let second_input = InputId::generate();
    let first_client = Arc::clone(&client);
    let first_session = session_id.clone();
    let sent_first_input = first_input.clone();
    let first = tokio::spawn(async move {
        first_client
            .prompt_content(
                first_session,
                sent_first_input,
                Some(101),
                vec![text_block("FIRST_REQUEST")],
                None,
            )
            .await
    });
    let mut observed = Vec::new();
    let first_item = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let event = events.recv().await.expect("item emitter remains open");
            let partial = match &event {
                SessionEvent::ItemStarted { item }
                    if item.text.as_deref() == Some("FIRST_PARTIAL") =>
                {
                    Some(item.clone())
                }
                _ => None,
            };
            observed.push(event);
            if let Some(item) = partial {
                break item;
            }
        }
    })
    .await
    .expect("actual first partial item observed before error release");
    control
        .write_all(b"e")
        .await
        .expect("release structured error");
    let first_result = tokio::time::timeout(std::time::Duration::from_secs(2), first)
        .await
        .expect("first prompt settles")
        .expect("first prompt task joins");
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let event = events.recv().await.expect("terminal emitter remains open");
            let ended = matches!(event, SessionEvent::TurnEnded { .. });
            observed.push(event);
            if ended {
                break;
            }
        }
    })
    .await
    .expect("original failed turn terminal observed");
    let connection_survived_error = !client.retirement().is_cancelled();
    let second_client = Arc::clone(&client);
    let sent_second_input = second_input.clone();
    let second = tokio::spawn(async move {
        second_client
            .prompt_content(
                session_id,
                sent_second_input,
                Some(202),
                vec![text_block("SECOND_REQUEST")],
                None,
            )
            .await
    });
    control
        .write_all(b"s")
        .await
        .expect("release explicit successor");
    let second_result = tokio::time::timeout(std::time::Duration::from_secs(2), second)
        .await
        .expect("successor prompt settles")
        .expect("successor prompt task joins");
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let event = events.recv().await.expect("successor emitter remains open");
            let ended = matches!(event, SessionEvent::TurnEnded { .. });
            observed.push(event);
            if ended {
                break;
            }
        }
    })
    .await
    .expect("successor terminal observed");
    let mut completed = [0u8; 1];
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        control.read_exact(&mut completed),
    )
    .await
    .expect("fixture completed both explicit prompts")
    .expect("fixture completion receipt");
    control
        .write_all(b"q")
        .await
        .expect("release fixture idle phase");
    tokio::time::timeout(std::time::Duration::from_secs(5), client.shutdown())
        .await
        .expect("owned fixture child and actor shutdown");
    let wire: serde_json::Value =
        serde_json::from_slice(&std::fs::read(receipt_path).expect("actual wire prompt receipt"))
            .expect("wire receipt JSON");
    assert_eq!(completed, *b"d");
    assert_ne!(first_input, second_input);
    assert!(
        connection_survived_error,
        "structured error must leave transport alive"
    );
    assert!(matches!(
        first_result,
        Err(ExternalProviderRuntimeError::ProviderRejected { code: -32603, .. })
    ));
    assert!(observed.iter().any(|event| matches!(event, SessionEvent::TurnStarted { input_id, .. } if input_id == &first_input)));
    assert!(observed.iter().any(|event| matches!(
        event,
        SessionEvent::TurnEnded {
            outcome: session_event_model::TurnOutcome::Lost {
                reason: session_event_model::TurnLostReason::ProviderTurnFailed
            },
            ..
        }
    )));
    assert_eq!(
        wire.get("promptCount").and_then(serde_json::Value::as_u64),
        Some(2)
    );
    assert_eq!(
        wire.get("promptTexts"),
        Some(&serde_json::json!(["FIRST_REQUEST", "SECOND_REQUEST"]))
    );
    assert_eq!(
        second_result.expect("successor succeeds").output,
        "SECOND_ONLY"
    );
    let second_start = observed.iter().rposition(|event| matches!(event, SessionEvent::TurnStarted { input_id, .. } if input_id == &second_input)).expect("distinct successor turn started");
    let successor_items = &observed[second_start..];
    assert!(
        !observed.iter().any(|event| matches!(event,
            SessionEvent::ItemCompleted { item_id } if item_id == &first_item.item_id)),
        "failed partial item must not be completed as if its turn succeeded"
    );
    assert!(successor_items.iter().any(|event| matches!(event, SessionEvent::ItemStarted { item } if item.text.as_deref() == Some("SECOND_ONLY") && item.item_id != first_item.item_id)), "successor must emit a new ItemStarted with SECOND_ONLY; actual successor events: {successor_items:?}");
    assert!(!successor_items.iter().any(|event| matches!(event, SessionEvent::ItemStarted { item } | SessionEvent::ItemUpdated { item } if item.item_id == first_item.item_id || item.text.as_deref().is_some_and(|text| text.contains("FIRST_PARTIAL")))), "failed partial or item identity crossed the explicit turn boundary");
}

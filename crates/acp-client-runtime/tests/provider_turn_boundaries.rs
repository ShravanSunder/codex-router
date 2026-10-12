//! Failed ACP turns keep their emitted items separate from explicit successors.
#![allow(clippy::expect_used)]

use acp_client_runtime::{
    AgentSessionClient, ApprovalPortOutcome, EventSinkClosed, ExternalProviderLaunch,
    ExternalProviderRuntimeError, HistoryReplayFuture, InteractionFuture, InteractionPort,
    RefusedApprovalOffer, SessionEventSink,
};
use session_event_model::{ApprovalRequest, InputId, PromptContent, SessionEvent};
use std::{path::PathBuf, sync::Arc};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

#[path = "provider_turn_boundaries/failed_tool_plan.rs"]
mod failed_tool_plan;

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
    type Context = u64;
    type OperationId = u64;
    fn operation_id(context: &Self::Context) -> Self::OperationId {
        *context
    }
    fn binding_retirement(_context: &Self::Context) -> CancellationToken {
        CancellationToken::new()
    }
    fn request_approval(
        &self,
        _context: u64,
        _request: ApprovalRequest,
        _turn_cancellation: CancellationToken,
        _agent_cancellation: CancellationToken,
    ) -> InteractionFuture<'_, ApprovalPortOutcome> {
        Box::pin(async { ApprovalPortOutcome::Cancelled })
    }
    fn request_question(
        &self,
        _context: u64,
        _request: session_event_model::QuestionRequest,
        _turn_cancellation: CancellationToken,
        _agent_cancellation: CancellationToken,
    ) -> InteractionFuture<'_, session_event_model::QuestionResponse> {
        Box::pin(async { session_event_model::QuestionResponse::Cancelled })
    }
    fn record_refusal(
        &self,
        _context: u64,
        _refusal: RefusedApprovalOffer,
    ) -> InteractionFuture<'_, ()> {
        Box::pin(async {})
    }
    fn cancel_all(&self, _context: u64, _reason: &'static str) -> InteractionFuture<'_, ()> {
        Box::pin(async {})
    }
    fn cancel_retired(&self) -> InteractionFuture<'_, ()> {
        Box::pin(async {})
    }
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
fn text_prompt_content(text: &str) -> PromptContent {
    PromptContent::text(text.to_owned()).expect("text prompt")
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
                    persistence_target: acp_client_runtime::ProviderPersistenceTarget::Unspecified,
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
            .prompt_contents_with_approval_dispatch_for_input(
                first_session,
                sent_first_input,
                vec![text_prompt_content("FIRST_REQUEST")],
                101,
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
            .prompt_contents_with_approval_dispatch_for_input(
                session_id,
                sent_second_input,
                vec![text_prompt_content("SECOND_REQUEST")],
                202,
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

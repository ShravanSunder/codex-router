//! Provider Input and Turn identity at the scripted ACP boundary.

#![allow(clippy::expect_used)]

use std::{path::PathBuf, sync::Arc, time::Duration};

use acp_client_runtime::{
    AgentSessionClient, ApprovalPortOutcome, EventSinkOverflow, ExternalProviderLaunch,
    HistoryReplayFuture, InteractionFuture, InteractionPort, ProviderPersistenceTarget,
    RefusedApprovalOffer, SessionEventSink,
};
use session_event_model::{ApprovalRequest, InputId, SessionEvent};
use tokio_util::sync::CancellationToken;

const INJECTED_STEER_FIXTURE: &str = r#"
import json,sys
def read(): return json.loads(sys.stdin.readline())
def send(value): print(json.dumps(value),flush=True)
request=read()
assert request['method']=='initialize'
send({'jsonrpc':'2.0','id':request['id'],'result':{
    'protocolVersion':1,'agentCapabilities':{},
    'agentInfo':{'name':'input-fixture','version':'1'},
    '_meta':{'steering':{'supported':True}}}})
request=read()
assert request['method']=='session/new'
send({'jsonrpc':'2.0','id':request['id'],'result':{'sessionId':'fixture-session'}})
prompt=read()
assert prompt['method']=='session/prompt'
steer=read()
assert steer['method']=='_session/steering'
send({'jsonrpc':'2.0','id':steer['id'],'result':{'outcome':'injected'}})
send({'jsonrpc':'2.0','id':prompt['id'],'result':{'stopReason':'end_turn'}})
sys.stdin.read()
"#;

const STARTED_STEER_FIXTURE: &str = r#"
import json,sys
def read(): return json.loads(sys.stdin.readline())
def send(value): print(json.dumps(value),flush=True)
request=read()
assert request['method']=='initialize'
send({'jsonrpc':'2.0','id':request['id'],'result':{
    'protocolVersion':1,'agentCapabilities':{},
    'agentInfo':{'name':'started-steer-fixture','version':'1'},
    '_meta':{'steering':{'supported':True}}}})
request=read()
assert request['method']=='session/new'
send({'jsonrpc':'2.0','id':request['id'],'result':{'sessionId':'fixture-session'}})
steer=read()
assert steer['method']=='_session/steering'
send({'jsonrpc':'2.0','id':steer['id'],'result':{'outcome':'startedNewTurn'}})
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
        _context: Self::Context,
        _request: ApprovalRequest,
        _turn_cancellation: CancellationToken,
        _agent_cancellation: CancellationToken,
    ) -> InteractionFuture<'_, ApprovalPortOutcome> {
        Box::pin(async { ApprovalPortOutcome::Cancelled })
    }
    fn request_question(
        &self,
        _context: Self::Context,
        _request: session_event_model::QuestionRequest,
        _turn_cancellation: CancellationToken,
        _agent_cancellation: CancellationToken,
    ) -> InteractionFuture<'_, session_event_model::QuestionResponse> {
        Box::pin(async { session_event_model::QuestionResponse::Cancelled })
    }
    fn record_refusal(
        &self,
        _context: Self::Context,
        _refusal: RefusedApprovalOffer,
    ) -> InteractionFuture<'_, ()> {
        Box::pin(async {})
    }
    fn cancel_all(
        &self,
        _context: Self::Context,
        _reason: &'static str,
    ) -> InteractionFuture<'_, ()> {
        Box::pin(async {})
    }
    fn cancel_retired(&self) -> InteractionFuture<'_, ()> {
        Box::pin(async {})
    }
}

struct CaptureEventSink(tokio::sync::mpsc::UnboundedSender<SessionEvent>);
impl SessionEventSink for CaptureEventSink {
    fn begin_history_replay(&self, _session_id: &str) -> HistoryReplayFuture<'_> {
        Box::pin(async { Ok(()) })
    }
    fn publish(&self, _session_id: &str, event: SessionEvent) -> Result<(), EventSinkOverflow> {
        self.0.send(event).map_err(|_| EventSinkOverflow)
    }
}

/// Oracle: E5 gives each Input its own identity; a steer injected into a
/// running Turn links to that Turn without starting another one.
#[tokio::test]
async fn prompt_and_injected_steer_publish_distinct_inputs_on_one_turn() {
    let root = tempfile::tempdir().expect("fixture root");
    let (sender, mut events) = tokio::sync::mpsc::unbounded_channel();
    let client = AgentSessionClient::initialize(
        ExternalProviderLaunch {
            executable: PathBuf::from("python3"),
            arguments: vec![
                "-u".to_owned(),
                "-c".to_owned(),
                INJECTED_STEER_FIXTURE.to_owned(),
            ],
            environment: Vec::new(),
            persistence_target: ProviderPersistenceTarget::Unspecified,
        },
        Arc::new(NoopInteractionPort),
        Arc::new(CaptureEventSink(sender)),
    )
    .await
    .expect("fixture initializes");
    let session_id = client
        .create_session(root.path().to_path_buf())
        .await
        .expect("session opens");
    let prompt_input = InputId::new("prompt-input").expect("input ID");
    let steer_input = InputId::new("steer-input").expect("input ID");
    let prompt =
        client.prompt_with_input(session_id.clone(), prompt_input.clone(), "First".to_owned());
    tokio::pin!(prompt);
    let turn_started = tokio::select! {
        event = tokio::time::timeout(Duration::from_secs(2), events.recv()) => event.expect("turn starts").expect("event"),
        result = &mut prompt => panic!("prompt settled before turn began: {result:?}"),
    };
    let SessionEvent::TurnStarted { turn_id, input_id } = turn_started else {
        panic!("expected TurnStarted, got {turn_started:?}");
    };
    assert_eq!(input_id, prompt_input);
    assert!(
        uuid::Uuid::parse_str(&turn_id)
            .expect("UUID turn ID")
            .get_version_num()
            == 7
    );

    let steering = client
        .steer_with_input(session_id.clone(), steer_input.clone(), "Second".to_owned())
        .await
        .expect("steer accepted");
    assert!(matches!(
        steering,
        acp_client_runtime::ProviderSteeringOutcome::Injected { .. }
    ));
    let accepted = tokio::time::timeout(Duration::from_secs(2), events.recv())
        .await
        .expect("input accepted")
        .expect("event");
    assert_eq!(
        accepted,
        SessionEvent::InputAccepted {
            input_id: steer_input,
            turn_id: turn_id.clone()
        }
    );
    prompt.await.expect("prompt ends");
    let ended = tokio::time::timeout(Duration::from_secs(2), events.recv())
        .await
        .expect("turn ends")
        .expect("event");
    assert!(
        matches!(ended, SessionEvent::TurnEnded { turn_id: ended_turn_id, .. } if ended_turn_id == turn_id)
    );
    client.shutdown().await;
}

/// Oracle: R10 keeps an agent's startedNewTurn outcome distinct from an
/// injected steer even when Router requested promptRequired idle behavior.
#[tokio::test]
async fn started_new_turn_uses_the_steer_input_id() {
    let root = tempfile::tempdir().expect("fixture root");
    let (sender, mut events) = tokio::sync::mpsc::unbounded_channel();
    let client = AgentSessionClient::initialize(
        ExternalProviderLaunch {
            executable: PathBuf::from("python3"),
            arguments: vec![
                "-u".to_owned(),
                "-c".to_owned(),
                STARTED_STEER_FIXTURE.to_owned(),
            ],
            environment: Vec::new(),
            persistence_target: ProviderPersistenceTarget::Unspecified,
        },
        Arc::new(NoopInteractionPort),
        Arc::new(CaptureEventSink(sender)),
    )
    .await
    .expect("fixture initializes");
    let session_id = client
        .create_session(root.path().to_path_buf())
        .await
        .expect("session opens");
    let input_id = InputId::new("new-turn-steer").expect("input ID");
    let steering = client
        .steer_with_input(session_id, input_id.clone(), "Begin".to_owned())
        .await
        .expect("steer accepted");
    assert_eq!(
        steering,
        acp_client_runtime::ProviderSteeringOutcome::StartedNewTurn
    );
    let event = tokio::time::timeout(Duration::from_secs(2), events.recv())
        .await
        .expect("turn starts")
        .expect("event");
    let SessionEvent::TurnStarted {
        turn_id,
        input_id: observed_input,
    } = event
    else {
        panic!("expected TurnStarted, got {event:?}");
    };
    assert_eq!(observed_input, input_id);
    assert_eq!(
        uuid::Uuid::parse_str(&turn_id)
            .expect("UUID turn ID")
            .get_version_num(),
        7
    );
    client.shutdown().await;
}

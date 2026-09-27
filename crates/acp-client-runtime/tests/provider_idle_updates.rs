//! Session updates keep their identity and settings outside an active prompt.

#![allow(clippy::expect_used)]

use std::{path::PathBuf, sync::Arc, time::Duration};

use acp_client_runtime::{
    AgentSessionClient, ApprovalPortOutcome, EventSinkOverflow, ExternalProviderLaunch,
    HistoryReplayFuture, InteractionFuture, InteractionPort, ProviderPersistenceTarget,
    RefusedApprovalOffer, SessionEventSink,
};
use session_event_model::{ApprovalRequest, SessionEvent, SessionItemKind, ToolCallStatus};
use tokio_util::sync::CancellationToken;

const IDLE_UPDATE_FIXTURE: &str = r#"
import json,sys
def read(): return json.loads(sys.stdin.readline())
def send(value): print(json.dumps(value),flush=True)
def update(value):
    send({'jsonrpc':'2.0','method':'session/update',
          'params':{'sessionId':'fixture-session','update':value}})
request=read()
assert request['method']=='initialize'
send({'jsonrpc':'2.0','id':request['id'],'result':{'protocolVersion':1,
    'agentCapabilities':{},'agentInfo':{'name':'idle-update-fixture','version':'1'},
    '_meta':{'steering':{'supported':True}}}})
request=read()
assert request['method']=='session/new'
send({'jsonrpc':'2.0','id':request['id'],'result':{'sessionId':'fixture-session',
    'modes':{'currentModeId':'auto','availableModes':[
        {'id':'auto','name':'Auto'},{'id':'ask','name':'Ask'}]}}})
update({'sessionUpdate':'current_mode_update','currentModeId':'ask'})
update({'sessionUpdate':'config_option_update','configOptions':[
    {'id':'model','name':'Model','category':'model','type':'select','currentValue':'b',
     'options':[{'value':'a','name':'A'},{'value':'b','name':'B'}]}]})
update({'sessionUpdate':'future_idle_kind','content':{'type':'text','text':'Future detail'}})
prompt=read()
assert prompt['method']=='session/prompt',prompt
update({'sessionUpdate':'tool_call','toolCallId':'late-tool',
        'title':'Run check','kind':'execute','status':'pending'})
send({'jsonrpc':'2.0','id':prompt['id'],'result':{'stopReason':'end_turn'}})
steer=read()
assert steer['method']=='_session/steering',steer
update({'sessionUpdate':'tool_call_update','toolCallId':'late-tool',
        'title':'Check passed','status':'completed'})
send({'jsonrpc':'2.0','id':steer['id'],'result':{'outcome':'promptRequired'}})
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

/// R4, R5 and R15: idle changes update inspect, unknown updates become Items,
/// and a late tool update retains its Session Item after TurnEnded.
#[tokio::test]
async fn idle_mode_config_unknown_and_late_tool_updates_are_projected() {
    let root = tempfile::tempdir().expect("fixture root");
    let (sender, mut events) = tokio::sync::mpsc::unbounded_channel();
    let client = AgentSessionClient::initialize(
        ExternalProviderLaunch {
            executable: PathBuf::from("python3"),
            arguments: vec![
                "-u".to_owned(),
                "-c".to_owned(),
                IDLE_UPDATE_FIXTURE.to_owned(),
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

    let initial_events = tokio::time::timeout(Duration::from_secs(2), async {
        let mut observed = Vec::new();
        while observed.len() < 6 {
            observed.push(events.recv().await.expect("event sink remains open"));
        }
        observed
    })
    .await
    .expect("three idle updates are projected");
    assert!(initial_events.iter().any(|event| matches!(event,
        SessionEvent::ItemStarted { item } if item.kind == SessionItemKind::ModeChange)));
    assert!(initial_events.iter().any(|event| matches!(event,
        SessionEvent::ItemStarted { item } if item.kind == SessionItemKind::ConfigChange)));
    assert!(initial_events.iter().any(|event| matches!(event,
        SessionEvent::ItemStarted { item } if matches!(&item.kind,
            SessionItemKind::Unknown { source_kind } if source_kind == "future_idle_kind"))));
    let catalog = client
        .settings_catalog(&session_id)
        .await
        .expect("catalog remains available");
    assert_eq!(catalog.effective_settings().mode.as_deref(), Some("ask"));
    assert_eq!(catalog.effective_settings().model.as_deref(), Some("b"));

    client
        .prompt(session_id.clone(), "Run".to_owned())
        .await
        .expect("prompt ends");
    let steering = client
        .steer_with_input(
            session_id,
            session_event_model::InputId::generate(),
            "Continue".to_owned(),
        )
        .await
        .expect("idle steer");
    assert_eq!(
        steering,
        acp_client_runtime::ProviderSteeringOutcome::PromptRequired
    );
    let later_events = tokio::time::timeout(Duration::from_secs(2), async {
        let mut observed = Vec::new();
        loop {
            let event = events.recv().await.expect("event sink remains open");
            let late_update = matches!(&event, SessionEvent::ItemUpdated { item }
                if item.item_id == "late-tool" && matches!(&item.kind,
                    SessionItemKind::ToolCall { status: ToolCallStatus::Completed, .. }));
            observed.push(event);
            if late_update {
                break observed;
            }
        }
    })
    .await
    .expect("late tool update is projected");
    let end_index = later_events
        .iter()
        .position(|event| matches!(event, SessionEvent::TurnEnded { .. }))
        .expect("Turn ended before late update");
    assert!(end_index + 1 < later_events.len());
    client.shutdown().await;
}

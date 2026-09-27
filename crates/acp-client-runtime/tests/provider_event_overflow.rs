//! Output byte limits and event-consumer shutdown have different outcomes.

#![allow(clippy::expect_used)]

use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use acp_client_runtime::{
    AgentSessionClient, ApprovalPortOutcome, EventSinkClosed, ExternalProviderLaunch,
    ExternalProviderRuntimeError, HistoryReplayFuture, InteractionFuture, InteractionPort,
    ProviderPersistenceTarget, RefusedApprovalOffer, SessionEventSink,
};
use session_event_model::{ApprovalRequest, LocalCause, SessionEvent, StopReason, TurnOutcome};
use tokio_util::sync::CancellationToken;

const OUTPUT_LIMIT_FIXTURE: &str = r#"
import json,sys
def read(): return json.loads(sys.stdin.readline())
def send(value): print(json.dumps(value),flush=True)
request=read()
assert request['method']=='initialize'
send({'jsonrpc':'2.0','id':request['id'],'result':{'protocolVersion':1,'agentCapabilities':{},
    'agentInfo':{'name':'output-limit-fixture','version':'1'}}})
request=read()
assert request['method']=='session/new'
send({'jsonrpc':'2.0','id':request['id'],'result':{'sessionId':'fixture-session'}})
prompt=read()
assert prompt['method']=='session/prompt'
send({'jsonrpc':'2.0','method':'session/update','params':{'sessionId':'fixture-session',
    'update':{'sessionUpdate':'agent_message_chunk',
              'content':{'type':'text','text':'x'*1048577}}}})
cancel=read()
assert cancel['method']=='session/cancel',cancel
send({'jsonrpc':'2.0','id':prompt['id'],'result':{'stopReason':'end_turn'}})
sys.stdin.read()
"#;

const CLOSED_SINK_FIXTURE: &str = r#"
import json,sys
def read(): return json.loads(sys.stdin.readline())
def send(value): print(json.dumps(value),flush=True)
request=read()
assert request['method']=='initialize'
send({'jsonrpc':'2.0','id':request['id'],'result':{'protocolVersion':1,'agentCapabilities':{},
    'agentInfo':{'name':'closed-sink-fixture','version':'1'}}})
request=read()
assert request['method']=='session/new'
send({'jsonrpc':'2.0','id':request['id'],'result':{'sessionId':'fixture-session'}})
prompt=read()
assert prompt['method']=='session/prompt'
send({'jsonrpc':'2.0','method':'session/update','params':{'sessionId':'fixture-session',
    'update':{'sessionUpdate':'agent_message_chunk',
              'content':{'type':'text','text':'Chunk'}}}})
sys.stdin.read()
"#;

const CLOSED_AUTH_SINK_FIXTURE: &str = r#"
import json,sys
def read(): return json.loads(sys.stdin.readline())
def send(value): print(json.dumps(value),flush=True)
request=read()
assert request['method']=='initialize'
send({'jsonrpc':'2.0','id':request['id'],'result':{'protocolVersion':1,'agentCapabilities':{},
    'agentInfo':{'name':'closed-auth-sink-fixture','version':'1'}}})
request=read()
assert request['method']=='session/new'
send({'jsonrpc':'2.0','id':request['id'],'result':{'sessionId':'fixture-session'}})
prompt=read()
assert prompt['method']=='session/prompt'
send({'jsonrpc':'2.0','method':'_auth/status_update',
    'params':{'authStatus':{'kind':'none','label':'Signed out'}}})
sys.stdin.read()
"#;

const LOST_PROVIDER_FIXTURE: &str = r#"
import json,os,sys
def read(): return json.loads(sys.stdin.readline())
def send(value): print(json.dumps(value),flush=True)
request=read()
assert request['method']=='initialize'
send({'jsonrpc':'2.0','id':request['id'],'result':{'protocolVersion':1,'agentCapabilities':{},
    'agentInfo':{'name':'lost-fixture','version':'1'}}})
request=read()
assert request['method']=='session/new'
send({'jsonrpc':'2.0','id':request['id'],'result':{'sessionId':'fixture-session'}})
prompt=read()
assert prompt['method']=='session/prompt'
os.close(1)
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

#[derive(Default)]
struct CaptureEventSink {
    events: Mutex<Vec<SessionEvent>>,
    reject_items: bool,
    reject_capabilities: bool,
    closed: AtomicBool,
}
impl SessionEventSink for CaptureEventSink {
    fn begin_history_replay(&self, _session_id: &str) -> HistoryReplayFuture<'_> {
        Box::pin(async { Ok(()) })
    }
    fn publish(&self, _session_id: &str, event: SessionEvent) -> Result<(), EventSinkClosed> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(EventSinkClosed);
        }
        if (self.reject_items && matches!(event, SessionEvent::ItemStarted { .. }))
            || (self.reject_capabilities
                && matches!(event, SessionEvent::CapabilitiesChanged { .. }))
        {
            self.closed.store(true, Ordering::SeqCst);
            return Err(EventSinkClosed);
        }
        self.events.lock().expect("event sink").push(event);
        Ok(())
    }
}

async fn run_fixture(
    fixture: &str,
    sink: Arc<CaptureEventSink>,
) -> (
    Result<acp_client_runtime::ExternalProviderPromptOutcome, ExternalProviderRuntimeError>,
    Arc<CaptureEventSink>,
) {
    let root = tempfile::tempdir().expect("fixture root");
    let client = AgentSessionClient::initialize(
        ExternalProviderLaunch {
            executable: PathBuf::from("python3"),
            arguments: vec!["-u".to_owned(), "-c".to_owned(), fixture.to_owned()],
            environment: Vec::new(),
            persistence_target: ProviderPersistenceTarget::Unspecified,
        },
        Arc::new(NoopInteractionPort),
        Arc::clone(&sink) as Arc<dyn SessionEventSink>,
    )
    .await
    .expect("fixture initializes");
    let session_id = client
        .create_session(root.path().to_path_buf())
        .await
        .expect("session opens");
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        client.prompt_contents_with_approval_dispatch_for_input(
            session_id,
            session_event_model::InputId::generate(),
            vec![session_event_model::PromptContent::text("Run".to_owned()).expect("text prompt")],
            (),
            None,
        ),
    )
    .await
    .expect("turn settles");
    client.shutdown().await;
    (result, sink)
}

/// R5: a bounded output byte limit cancels the Turn, then keeps the agent's
/// confirmed stop reason alongside Router's separate local cause.
#[tokio::test]
async fn output_byte_limit_cancels_turn_and_keeps_agent_stop_reason() {
    let (result, sink) =
        run_fixture(OUTPUT_LIMIT_FIXTURE, Arc::new(CaptureEventSink::default())).await;
    assert!(matches!(
        result,
        Err(ExternalProviderRuntimeError::PromptOutputLimitExceeded)
    ));
    let events = sink.events.lock().expect("event sink");
    assert!(matches!(
        events.first(),
        Some(SessionEvent::SettingsChanged { .. })
    ));
    assert!(
        events
            .iter()
            .any(|event| matches!(event, SessionEvent::TurnStarted { .. }))
    );
    assert!(events.iter().any(|event| matches!(
        event,
        SessionEvent::TurnEnded {
            outcome: TurnOutcome::Ended {
                stop_reason: StopReason::EndTurn,
                local_cause: Some(LocalCause::OutputOverflow),
            },
            ..
        }
    )));
}

/// A closed event consumer is a typed shutdown, without inventing an agent
/// stop reason or treating it as the prompt byte limit.
#[tokio::test]
async fn closed_event_consumer_ends_active_turn_with_sink_closed() {
    let (result, sink) = run_fixture(
        CLOSED_SINK_FIXTURE,
        Arc::new(CaptureEventSink {
            reject_items: true,
            ..CaptureEventSink::default()
        }),
    )
    .await;
    assert!(matches!(
        result,
        Err(ExternalProviderRuntimeError::SinkClosed)
    ));
    let events = sink.events.lock().expect("event sink");
    assert!(matches!(
        events.first(),
        Some(SessionEvent::SettingsChanged { .. })
    ));
    assert!(
        events
            .iter()
            .any(|event| matches!(event, SessionEvent::TurnStarted { .. }))
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, SessionEvent::TurnEnded { .. }))
    );
}

/// A connection-scoped callback also retires the active prompt with the same
/// typed shutdown when its consumer closes.
#[tokio::test]
async fn closed_auth_event_consumer_retires_active_prompt() {
    let (result, sink) = run_fixture(
        CLOSED_AUTH_SINK_FIXTURE,
        Arc::new(CaptureEventSink {
            reject_capabilities: true,
            ..CaptureEventSink::default()
        }),
    )
    .await;
    assert!(
        matches!(result, Err(ExternalProviderRuntimeError::SinkClosed)),
        "result: {result:?}"
    );
    let events = sink.events.lock().expect("event sink");
    assert!(matches!(
        events.first(),
        Some(SessionEvent::SettingsChanged { .. })
    ));
    assert!(
        events
            .iter()
            .any(|event| matches!(event, SessionEvent::TurnStarted { .. }))
    );
}

/// A provider that closes stdout before its prompt result cannot confirm a
/// stop reason; the Turn is lost with the retirement reason.
#[tokio::test]
async fn provider_eof_ends_running_turn_lost() {
    let (result, sink) =
        run_fixture(LOST_PROVIDER_FIXTURE, Arc::new(CaptureEventSink::default())).await;
    assert!(result.is_err());
    let events = sink.events.lock().expect("event sink");
    assert!(matches!(
        events.first(),
        Some(SessionEvent::SettingsChanged { .. })
    ));
    assert!(
        events
            .iter()
            .any(|event| matches!(event, SessionEvent::TurnStarted { .. }))
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, SessionEvent::TurnEnded {
        outcome: TurnOutcome::Lost { reason }, ..
    } if reason == &session_event_model::TurnLostReason::ProviderRetired))
    );
    assert!(matches!(
        events.last(),
        Some(SessionEvent::StateChanged {
            state: session_event_model::SessionState::Unloaded
        })
    ));
}

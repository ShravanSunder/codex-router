//! A subprocess checks the ACP wire order around history replay admission.

#![allow(clippy::expect_used)]

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use acp_client_runtime::{
    AgentSessionClient, ApprovalPortOutcome, EventSinkOverflow, ExternalProviderLaunch,
    ExternalProviderRuntimeError, HistoryReplayFuture, HistoryReplayUnavailable, InteractionFuture,
    InteractionPort, RefusedApprovalOffer, SessionEventSink,
};
use session_event_model::{ApprovalRequest, SessionEvent};
use tokio_util::sync::CancellationToken;

const REPLAY_FIXTURE: &str = r#"
import json, os, sys
marker = sys.argv[1]
request = json.loads(sys.stdin.readline())
assert request['method'] == 'initialize'
print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':{'protocolVersion':1,'agentCapabilities':{'loadSession':True},'agentInfo':{'name':'replay-fixture','version':'1'}}}), flush=True)
line = sys.stdin.readline()
if line:
    request = json.loads(line)
    assert request['method'] == 'session/load'
    if not os.path.exists(marker):
        with open(marker + '.early', 'w') as receipt:
            receipt.write('load preceded replay reset')
    print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':{
        'configOptions':[{'id':'model','name':'Model','type':'select',
                          'currentValue':'model-a',
                          'options':[{'value':'model-a','name':'Model A'}]}]}}), flush=True)
sys.stdin.read()
"#;

const REPLAY_ITEMS_FIXTURE: &str = r#"
import json,sys
def read(): return json.loads(sys.stdin.readline())
def send(value): print(json.dumps(value),flush=True)
request=read()
assert request['method']=='initialize'
send({'jsonrpc':'2.0','id':request['id'],'result':{'protocolVersion':1,
    'agentCapabilities':{'loadSession':True},
    'agentInfo':{'name':'replay-items-fixture','version':'1'}}})
request=read()
assert request['method']=='session/load'
for kind,text in [('user_message_chunk','First user'),('agent_message_chunk','First answer'),
                  ('user_message_chunk','Second user'),('agent_message_chunk','Second answer')]:
    send({'jsonrpc':'2.0','method':'session/update','params':{'sessionId':'fixture-session',
        'update':{'sessionUpdate':kind,'content':{'type':'text','text':text}}}})
send({'jsonrpc':'2.0','id':request['id'],'result':{}})
sys.stdin.read()
"#;

struct TestInteractionPort;

impl InteractionPort for TestInteractionPort {
    type Context = ();
    type OperationId = u64;

    fn operation_id(_context: &Self::Context) -> Self::OperationId {
        0
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

struct ReplaySink {
    marker: PathBuf,
    fail: bool,
    events: Option<Arc<Mutex<Vec<SessionEvent>>>>,
}

impl SessionEventSink for ReplaySink {
    fn begin_history_replay(&self, _session_id: &str) -> HistoryReplayFuture<'_> {
        Box::pin(async move {
            if self.fail {
                return Err(HistoryReplayUnavailable);
            }
            std::fs::write(&self.marker, b"replay reset").map_err(|_| HistoryReplayUnavailable)
        })
    }

    fn publish(&self, _session_id: &str, event: SessionEvent) -> Result<(), EventSinkOverflow> {
        if let Some(events) = &self.events {
            events.lock().expect("replay events").push(event);
        }
        Ok(())
    }
}

fn fixture_launch(marker: &std::path::Path) -> ExternalProviderLaunch {
    ExternalProviderLaunch {
        executable: PathBuf::from("python3"),
        arguments: vec![
            "-u".to_owned(),
            "-c".to_owned(),
            REPLAY_FIXTURE.to_owned(),
            marker.to_string_lossy().into_owned(),
        ],
        environment: Vec::new(),
        persistence_target: acp_client_runtime::ProviderPersistenceTarget::Unspecified,
    }
}

/// Oracle: ACP v1 session/load replays updates before its response
/// (docs/protocol/v1/session-setup.mdx:150-211); program design requires the
/// hub reset to finish before the request can be sent.
#[tokio::test]
async fn history_replay_reset_precedes_session_load() {
    let root = tempfile::tempdir().expect("fixture root");
    let marker = root.path().join("reset-finished");
    let client = AgentSessionClient::initialize(
        fixture_launch(&marker),
        Arc::new(TestInteractionPort),
        Arc::new(ReplaySink {
            marker: marker.clone(),
            fail: false,
            events: None,
        }),
    )
    .await
    .expect("fixture initializes");

    let result = client
        .load_session("fixture-session".to_owned(), root.path().to_path_buf())
        .await;
    let catalog = client
        .settings_catalog("fixture-session")
        .await
        .expect("loaded session catalog");
    let last_catalog = client.last_settings_catalog().await.expect("last catalog");
    client.shutdown().await;

    assert!(result.is_ok(), "load result: {result:?}");
    assert!(marker.exists(), "reset completed");
    assert_eq!(
        catalog.effective_settings().model.as_deref(),
        Some("model-a")
    );
    assert_eq!(last_catalog, catalog);
    assert!(
        !marker.with_extension("early").exists(),
        "load waited for reset"
    );
}

/// Oracle: program design AgentSessionClient.attach requires a failed hub reset
/// to abort the load, so the agent cannot replay into stale history.
#[tokio::test]
async fn failed_history_replay_never_sends_session_load() {
    let root = tempfile::tempdir().expect("fixture root");
    let marker = root.path().join("reset-finished");
    let client = AgentSessionClient::initialize(
        fixture_launch(&marker),
        Arc::new(TestInteractionPort),
        Arc::new(ReplaySink {
            marker: marker.clone(),
            fail: true,
            events: None,
        }),
    )
    .await
    .expect("fixture initializes");

    let result = client
        .load_session("fixture-session".to_owned(), root.path().to_path_buf())
        .await;
    client.shutdown().await;

    assert!(matches!(
        result,
        Err(ExternalProviderRuntimeError::HistoryReplayUnavailable)
    ));
    assert!(!marker.with_extension("early").exists(), "no ACP load sent");
}

/// Oracle: E4/R25 rebuild one historical Turn per replayed user message,
/// with unknown(replayed) instead of inventing an original stop reason.
#[tokio::test]
async fn session_load_publishes_two_historical_turns_and_items() {
    let root = tempfile::tempdir().expect("fixture root");
    let marker = root.path().join("reset-finished");
    let events = Arc::new(Mutex::new(Vec::new()));
    let client = AgentSessionClient::initialize(
        ExternalProviderLaunch {
            executable: PathBuf::from("python3"),
            arguments: vec![
                "-u".to_owned(),
                "-c".to_owned(),
                REPLAY_ITEMS_FIXTURE.to_owned(),
            ],
            environment: Vec::new(),
            persistence_target: acp_client_runtime::ProviderPersistenceTarget::Unspecified,
        },
        Arc::new(TestInteractionPort),
        Arc::new(ReplaySink {
            marker,
            fail: false,
            events: Some(Arc::clone(&events)),
        }),
    )
    .await
    .expect("fixture initializes");
    client
        .load_session("fixture-session".to_owned(), root.path().to_path_buf())
        .await
        .expect("load");
    client.shutdown().await;
    let events = events.lock().expect("replay events");
    let starts = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::TurnStarted { turn_id, input_id } => Some((turn_id, input_id)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(starts.len(), 2, "replayed events: {events:?}");
    assert_ne!(starts[0].0, starts[1].0);
    assert_ne!(starts[0].1, starts[1].1);
    let ends = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::TurnEnded { turn_id, outcome } => Some((turn_id, outcome)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(ends.len(), 2, "replayed events: {events:?}");
    for ((started_id, _), (ended_id, outcome)) in starts.iter().zip(ends) {
        assert_eq!(*started_id, ended_id);
        assert_eq!(
            *outcome,
            session_event_model::TurnOutcome::Ended {
                stop_reason: session_event_model::StopReason::Unknown("replayed".to_owned()),
                local_cause: None,
            }
        );
    }
    let texts = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::ItemStarted { item } => item.text.as_deref(),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        texts,
        ["First user", "First answer", "Second user", "Second answer"]
    );
}

//! A subprocess checks the ACP wire order around history replay admission.

#![allow(clippy::expect_used)]

use std::{path::PathBuf, sync::Arc};

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

    fn publish(&self, _session_id: &str, _event: SessionEvent) -> Result<(), EventSinkOverflow> {
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

//! Scripted ACP permission exchanges over a real stdio subprocess.

#![allow(clippy::expect_used)]

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use acp_client_runtime::{
    AgentSessionClient, ApprovalPortOutcome, EventSinkClosed, ExternalProviderLaunch,
    HistoryReplayFuture, InteractionFuture, InteractionPort, ProviderPersistenceTarget,
    RefusedApprovalOffer, SessionEventSink,
};
use session_event_model::{ApprovalRequest, ApprovalScope, SessionEvent};
use tokio_util::sync::CancellationToken;

const PERMISSION_FIXTURE: &str = r#"
import json,sys
mode,receipt=sys.argv[1:]
def read(): return json.loads(sys.stdin.readline())
def send(value): print(json.dumps(value),flush=True)
request=read()
assert request['method']=='initialize'
send({'jsonrpc':'2.0','id':request['id'],'result':{'protocolVersion':1,'agentCapabilities':{},'agentInfo':{'name':'permission-fixture','version':'1'}}})
request=read()
assert request['method']=='session/new'
send({'jsonrpc':'2.0','id':request['id'],'result':{'sessionId':'fixture-session'}})
prompt=read()
assert prompt['method']=='session/prompt'
options=([{'optionId':'allow-always','name':'Always allow','kind':'allow_always'}]
         if mode=='persistent' else
         [{'optionId':'same','name':'Allow','kind':'allow_once'},
          {'optionId':'same','name':'Reject','kind':'reject_once'}])
send({'jsonrpc':'2.0','id':91,'method':'session/request_permission','params':{
    'sessionId':'fixture-session',
    'toolCall':{'toolCallId':'tool-1','title':'Run command','kind':'execute',
                'rawInput':{'command':'echo safe','api_key':'secret'}},
    'options':options}})
answer=read()
assert answer['id']==91
with open(receipt,'w') as output: json.dump(answer['result'],output)
send({'jsonrpc':'2.0','id':prompt['id'],'result':{'stopReason':'end_turn'}})
sys.stdin.read()
"#;

#[derive(Default)]
struct CaptureInteractionPort {
    accepted: Mutex<Option<ApprovalRequest>>,
    refused: Mutex<Option<RefusedApprovalOffer>>,
}

impl InteractionPort for CaptureInteractionPort {
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
        request: ApprovalRequest,
        _turn_cancellation: CancellationToken,
        _agent_cancellation: CancellationToken,
    ) -> InteractionFuture<'_, ApprovalPortOutcome> {
        Box::pin(async move {
            *self.accepted.lock().expect("capture lock") = Some(request);
            ApprovalPortOutcome::Selected {
                option_id: "allow-always".to_owned(),
                note: None,
            }
        })
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
        refusal: RefusedApprovalOffer,
    ) -> InteractionFuture<'_, ()> {
        Box::pin(async move {
            *self.refused.lock().expect("capture lock") = Some(refusal);
        })
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

struct NoopEventSink;

impl SessionEventSink for NoopEventSink {
    fn begin_history_replay(&self, _session_id: &str) -> HistoryReplayFuture<'_> {
        Box::pin(async { Ok(()) })
    }
    fn publish(&self, _session_id: &str, _event: SessionEvent) -> Result<(), EventSinkClosed> {
        Ok(())
    }
}

async fn run_permission_fixture(mode: &str) -> (Arc<CaptureInteractionPort>, serde_json::Value) {
    let root = tempfile::tempdir().expect("fixture root");
    let receipt = root.path().join("agent-answer.json");
    let port = Arc::new(CaptureInteractionPort::default());
    let client = AgentSessionClient::initialize(
        ExternalProviderLaunch {
            executable: PathBuf::from("python3"),
            arguments: vec![
                "-u".to_owned(),
                "-c".to_owned(),
                PERMISSION_FIXTURE.to_owned(),
                mode.to_owned(),
                receipt.to_string_lossy().into_owned(),
            ],
            environment: Vec::new(),
            persistence_target: ProviderPersistenceTarget::CursorAllowlist,
        },
        Arc::clone(&port),
        Arc::new(NoopEventSink),
    )
    .await
    .expect("fixture initializes");
    client
        .create_session(root.path().to_path_buf())
        .await
        .expect("session opens");
    let result = client
        .prompt_with_approval_context("fixture-session".to_owned(), "Run it".to_owned(), ())
        .await;
    client.shutdown().await;
    assert!(result.is_ok(), "prompt result: {result:?}");
    let answer = serde_json::from_slice(&std::fs::read(receipt).expect("agent answer receipt"))
        .expect("agent answer JSON");
    (port, answer)
}

/// Oracle: specification R17 permits an allow-always-only offer and preserves
/// its exact ACP option ID and persistent destination.
#[tokio::test]
async fn persistent_only_offer_is_selectable_with_exact_agent_id() {
    let (port, answer) = run_permission_fixture("persistent").await;
    let request = port
        .accepted
        .lock()
        .expect("capture lock")
        .clone()
        .expect("request reached port");
    assert!(request.request_id.contains("fixture-session"));
    assert_eq!(request.options.iter().count(), 1);
    let choice = &request.options.iter().next().expect("option").choice;
    let ApprovalScope::Persistent { where_stored } = &choice.scope else {
        panic!("persistent choice must stay persistent");
    };
    assert_eq!(
        where_stored.as_str(),
        "Cursor allowlist (Cursor decides whether global or per-project)"
    );
    assert_eq!(
        answer,
        serde_json::json!({"outcome":{"outcome":"selected","optionId":"allow-always"}})
    );
}

/// Oracle: specification R16 records a malformed refusal and ACP R1 answers
/// the pending permission request with cancelled.
#[tokio::test]
async fn duplicate_ids_are_recorded_as_refusal_and_answered_cancelled() {
    let (port, answer) = run_permission_fixture("duplicate").await;
    assert!(port.accepted.lock().expect("capture lock").is_none());
    let refusal = port
        .refused
        .lock()
        .expect("capture lock")
        .clone()
        .expect("refusal reached port");
    assert_eq!(
        refusal.reason,
        "permission options contain a duplicate identifier"
    );
    assert_eq!(refusal.options.len(), 2);
    assert_eq!(
        answer,
        serde_json::json!({"outcome":{"outcome":"cancelled"}})
    );
}

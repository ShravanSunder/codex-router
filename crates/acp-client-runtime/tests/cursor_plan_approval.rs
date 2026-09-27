//! Cursor create_plan is a blocking Approval with a preceding plan Item.

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
use session_event_model::{
    ApprovalRequest, ApprovalSubject, OptionsOrigin, QuestionRequest, QuestionResponse,
    SessionEvent, SessionItemKind,
};
use tokio_util::sync::CancellationToken;

const CREATE_PLAN_FIXTURE: &str = r#"
import json,sys
receipt=sys.argv[1]
def read(): return json.loads(sys.stdin.readline())
def send(value): print(json.dumps(value),flush=True)
request=read()
assert request['method']=='initialize'
send({'jsonrpc':'2.0','id':request['id'],'result':{'protocolVersion':1,'agentCapabilities':{},
    'agentInfo':{'name':'plan-fixture','version':'1'}}})
request=read()
assert request['method']=='session/new'
send({'jsonrpc':'2.0','id':request['id'],'result':{'sessionId':'fixture-session'}})
prompt=read()
assert prompt['method']=='session/prompt'
send({'jsonrpc':'2.0','method':'session/update','params':{'sessionId':'fixture-session',
    'update':{'sessionUpdate':'tool_call','toolCallId':'plan-tool',
              'title':'Create plan','kind':'other','status':'pending'}}})
send({'jsonrpc':'2.0','id':'create-plan','method':'cursor/create_plan',
    'params':{'toolCallId':'plan-tool','name':'Migration plan','overview':'Move clients safely',
              'plan':'# Steps\n\nShip in slices.',
              'todos':[{'id':'one','content':'Build client','status':'pending'}]}})
response=read()
assert response['id']=='create-plan',response
with open(receipt,'w') as output: json.dump(response,output)
send({'jsonrpc':'2.0','id':prompt['id'],'result':{'stopReason':'end_turn'}})
sys.stdin.read()
"#;

struct PlanPort {
    answer: ApprovalPortOutcome,
    captured: Mutex<Option<ApprovalRequest>>,
    order: Arc<Mutex<Vec<&'static str>>>,
}

impl InteractionPort for PlanPort {
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
        self.order.lock().expect("order").push("approval");
        *self.captured.lock().expect("approval capture") = Some(request);
        let answer = self.answer.clone();
        Box::pin(async move { answer })
    }
    fn request_question(
        &self,
        _context: Self::Context,
        _request: QuestionRequest,
        _turn_cancellation: CancellationToken,
        _agent_cancellation: CancellationToken,
    ) -> InteractionFuture<'_, QuestionResponse> {
        Box::pin(async { QuestionResponse::Cancelled })
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

struct PlanEventSink {
    items: Mutex<Vec<(String, session_event_model::SessionItem)>>,
    order: Arc<Mutex<Vec<&'static str>>>,
}
impl SessionEventSink for PlanEventSink {
    fn begin_history_replay(&self, _session_id: &str) -> HistoryReplayFuture<'_> {
        Box::pin(async { Ok(()) })
    }
    fn publish(&self, session_id: &str, event: SessionEvent) -> Result<(), EventSinkClosed> {
        if let SessionEvent::ItemStarted { item } = event
            && item.kind == SessionItemKind::Plan
        {
            self.order.lock().expect("order").push("planItem");
            self.items
                .lock()
                .expect("items")
                .push((session_id.to_owned(), item));
        }
        Ok(())
    }
}

async fn run_plan_case(
    answer: ApprovalPortOutcome,
) -> (
    serde_json::Value,
    ApprovalRequest,
    Vec<&'static str>,
    Vec<(String, session_event_model::SessionItem)>,
) {
    let root = tempfile::tempdir().expect("fixture root");
    let receipt = root.path().join("create-plan-response.json");
    let order = Arc::new(Mutex::new(Vec::new()));
    let port = Arc::new(PlanPort {
        answer,
        captured: Mutex::new(None),
        order: Arc::clone(&order),
    });
    let sink = Arc::new(PlanEventSink {
        items: Mutex::new(Vec::new()),
        order: Arc::clone(&order),
    });
    let client = AgentSessionClient::initialize(
        ExternalProviderLaunch {
            executable: PathBuf::from("python3"),
            arguments: vec![
                "-u".to_owned(),
                "-c".to_owned(),
                CREATE_PLAN_FIXTURE.to_owned(),
                receipt.to_string_lossy().into_owned(),
            ],
            environment: Vec::new(),
            persistence_target: ProviderPersistenceTarget::Unspecified,
        },
        port.clone(),
        sink.clone(),
    )
    .await
    .expect("fixture initializes");
    let session_id = client
        .create_session(root.path().to_path_buf())
        .await
        .expect("session opens");
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        client.prompt_with_approval_context(session_id, "Plan".to_owned(), ()),
    )
    .await
    .expect("plan exchange completes")
    .expect("prompt ends");
    client.shutdown().await;
    let response =
        serde_json::from_slice(&std::fs::read(receipt).expect("receipt")).expect("response JSON");
    let request = port
        .captured
        .lock()
        .expect("approval capture")
        .clone()
        .expect("approval routed");
    let order = order.lock().expect("order").clone();
    let items = sink.items.lock().expect("items").clone();
    (response, request, order, items)
}

/// Oracle: a plan is visible before the Approver decides; Router-authored
/// option IDs and the plan Item link are explicit in the request.
#[tokio::test]
async fn create_plan_accepts_after_publishing_plan_item() {
    let (response, request, order, items) = run_plan_case(ApprovalPortOutcome::Selected {
        option_id: "plan.accept".to_owned(),
        note: None,
    })
    .await;
    assert_eq!(
        response["result"],
        serde_json::json!({"outcome":"accepted"})
    );
    assert_eq!(order, ["planItem", "approval"]);
    assert_eq!(request.title, "Migration plan");
    assert_eq!(request.options_origin, OptionsOrigin::RouterSynthesized);
    assert_eq!(
        request
            .options
            .iter()
            .map(|option| option.option_id.as_str())
            .collect::<Vec<_>>(),
        ["plan.accept", "plan.reject"]
    );
    assert!(
        matches!(request.subject, Some(ApprovalSubject::Plan { tool_call_id, plan_item_id })
        if tool_call_id == "plan-tool" && plan_item_id == items[0].1.item_id)
    );
    assert_eq!(items[0].0, "fixture-session");
    let text = items[0].1.text.as_deref().expect("plan text");
    for expected in [
        "Migration plan",
        "Move clients safely",
        "Ship in slices.",
        "Build client",
    ] {
        assert!(text.contains(expected), "missing {expected}: {text}");
    }
}

#[tokio::test]
async fn create_plan_rejects_with_approver_note() {
    let (response, _, _, _) = run_plan_case(ApprovalPortOutcome::Selected {
        option_id: "plan.reject".to_owned(),
        note: Some("Revise the rollout".to_owned()),
    })
    .await;
    assert_eq!(
        response["result"],
        serde_json::json!({"outcome":"rejected","reason":"Revise the rollout"})
    );
}

#[tokio::test]
async fn create_plan_cancels_when_turn_is_cancelled() {
    let (response, _, _, _) = run_plan_case(ApprovalPortOutcome::Cancelled).await;
    assert_eq!(
        response["result"],
        serde_json::json!({"outcome":"cancelled"})
    );
}

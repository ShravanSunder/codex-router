//! ACP v1 list, resume, and close through one scripted agent subprocess.

#![allow(clippy::expect_used)]

use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use acp_client_runtime::{
    AgentSessionClient, ApprovalPortOutcome, EventSinkClosed, ExternalProviderLaunch,
    HistoryReplayFuture, InteractionFuture, InteractionPort, ProviderPersistenceTarget,
    ProviderPromptDispatchObservation, RefusedApprovalOffer, SessionEventSink,
};
use session_event_model::{ApprovalRequest, SessionEvent};
use tokio_util::sync::CancellationToken;

const LIFECYCLE_FIXTURE: &str = r#"
import json,sys
mode,cwd,receipt=sys.argv[1:]
seen=[]
def read(): return json.loads(sys.stdin.readline())
def send(value): print(json.dumps(value),flush=True)
request=read()
assert request['method']=='initialize'
send({'jsonrpc':'2.0','id':request['id'],'result':{
    'protocolVersion':1,'agentCapabilities':{'sessionCapabilities':{'list':{},'resume':{},'close':{}}},
    'agentInfo':{'name':'lifecycle-fixture','version':'1'}}})
request=read()
assert request['method']=='session/list',request
assert request['params']['cwd']==cwd,request
seen.append('list')
send({'jsonrpc':'2.0','id':request['id'],'result':{
    'sessions':[{'sessionId':'fixture-session','cwd':cwd,'title':'Fixture session'}]}})
request=read()
assert request['method']=='session/resume',request
seen.append('resume')
send({'jsonrpc':'2.0','id':request['id'],'result':{}})
prompt=read()
assert prompt['method']=='session/prompt',prompt
seen.append('prompt')
if mode=='running-close':
    cancel=read()
    assert cancel['method']=='session/cancel',cancel
    seen.append('cancel')
    send({'jsonrpc':'2.0','id':prompt['id'],'result':{'stopReason':'cancelled'}})
else:
    send({'jsonrpc':'2.0','id':prompt['id'],'result':{'stopReason':'end_turn'}})
close=read()
assert close['method']=='session/close',close
seen.append('close')
with open(receipt,'w') as output: json.dump(seen,output)
send({'jsonrpc':'2.0','id':close['id'],'result':{}})
sys.stdin.read()
"#;

const UNADVERTISED_FIXTURE: &str = r#"
import json,sys
marker=sys.argv[1]
request=json.loads(sys.stdin.readline())
assert request['method']=='initialize'
print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':{
    'protocolVersion':1,'agentCapabilities':{},
    'agentInfo':{'name':'no-lifecycle-fixture','version':'1'}}}),flush=True)
if sys.stdin.readline():
    with open(marker,'w') as output: output.write('unexpected lifecycle request')
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

struct CountingEventSink(AtomicUsize);
impl SessionEventSink for CountingEventSink {
    fn begin_history_replay(&self, _session_id: &str) -> HistoryReplayFuture<'_> {
        self.0.fetch_add(1, Ordering::Relaxed);
        Box::pin(async { Ok(()) })
    }
    fn publish(&self, _session_id: &str, _event: SessionEvent) -> Result<(), EventSinkClosed> {
        Ok(())
    }
}

fn fixture_launch(
    mode: &str,
    cwd: &std::path::Path,
    receipt: &std::path::Path,
) -> ExternalProviderLaunch {
    ExternalProviderLaunch {
        executable: PathBuf::from("python3"),
        arguments: vec![
            "-u".to_owned(),
            "-c".to_owned(),
            LIFECYCLE_FIXTURE.to_owned(),
            mode.to_owned(),
            cwd.to_string_lossy().into_owned(),
            receipt.to_string_lossy().into_owned(),
        ],
        environment: Vec::new(),
        persistence_target: ProviderPersistenceTarget::Unspecified,
    }
}

/// Oracle: ACP v1 sessionCapabilities.list/resume/close gate the three methods;
/// resume does not replay history (docs/protocol/v1/session-setup.mdx:205-278).
#[tokio::test]
async fn advertised_list_resume_and_idle_close_keep_wire_order() {
    let root = tempfile::tempdir().expect("fixture root");
    let receipt = root.path().join("exchange.json");
    let sink = Arc::new(CountingEventSink(AtomicUsize::new(0)));
    let client = AgentSessionClient::initialize(
        fixture_launch("idle-close", root.path(), &receipt),
        Arc::new(NoopInteractionPort),
        sink.clone(),
    )
    .await
    .expect("fixture initializes");
    let listed = client
        .list_sessions(Some(root.path().to_path_buf()))
        .await
        .expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].provider_session_id, "fixture-session");
    assert_eq!(listed[0].title.as_deref(), Some("Fixture session"));
    client
        .resume_session("fixture-session".to_owned(), root.path().to_path_buf())
        .await
        .expect("resume");
    assert_eq!(sink.0.load(Ordering::Relaxed), 0, "resume does not replay");
    client
        .prompt_with_approval_context("fixture-session".to_owned(), "Hello".to_owned(), ())
        .await
        .expect("prompt");
    client
        .close_session("fixture-session".to_owned())
        .await
        .expect("close");
    client.shutdown().await;
    let seen: Vec<String> =
        serde_json::from_slice(&std::fs::read(receipt).expect("receipt")).expect("receipt JSON");
    assert_eq!(seen, ["list", "resume", "prompt", "close"]);
}

/// Oracle: specification R13 requires close to end the active Turn first;
/// R5 requires cancel to settle at the prompt result before closure.
#[tokio::test]
async fn close_cancels_running_turn_before_session_close() {
    let root = tempfile::tempdir().expect("fixture root");
    let receipt = root.path().join("exchange.json");
    let client = AgentSessionClient::initialize(
        fixture_launch("running-close", root.path(), &receipt),
        Arc::new(NoopInteractionPort),
        Arc::new(CountingEventSink(AtomicUsize::new(0))),
    )
    .await
    .expect("fixture initializes");
    client
        .list_sessions(Some(root.path().to_path_buf()))
        .await
        .expect("list");
    client
        .resume_session("fixture-session".to_owned(), root.path().to_path_buf())
        .await
        .expect("resume");
    let (dispatch_tx, dispatch_rx) = tokio::sync::oneshot::channel();
    let prompt = client.prompt_with_approval_dispatch(
        "fixture-session".to_owned(),
        "Hello".to_owned(),
        (),
        Some(dispatch_tx),
    );
    tokio::pin!(prompt);
    let dispatched = tokio::select! {
        receipt = dispatch_rx => receipt.expect("prompt receipt"),
        result = &mut prompt => panic!("prompt settled before submission: {result:?}"),
    };
    assert_eq!(dispatched, ProviderPromptDispatchObservation::Submitted);
    let (close, prompt_result) = tokio::join!(
        client.close_session("fixture-session".to_owned()),
        &mut prompt,
    );
    client.shutdown().await;
    assert!(close.is_ok(), "close result: {close:?}");
    assert!(prompt_result.is_ok(), "prompt result: {prompt_result:?}");
    let seen: Vec<String> =
        serde_json::from_slice(&std::fs::read(receipt).expect("receipt")).expect("receipt JSON");
    assert_eq!(seen, ["list", "resume", "prompt", "cancel", "close"]);
}

/// Oracle: ACP v1 session-setup requires the client to omit resume, close and
/// list requests unless each capability is advertised.
#[tokio::test]
async fn unadvertised_lifecycle_methods_send_no_acp_request() {
    let root = tempfile::tempdir().expect("fixture root");
    let marker = root.path().join("unexpected.txt");
    let client = AgentSessionClient::initialize(
        ExternalProviderLaunch {
            executable: PathBuf::from("python3"),
            arguments: vec![
                "-u".to_owned(),
                "-c".to_owned(),
                UNADVERTISED_FIXTURE.to_owned(),
                marker.to_string_lossy().into_owned(),
            ],
            environment: Vec::new(),
            persistence_target: ProviderPersistenceTarget::Unspecified,
        },
        Arc::new(NoopInteractionPort),
        Arc::new(CountingEventSink(AtomicUsize::new(0))),
    )
    .await
    .expect("fixture initializes");
    let listed = client.list_sessions(None).await;
    let loaded = client
        .load_session("fixture-session".to_owned(), root.path().to_path_buf())
        .await;
    let resumed = client
        .resume_session("fixture-session".to_owned(), root.path().to_path_buf())
        .await;
    let closed = client.close_session("fixture-session".to_owned()).await;
    client.shutdown().await;
    assert!(matches!(
        listed,
        Err(
            acp_client_runtime::ExternalProviderRuntimeError::UnsupportedCapability {
                capability: "session/list"
            }
        )
    ));
    assert!(matches!(
        loaded,
        Err(
            acp_client_runtime::ExternalProviderRuntimeError::UnsupportedCapability {
                capability: "session/load"
            }
        )
    ));
    assert!(matches!(
        resumed,
        Err(
            acp_client_runtime::ExternalProviderRuntimeError::UnsupportedCapability {
                capability: "session/resume"
            }
        )
    ));
    assert!(matches!(
        closed,
        Err(
            acp_client_runtime::ExternalProviderRuntimeError::UnsupportedCapability {
                capability: "session/close"
            }
        )
    ));
    assert!(!marker.exists(), "unadvertised request never sent");
}

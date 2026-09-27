//! Model-neutral prompt content reaches ACP in order and under R11 gates.

#![allow(clippy::expect_used)]

use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use acp_client_runtime::{
    AgentSessionClient, ApprovalPortOutcome, EventSinkClosed, ExternalProviderLaunch,
    ExternalProviderRuntimeError, HistoryReplayFuture, InteractionFuture, InteractionPort,
    ProviderPersistenceTarget, RefusedApprovalOffer, SessionEventSink,
};
use session_event_model::{ApprovalRequest, InputId, PromptContent, SessionEvent};
use tokio_util::sync::CancellationToken;

const MULTIBLOCK_FIXTURE: &str = r#"
import json,sys
mode=sys.argv[1]
def read(): return json.loads(sys.stdin.readline())
def send(value): print(json.dumps(value),flush=True)
request=read()
assert request['method']=='initialize'
capabilities={'promptCapabilities':{'image':True}} if mode=='image' else {}
meta={'steering':{'supported':True}} if mode=='steer' else {}
send({'jsonrpc':'2.0','id':request['id'],'result':{'protocolVersion':1,
    'agentCapabilities':capabilities,'agentInfo':{'name':'multiblock-fixture','version':'1'},
    '_meta':meta}})
request=read()
assert request['method']=='session/new'
send({'jsonrpc':'2.0','id':request['id'],'result':{'sessionId':'fixture-session'}})
request=read()
if mode=='unsupported':
    assert request['method']=='session/prompt',request
    assert request['params']['prompt']==[{'type':'text','text':'After rejection'}],request
    send({'jsonrpc':'2.0','id':request['id'],'result':{'stopReason':'end_turn'}})
elif mode=='image':
    assert request['method']=='session/prompt',request
    assert request['params']['prompt']==[
        {'type':'image','data':'aGVsbG8=','mimeType':'image/png'}],request
    send({'jsonrpc':'2.0','id':request['id'],'result':{'stopReason':'end_turn'}})
elif mode=='steer':
    assert request['method']=='_session/steering',request
    assert request['params']['prompt']==[
        {'type':'text','text':'Read'},
        {'type':'resource_link','uri':'file:///notes.txt','name':'notes.txt',
         'mimeType':'text/plain'}],request
    assert request['params']['_meta']['steering']['idleBehavior']=='promptRequired'
    send({'jsonrpc':'2.0','id':request['id'],'result':{'outcome':'promptRequired'}})
elif mode=='approval':
    assert request['method']=='session/prompt',request
    assert len(request['params']['prompt'])==2,request
    send({'jsonrpc':'2.0','id':91,'method':'session/request_permission','params':{
        'sessionId':'fixture-session',
        'toolCall':{'toolCallId':'tool-1','title':'Run command','kind':'execute'},
        'options':[{'optionId':'allow-once','name':'Allow','kind':'allow_once'}]}})
    answer=read()
    assert answer['id']==91 and answer['result']['outcome']['outcome']=='cancelled',answer
    send({'jsonrpc':'2.0','id':request['id'],'result':{'stopReason':'end_turn'}})
else:
    assert request['method']=='session/prompt',request
    assert request['params']['prompt']==[
        {'type':'text','text':'Read'},
        {'type':'resource_link','uri':'file:///notes.txt','name':'notes.txt',
         'mimeType':'text/plain'}],request
    send({'jsonrpc':'2.0','id':request['id'],'result':{'stopReason':'end_turn'}})
sys.stdin.read()
"#;

#[derive(Default)]
struct NoopInteractionPort {
    approval_requests: AtomicUsize,
}
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
        self.approval_requests.fetch_add(1, Ordering::SeqCst);
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

struct NoopEventSink;
impl SessionEventSink for NoopEventSink {
    fn begin_history_replay(&self, _session_id: &str) -> HistoryReplayFuture<'_> {
        Box::pin(async { Ok(()) })
    }
    fn publish(&self, _session_id: &str, _event: SessionEvent) -> Result<(), EventSinkClosed> {
        Ok(())
    }
}

async fn fixture_client(
    mode: &str,
) -> (
    tempfile::TempDir,
    AgentSessionClient<NoopInteractionPort>,
    Arc<NoopInteractionPort>,
) {
    let root = tempfile::tempdir().expect("fixture root");
    let port = Arc::new(NoopInteractionPort::default());
    let client = AgentSessionClient::initialize(
        ExternalProviderLaunch {
            executable: PathBuf::from("python3"),
            arguments: vec![
                "-u".to_owned(),
                "-c".to_owned(),
                MULTIBLOCK_FIXTURE.to_owned(),
                mode.to_owned(),
            ],
            environment: Vec::new(),
            persistence_target: ProviderPersistenceTarget::Unspecified,
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
    (root, client, port)
}

fn text_and_link() -> Vec<PromptContent> {
    vec![
        PromptContent::text("Read".to_owned()).expect("text"),
        PromptContent::resource_link(
            "file:///notes.txt".to_owned(),
            "notes.txt".to_owned(),
            Some("text/plain".to_owned()),
        )
        .expect("resource link"),
    ]
}

#[tokio::test]
async fn text_and_resource_link_reach_agent_as_two_ordered_blocks() {
    let (_root, client, _port) = fixture_client("blocks").await;
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        client.prompt_contents_for_operation_with_input(
            "fixture-session".to_owned(),
            InputId::generate(),
            None,
            text_and_link(),
            None,
        ),
    )
    .await
    .expect("prompt completes");
    assert!(result.is_ok(), "prompt result: {result:?}");
    client.shutdown().await;
}

#[tokio::test]
async fn unsupported_image_is_rejected_before_any_acp_prompt() {
    let (_root, client, port) = fixture_client("unsupported").await;
    let image =
        PromptContent::image("image/png".to_owned(), "aGVsbG8=".to_owned(), None).expect("image");
    let (dispatch, dispatch_result) = tokio::sync::oneshot::channel();
    let result = client
        .prompt_contents_with_approval_dispatch_for_input(
            "fixture-session".to_owned(),
            InputId::generate(),
            vec![image],
            (),
            Some(dispatch),
        )
        .await;
    assert!(
        matches!(
            result,
            Err(ExternalProviderRuntimeError::UnsupportedContent {
                content_type: "image"
            })
        ),
        "result: {result:?}"
    );
    assert_eq!(
        dispatch_result.await.expect("dispatch result"),
        acp_client_runtime::ProviderPromptDispatchObservation::NotSubmitted
    );
    assert_eq!(port.approval_requests.load(Ordering::SeqCst), 0);
    let follow_up = tokio::time::timeout(
        Duration::from_secs(2),
        client.prompt("fixture-session".to_owned(), "After rejection".to_owned()),
    )
    .await
    .expect("follow-up completes");
    assert!(follow_up.is_ok(), "follow-up: {follow_up:?}");
    client.shutdown().await;
}

#[tokio::test]
async fn advertised_image_reaches_agent_with_exact_data_and_mime_type() {
    let (_root, client, _port) = fixture_client("image").await;
    let image =
        PromptContent::image("image/png".to_owned(), "aGVsbG8=".to_owned(), None).expect("image");
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        client.prompt_contents_for_operation_with_input(
            "fixture-session".to_owned(),
            InputId::generate(),
            None,
            vec![image],
            None,
        ),
    )
    .await
    .expect("image prompt completes");
    assert!(result.is_ok(), "prompt result: {result:?}");
    client.shutdown().await;
}

#[tokio::test]
async fn multi_block_steer_reaches_extension_in_order() {
    let (_root, client, _port) = fixture_client("steer").await;
    let image =
        PromptContent::image("image/png".to_owned(), "aGVsbG8=".to_owned(), None).expect("image");
    let rejected = client
        .steer_contents_with_input(
            "fixture-session".to_owned(),
            InputId::generate(),
            vec![image],
        )
        .await;
    assert!(matches!(
        rejected,
        Err(ExternalProviderRuntimeError::UnsupportedContent {
            content_type: "image"
        })
    ));
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        client.steer_contents_with_input(
            "fixture-session".to_owned(),
            InputId::generate(),
            text_and_link(),
        ),
    )
    .await
    .expect("steer completes");
    assert_eq!(
        result.expect("steer result"),
        acp_client_runtime::ProviderSteeringOutcome::PromptRequired
    );
    client.shutdown().await;
}

#[tokio::test]
async fn multi_block_delivery_retains_approval_context() {
    let (_root, client, port) = fixture_client("approval").await;
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        client.prompt_contents_with_approval_dispatch_for_input(
            "fixture-session".to_owned(),
            InputId::generate(),
            text_and_link(),
            (),
            None,
        ),
    )
    .await
    .expect("prompt completes");
    assert!(result.is_ok(), "prompt result: {result:?}");
    assert_eq!(port.approval_requests.load(Ordering::SeqCst), 1);
    client.shutdown().await;
}

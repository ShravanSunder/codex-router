//! Cursor's connection requests must retain their originating Session.

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
use session_event_model::{ApprovalRequest, SessionEvent, SessionItemKind};
use tokio_util::sync::CancellationToken;

const TWO_SESSION_TODOS: &str = r#"
import json,sys
def read(): return json.loads(sys.stdin.readline())
def send(value): print(json.dumps(value),flush=True)
request=read()
assert request['method']=='initialize'
send({'jsonrpc':'2.0','id':request['id'],'result':{
    'protocolVersion':1,'agentCapabilities':{},
    'agentInfo':{'name':'todo-fixture','version':'1'}}})
for session_id in ('first','second'):
    request=read()
    assert request['method']=='session/new',request
    send({'jsonrpc':'2.0','id':request['id'],'result':{'sessionId':session_id}})
prompts={}
for _ in range(2):
    prompt=read()
    assert prompt['method']=='session/prompt',prompt
    prompts[prompt['params']['sessionId']]=prompt['id']
for session_id,tool_id,content in (('first','tool-first','First task'),('second','tool-second','Second task')):
    send({'jsonrpc':'2.0','method':'session/update','params':{
        'sessionId':session_id,'update':{'sessionUpdate':'tool_call',
        'toolCallId':tool_id,'title':'Update plan','kind':'other','status':'pending'}}})
    send({'jsonrpc':'2.0','id':'todo-'+session_id,'method':'cursor/update_todos',
        'params':{'toolCallId':tool_id,'todos':[{'id':'one','content':content,'status':'pending'}],
                  'merge':False}})
    response=read()
    assert response['id']=='todo-'+session_id,response
    assert response.get('result')=={},response
    send({'jsonrpc':'2.0','id':'plan-'+session_id,'method':'cursor/create_plan',
        'params':{'toolCallId':tool_id,'name':'Plan for '+session_id,
                  'overview':'Scoped plan','plan':'# Steps',
                  'todos':[{'id':'one','content':content,'status':'completed'}]}})
    response=read()
    assert response['id']=='plan-'+session_id and response.get('result')=={'outcome':'cancelled'},response
send({'jsonrpc':'2.0','id':'todo-unknown','method':'cursor/update_todos',
    'params':{'toolCallId':'unknown','todos':[{'id':'x','content':'Must not appear','status':'pending'}],
              'merge':False}})
response=read()
assert response['id']=='todo-unknown' and response.get('result')=={},response
send({'jsonrpc':'2.0','id':'question-unknown','method':'cursor/ask_question',
    'params':{'toolCallId':'unknown','questions':[{'id':'q','prompt':'Which?',
        'options':[{'id':'a','label':'A'}]}]}})
response=read()
assert response['id']=='question-unknown' and response.get('result')=={'outcome':{'outcome':'cancelled'}},response
for session_id,prompt_id in prompts.items():
    send({'jsonrpc':'2.0','id':prompt_id,'result':{'stopReason':'end_turn'}})
sys.stdin.read()
"#;

const SINGLE_RUNNING_TODO: &str = r#"
import json,sys
def read(): return json.loads(sys.stdin.readline())
def send(value): print(json.dumps(value),flush=True)
request=read()
assert request['method']=='initialize'
send({'jsonrpc':'2.0','id':request['id'],'result':{'protocolVersion':1,'agentCapabilities':{},
    'agentInfo':{'name':'single-todo-fixture','version':'1'}}})
request=read()
assert request['method']=='session/new'
send({'jsonrpc':'2.0','id':request['id'],'result':{'sessionId':'only-session'}})
prompt=read()
assert prompt['method']=='session/prompt'
send({'jsonrpc':'2.0','id':'todo','method':'cursor/update_todos',
    'params':{'toolCallId':'unobserved','todos':[{'id':'one','content':'Fallback task','status':'pending'}],
              'merge':False}})
response=read()
assert response['id']=='todo' and response.get('result')=={},response
send({'jsonrpc':'2.0','id':prompt['id'],'result':{'stopReason':'end_turn'}})
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
struct CaptureEventSink(Mutex<Vec<(String, SessionEvent)>>);
impl SessionEventSink for CaptureEventSink {
    fn begin_history_replay(&self, _session_id: &str) -> HistoryReplayFuture<'_> {
        Box::pin(async { Ok(()) })
    }
    fn publish(&self, session_id: &str, event: SessionEvent) -> Result<(), EventSinkClosed> {
        self.0
            .lock()
            .expect("event sink")
            .push((session_id.to_owned(), event));
        Ok(())
    }
}

/// Oracle: Cursor omits sessionId in update_todos; observed tool calls on
/// concurrent Sessions must keep each plan Item in its own Session history.
#[tokio::test]
async fn concurrent_cursor_todos_publish_to_their_observed_sessions() {
    let root = tempfile::tempdir().expect("fixture root");
    let sink = Arc::new(CaptureEventSink::default());
    let client = AgentSessionClient::initialize(
        ExternalProviderLaunch {
            executable: PathBuf::from("python3"),
            arguments: vec![
                "-u".to_owned(),
                "-c".to_owned(),
                TWO_SESSION_TODOS.to_owned(),
            ],
            environment: Vec::new(),
            persistence_target: ProviderPersistenceTarget::Unspecified,
        },
        Arc::new(NoopInteractionPort),
        sink.clone(),
    )
    .await
    .expect("fixture initializes");
    let first = client
        .create_session(root.path().to_path_buf())
        .await
        .expect("first session");
    let second = client
        .create_session(root.path().to_path_buf())
        .await
        .expect("second session");
    let (first_result, second_result) =
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            tokio::join!(
                client.prompt_with_approval_context(first.clone(), "One".to_owned(), ()),
                client.prompt_with_approval_context(second.clone(), "Two".to_owned(), ())
            )
        })
        .await
        .expect("fixture exchanges complete");
    first_result.expect("first prompt");
    second_result.expect("second prompt");
    client.shutdown().await;
    let events = sink.0.lock().expect("event sink");
    for (session_id, expected_text) in [
        (first.as_str(), "First task"),
        (second.as_str(), "Second task"),
    ] {
        assert!(
            events.iter().any(|(owner, event)| owner == session_id
                && matches!(event, SessionEvent::ItemStarted { item }
                if item.kind == SessionItemKind::Plan
                    && item.text.as_deref().is_some_and(|text| text.contains(expected_text)))),
            "plan Item for {session_id}: {events:?}"
        );
        assert!(
            events.iter().any(|(owner, event)| owner == session_id
                && matches!(event, SessionEvent::ItemUpdated { item }
                    if item.kind == SessionItemKind::Plan
                        && item.text.as_deref().is_some_and(|text| text.contains(&format!("Plan for {session_id}"))))),
            "updated plan for {session_id}: {events:?}"
        );
    }
    assert_eq!(events.iter().filter(|(_, event)| matches!(event, SessionEvent::ItemStarted { item } if item.kind == SessionItemKind::Plan)).count(), 2);
}

/// Without a tool card, one running Turn is a unique Session owner.
#[tokio::test]
async fn a_single_running_turn_routes_an_unobserved_cursor_todo() {
    let root = tempfile::tempdir().expect("fixture root");
    let sink = Arc::new(CaptureEventSink::default());
    let client = AgentSessionClient::initialize(
        ExternalProviderLaunch {
            executable: PathBuf::from("python3"),
            arguments: vec![
                "-u".to_owned(),
                "-c".to_owned(),
                SINGLE_RUNNING_TODO.to_owned(),
            ],
            environment: Vec::new(),
            persistence_target: ProviderPersistenceTarget::Unspecified,
        },
        Arc::new(NoopInteractionPort),
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
        client.prompt(session_id.clone(), "Go".to_owned()),
    )
    .await
    .expect("exchange completes")
    .expect("prompt ends");
    client.shutdown().await;
    assert!(
        sink.0
            .lock()
            .expect("event sink")
            .iter()
            .any(|(owner, event)| owner == &session_id
                && matches!(event, SessionEvent::ItemStarted { item }
            if item.kind == SessionItemKind::Plan
                && item.text.as_deref().is_some_and(|text| text.contains("Fallback task"))))
    );
}

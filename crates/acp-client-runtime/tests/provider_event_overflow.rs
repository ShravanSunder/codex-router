//! A full event sink cancels an active provider Turn without inventing its end.

#![allow(clippy::expect_used)]

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use acp_client_runtime::{
    AgentSessionClient, ApprovalPortOutcome, EventSinkOverflow, ExternalProviderLaunch,
    ExternalProviderRuntimeError, HistoryReplayFuture, InteractionFuture, InteractionPort,
    ProviderPersistenceTarget, RefusedApprovalOffer, SessionEventSink,
};
use session_event_model::{
    ApprovalRequest, LocalCause, SessionEvent, SessionItemKind, StopReason, TurnOutcome,
};
use tokio_util::sync::CancellationToken;

const OVERFLOW_FIXTURE: &str = r#"
import json,sys
def read(): return json.loads(sys.stdin.readline())
def send(value): print(json.dumps(value),flush=True)
request=read()
assert request['method']=='initialize'
send({'jsonrpc':'2.0','id':request['id'],'result':{'protocolVersion':1,'agentCapabilities':{},
    'agentInfo':{'name':'overflow-fixture','version':'1'}}})
request=read()
assert request['method']=='session/new'
send({'jsonrpc':'2.0','id':request['id'],'result':{'sessionId':'fixture-session'}})
prompt=read()
assert prompt['method']=='session/prompt'
send({'jsonrpc':'2.0','method':'session/update','params':{'sessionId':'fixture-session',
    'update':{'sessionUpdate':'agent_message_chunk',
              'content':{'type':'text','text':'Chunk'}}}})
cancel=read()
assert cancel['method']=='session/cancel',cancel
send({'jsonrpc':'2.0','id':prompt['id'],'result':{'stopReason':'end_turn'}})
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

const PLAN_OVERFLOW_FIXTURE: &str = r#"
import json,sys
def read(): return json.loads(sys.stdin.readline())
def send(value): print(json.dumps(value),flush=True)
request=read()
assert request['method']=='initialize'
send({'jsonrpc':'2.0','id':request['id'],'result':{'protocolVersion':1,'agentCapabilities':{},
    'agentInfo':{'name':'plan-overflow-fixture','version':'1'}}})
request=read()
assert request['method']=='session/new'
send({'jsonrpc':'2.0','id':request['id'],'result':{'sessionId':'fixture-session'}})
prompt=read()
assert prompt['method']=='session/prompt'
send({'jsonrpc':'2.0','method':'session/update','params':{'sessionId':'fixture-session',
    'update':{'sessionUpdate':'tool_call','toolCallId':'plan-tool',
              'title':'Create plan','kind':'other','status':'pending'}}})
send({'jsonrpc':'2.0','id':'create-plan','method':'cursor/create_plan',
    'params':{'toolCallId':'plan-tool','name':'Plan','plan':'# Steps'}})
received=[]
for _ in range(2): received.append(read())
assert any(message.get('method')=='session/cancel' for message in received),received
assert any(message.get('id')=='create-plan' and message.get('result',{}).get('outcome')=='cancelled'
           for message in received),received
send({'jsonrpc':'2.0','id':prompt['id'],'result':{'stopReason':'end_turn'}})
sys.stdin.read()
"#;

const AUTH_OVERFLOW_FIXTURE: &str = r#"
import json,sys
def read(): return json.loads(sys.stdin.readline())
def send(value): print(json.dumps(value),flush=True)
request=read()
assert request['method']=='initialize'
send({'jsonrpc':'2.0','id':request['id'],'result':{'protocolVersion':1,'agentCapabilities':{},
    'agentInfo':{'name':'auth-overflow-fixture','version':'1'}}})
request=read()
assert request['method']=='session/new'
send({'jsonrpc':'2.0','id':request['id'],'result':{'sessionId':'fixture-session'}})
prompt=read()
assert prompt['method']=='session/prompt'
send({'jsonrpc':'2.0','method':'_auth/status_update',
    'params':{'authStatus':{'kind':'none','label':'Signed out'}}})
cancel=read()
assert cancel['method']=='session/cancel',cancel
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
struct RejectItemSink(Mutex<Vec<SessionEvent>>);
impl SessionEventSink for RejectItemSink {
    fn begin_history_replay(&self, _session_id: &str) -> HistoryReplayFuture<'_> {
        Box::pin(async { Ok(()) })
    }
    fn publish(&self, _session_id: &str, event: SessionEvent) -> Result<(), EventSinkOverflow> {
        if matches!(event, SessionEvent::ItemStarted { .. }) {
            return Err(EventSinkOverflow);
        }
        self.0.lock().expect("event sink").push(event);
        Ok(())
    }
}

#[derive(Default)]
struct RejectPlanSink(Mutex<Vec<SessionEvent>>);
impl SessionEventSink for RejectPlanSink {
    fn begin_history_replay(&self, _session_id: &str) -> HistoryReplayFuture<'_> {
        Box::pin(async { Ok(()) })
    }
    fn publish(&self, _session_id: &str, event: SessionEvent) -> Result<(), EventSinkOverflow> {
        if matches!(&event, SessionEvent::ItemStarted { item } if item.kind == SessionItemKind::Plan)
        {
            return Err(EventSinkOverflow);
        }
        self.0.lock().expect("event sink").push(event);
        Ok(())
    }
}

#[derive(Default)]
struct RejectAuthSink(Mutex<Vec<SessionEvent>>);
impl SessionEventSink for RejectAuthSink {
    fn begin_history_replay(&self, _session_id: &str) -> HistoryReplayFuture<'_> {
        Box::pin(async { Ok(()) })
    }
    fn publish(&self, _session_id: &str, event: SessionEvent) -> Result<(), EventSinkOverflow> {
        if matches!(event, SessionEvent::CapabilitiesChanged { .. }) {
            return Err(EventSinkOverflow);
        }
        self.0.lock().expect("event sink").push(event);
        Ok(())
    }
}

/// A connection-scoped auth notification must cancel the active Turn when
/// its capability event cannot enter the Session history.
#[tokio::test]
async fn rejected_auth_change_cancels_running_turn_with_local_cause() {
    let root = tempfile::tempdir().expect("fixture root");
    let sink = Arc::new(RejectAuthSink::default());
    let client = AgentSessionClient::initialize(
        ExternalProviderLaunch {
            executable: PathBuf::from("python3"),
            arguments: vec![
                "-u".to_owned(),
                "-c".to_owned(),
                AUTH_OVERFLOW_FIXTURE.to_owned(),
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
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        client.prompt(session_id, "Check auth".to_owned()),
    )
    .await
    .expect("turn settles");
    assert!(matches!(
        result,
        Err(ExternalProviderRuntimeError::PromptOutputLimitExceeded)
    ));
    client.shutdown().await;
    let events = sink.0.lock().expect("event sink");
    assert!(matches!(
        events.last(),
        Some(SessionEvent::TurnEnded {
            outcome: TurnOutcome::Ended {
                stop_reason: StopReason::EndTurn,
                local_cause: Some(LocalCause::OutputOverflow),
            },
            ..
        })
    ));
}

/// A connection-scoped Cursor plan request can overflow the same Turn's
/// Item sink, even though its callback does not pass through read_update.
#[tokio::test]
async fn rejected_cursor_plan_cancels_running_turn_with_local_cause() {
    let root = tempfile::tempdir().expect("fixture root");
    let sink = Arc::new(RejectPlanSink::default());
    let client = AgentSessionClient::initialize(
        ExternalProviderLaunch {
            executable: PathBuf::from("python3"),
            arguments: vec![
                "-u".to_owned(),
                "-c".to_owned(),
                PLAN_OVERFLOW_FIXTURE.to_owned(),
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
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        client.prompt_with_approval_context(session_id, "Plan".to_owned(), ()),
    )
    .await
    .expect("turn settles");
    assert!(matches!(
        result,
        Err(ExternalProviderRuntimeError::PromptOutputLimitExceeded)
    ));
    client.shutdown().await;
    let events = sink.0.lock().expect("event sink");
    assert!(matches!(
        events.last(),
        Some(SessionEvent::TurnEnded {
            outcome: TurnOutcome::Ended {
                stop_reason: StopReason::EndTurn,
                local_cause: Some(LocalCause::OutputOverflow),
            },
            ..
        })
    ));
}

/// Oracle: R5 and Program Design overflow row require a cancel, followed by
/// the agent-confirmed stop reason plus Router's separate local cause.
#[tokio::test]
async fn rejected_item_cancels_turn_and_keeps_agent_stop_reason() {
    let root = tempfile::tempdir().expect("fixture root");
    let sink = Arc::new(RejectItemSink::default());
    let client = AgentSessionClient::initialize(
        ExternalProviderLaunch {
            executable: PathBuf::from("python3"),
            arguments: vec![
                "-u".to_owned(),
                "-c".to_owned(),
                OVERFLOW_FIXTURE.to_owned(),
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
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        client.prompt(session_id, "Generate".to_owned()),
    )
    .await
    .expect("turn settles");
    assert!(
        matches!(
            result,
            Err(ExternalProviderRuntimeError::PromptOutputLimitExceeded)
        ),
        "prompt result: {result:?}"
    );
    client.shutdown().await;
    let events = sink.0.lock().expect("event sink");
    assert!(matches!(
        events.first(),
        Some(SessionEvent::TurnStarted { .. })
    ));
    assert!(matches!(
        events.last(),
        Some(SessionEvent::TurnEnded {
            outcome: TurnOutcome::Ended {
                stop_reason: StopReason::EndTurn,
                local_cause: Some(LocalCause::OutputOverflow),
            },
            ..
        })
    ));
}

/// A provider that closes stdout before its prompt result cannot confirm a
/// stop reason; the Turn is lost and keeps the retirement reason.
#[tokio::test]
async fn provider_eof_ends_running_turn_lost() {
    let root = tempfile::tempdir().expect("fixture root");
    let sink = Arc::new(RejectItemSink::default());
    let client = AgentSessionClient::initialize(
        ExternalProviderLaunch {
            executable: PathBuf::from("python3"),
            arguments: vec![
                "-u".to_owned(),
                "-c".to_owned(),
                LOST_PROVIDER_FIXTURE.to_owned(),
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
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        client.prompt(session_id, "Run".to_owned()),
    )
    .await
    .expect("turn settles");
    assert!(result.is_err());
    client.shutdown().await;
    let events = sink.0.lock().expect("event sink");
    assert!(matches!(
        events.first(),
        Some(SessionEvent::TurnStarted { .. })
    ));
    assert!(
        matches!(events.last(), Some(SessionEvent::TurnEnded {
        outcome: TurnOutcome::Lost { reason }, ..
    }) if reason == "providerRetired"),
        "events: {events:?}"
    );
}

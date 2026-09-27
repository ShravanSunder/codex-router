//! Provider Input and Turn identity at the scripted ACP boundary.

#![allow(clippy::expect_used)]

use std::{path::PathBuf, sync::Arc, time::Duration};

use acp_client_runtime::{
    AgentSessionClient, ApprovalPortOutcome, EventSinkClosed, ExternalProviderLaunch,
    HistoryReplayFuture, InteractionFuture, InteractionPort, ProviderPersistenceTarget,
    RefusedApprovalOffer, SessionEventSink,
};
use session_event_model::{ApprovalRequest, InputId, SessionEvent};
use tokio_util::sync::CancellationToken;

const INJECTED_STEER_FIXTURE: &str = r#"
import json,sys
def read(): return json.loads(sys.stdin.readline())
def send(value): print(json.dumps(value),flush=True)
request=read()
assert request['method']=='initialize'
send({'jsonrpc':'2.0','id':request['id'],'result':{
    'protocolVersion':1,'agentCapabilities':{},
    'agentInfo':{'name':'input-fixture','version':'1'},
    '_meta':{'steering':{'supported':True}}}})
request=read()
assert request['method']=='session/new'
send({'jsonrpc':'2.0','id':request['id'],'result':{'sessionId':'fixture-session'}})
prompt=read()
assert prompt['method']=='session/prompt'
steer=read()
assert steer['method']=='_session/steering'
send({'jsonrpc':'2.0','id':steer['id'],'result':{'outcome':'injected'}})
send({'jsonrpc':'2.0','id':prompt['id'],'result':{'stopReason':'end_turn'}})
sys.stdin.read()
"#;

const STARTED_STEER_FIXTURE: &str = r#"
import json,sys
def read(): return json.loads(sys.stdin.readline())
def send(value): print(json.dumps(value),flush=True)
request=read()
assert request['method']=='initialize'
send({'jsonrpc':'2.0','id':request['id'],'result':{
    'protocolVersion':1,'agentCapabilities':{},
    'agentInfo':{'name':'started-steer-fixture','version':'1'},
    '_meta':{'steering':{'supported':True}}}})
request=read()
assert request['method']=='session/new'
send({'jsonrpc':'2.0','id':request['id'],'result':{'sessionId':'fixture-session'}})
steer=read()
assert steer['method']=='_session/steering'
send({'jsonrpc':'2.0','id':steer['id'],'result':{'outcome':'startedNewTurn'}})
sys.stdin.read()
"#;

const STREAMED_ITEMS_FIXTURE: &str = r#"
import json,sys
def read(): return json.loads(sys.stdin.readline())
def send(value): print(json.dumps(value),flush=True)
request=read()
assert request['method']=='initialize'
send({'jsonrpc':'2.0','id':request['id'],'result':{'protocolVersion':1,'agentCapabilities':{},
    'agentInfo':{'name':'item-fixture','version':'1'}}})
request=read()
assert request['method']=='session/new'
send({'jsonrpc':'2.0','id':request['id'],'result':{'sessionId':'fixture-session'}})
prompt=read()
assert prompt['method']=='session/prompt'
for kind,text in [('agent_message_chunk','Hello'),('agent_message_chunk',' world'),
                  ('agent_thought_chunk','Thinking')]:
    send({'jsonrpc':'2.0','method':'session/update','params':{'sessionId':'fixture-session',
        'update':{'sessionUpdate':kind,'content':{'type':'text','text':text}}}})
send({'jsonrpc':'2.0','method':'session/update','params':{'sessionId':'fixture-session',
    'update':{'sessionUpdate':'tool_call','toolCallId':'tool-one',
              'title':'Run check','kind':'execute','status':'pending'}}})
send({'jsonrpc':'2.0','method':'session/update','params':{'sessionId':'fixture-session',
    'update':{'sessionUpdate':'tool_call_update','toolCallId':'tool-one',
              'title':'Check passed','status':'completed'}}})
send({'jsonrpc':'2.0','method':'session/update','params':{'sessionId':'fixture-session',
    'update':{'sessionUpdate':'plan','entries':[{'content':'Build client',
              'priority':'medium','status':'pending'}]}}})
send({'jsonrpc':'2.0','method':'session/update','params':{'sessionId':'fixture-session',
    'update':{'sessionUpdate':'future_kind','content':{'type':'text','text':'Future detail'}}}})
for update in [
    {'sessionUpdate':'current_mode_update','currentModeId':'ask'},
    {'sessionUpdate':'config_option_update','configOptions':[]},
    {'sessionUpdate':'session_info_update','title':'Working session'},
    {'sessionUpdate':'usage_update','used':5,'size':100},
    {'sessionUpdate':'available_commands_update','availableCommands':[]},
]:
    send({'jsonrpc':'2.0','method':'session/update','params':{'sessionId':'fixture-session','update':update}})
send({'jsonrpc':'2.0','id':prompt['id'],'result':{'stopReason':'end_turn'}})
sys.stdin.read()
"#;

const PROMPT_REQUIRED_FIXTURE: &str = r#"
import json,sys
def read(): return json.loads(sys.stdin.readline())
def send(value): print(json.dumps(value),flush=True)
request=read()
assert request['method']=='initialize'
send({'jsonrpc':'2.0','id':request['id'],'result':{'protocolVersion':1,'agentCapabilities':{},
    'agentInfo':{'name':'prompt-required-fixture','version':'1'},
    '_meta':{'steering':{'supported':True}}}})
request=read()
assert request['method']=='session/new'
send({'jsonrpc':'2.0','id':request['id'],'result':{'sessionId':'fixture-session'}})
steer=read()
assert steer['method']=='_session/steering'
assert steer['params']['_meta']['steering']['idleBehavior']=='promptRequired'
send({'jsonrpc':'2.0','id':steer['id'],'result':{'outcome':'promptRequired'}})
prompt=read()
assert prompt['method']=='session/prompt'
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

struct CaptureEventSink(tokio::sync::mpsc::UnboundedSender<SessionEvent>);
impl SessionEventSink for CaptureEventSink {
    fn begin_history_replay(&self, _session_id: &str) -> HistoryReplayFuture<'_> {
        Box::pin(async { Ok(()) })
    }
    fn publish(&self, _session_id: &str, event: SessionEvent) -> Result<(), EventSinkClosed> {
        self.0.send(event).map_err(|_| EventSinkClosed)
    }
}

/// Oracle: E5 gives each Input its own identity; a steer injected into a
/// running Turn links to that Turn without starting another one.
#[tokio::test]
async fn prompt_and_injected_steer_publish_distinct_inputs_on_one_turn() {
    let root = tempfile::tempdir().expect("fixture root");
    let (sender, mut events) = tokio::sync::mpsc::unbounded_channel();
    let client = AgentSessionClient::initialize(
        ExternalProviderLaunch {
            executable: PathBuf::from("python3"),
            arguments: vec![
                "-u".to_owned(),
                "-c".to_owned(),
                INJECTED_STEER_FIXTURE.to_owned(),
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
    assert!(matches!(
        events.recv().await,
        Some(SessionEvent::SettingsChanged { .. })
    ));
    let prompt_input = InputId::new("prompt-input").expect("input ID");
    let steer_input = InputId::new("steer-input").expect("input ID");
    let prompt = client.prompt_contents_with_approval_dispatch_for_input(
        session_id.clone(),
        prompt_input.clone(),
        vec![session_event_model::PromptContent::text("First".to_owned()).expect("text prompt")],
        (),
        None,
    );
    tokio::pin!(prompt);
    let turn_started = tokio::select! {
        event = tokio::time::timeout(Duration::from_secs(2), events.recv()) => event.expect("turn starts").expect("event"),
        result = &mut prompt => panic!("prompt settled before turn began: {result:?}"),
    };
    let SessionEvent::TurnStarted { turn_id, input_id } = turn_started else {
        panic!("expected TurnStarted, got {turn_started:?}");
    };
    assert_eq!(input_id, prompt_input);
    assert!(
        uuid::Uuid::parse_str(&turn_id)
            .expect("UUID turn ID")
            .get_version_num()
            == 7
    );

    let steering = client
        .steer_contents_with_input(
            session_id.clone(),
            steer_input.clone(),
            vec![
                session_event_model::PromptContent::text("Second".to_owned()).expect("text steer"),
            ],
        )
        .await
        .expect("steer accepted");
    assert!(matches!(
        steering,
        acp_client_runtime::ProviderSteeringOutcome::Injected { .. }
    ));
    let accepted = tokio::time::timeout(Duration::from_secs(2), events.recv())
        .await
        .expect("input accepted")
        .expect("event");
    assert_eq!(
        accepted,
        SessionEvent::InputAccepted {
            input_id: steer_input,
            turn_id: turn_id.clone()
        }
    );
    prompt.await.expect("prompt ends");
    let ended = tokio::time::timeout(Duration::from_secs(2), events.recv())
        .await
        .expect("turn ends")
        .expect("event");
    assert!(
        matches!(ended, SessionEvent::TurnEnded { turn_id: ended_turn_id, .. } if ended_turn_id == turn_id)
    );
    client.shutdown().await;
}

/// Oracle: R10 keeps an agent's startedNewTurn outcome distinct from an
/// injected steer even when Router requested promptRequired idle behavior.
#[tokio::test]
async fn started_new_turn_uses_the_steer_input_id() {
    let root = tempfile::tempdir().expect("fixture root");
    let (sender, mut events) = tokio::sync::mpsc::unbounded_channel();
    let client = AgentSessionClient::initialize(
        ExternalProviderLaunch {
            executable: PathBuf::from("python3"),
            arguments: vec![
                "-u".to_owned(),
                "-c".to_owned(),
                STARTED_STEER_FIXTURE.to_owned(),
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
    assert!(matches!(
        events.recv().await,
        Some(SessionEvent::SettingsChanged { .. })
    ));
    let input_id = InputId::new("new-turn-steer").expect("input ID");
    let steering = client
        .steer_contents_with_input(
            session_id,
            input_id.clone(),
            vec![session_event_model::PromptContent::text("Begin".to_owned()).expect("text steer")],
        )
        .await
        .expect("steer accepted");
    assert_eq!(
        steering,
        acp_client_runtime::ProviderSteeringOutcome::StartedNewTurn
    );
    let event = tokio::time::timeout(Duration::from_secs(2), events.recv())
        .await
        .expect("turn starts")
        .expect("event");
    let SessionEvent::TurnStarted {
        turn_id,
        input_id: observed_input,
    } = event
    else {
        panic!("expected TurnStarted, got {event:?}");
    };
    assert_eq!(observed_input, input_id);
    assert_eq!(
        uuid::Uuid::parse_str(&turn_id)
            .expect("UUID turn ID")
            .get_version_num(),
        7
    );
    let ended = tokio::time::timeout(Duration::from_secs(2), events.recv())
        .await
        .expect("unobservable turn settles")
        .expect("event");
    assert_eq!(
        ended,
        SessionEvent::TurnEnded {
            turn_id,
            outcome: session_event_model::TurnOutcome::Lost {
                reason: session_event_model::TurnLostReason::EndNotObservable
            },
        }
    );
    assert_eq!(
        client
            .session_activity("fixture-session".to_owned())
            .await
            .expect("activity"),
        acp_client_runtime::ProviderSessionActivity::Idle
    );
    client.shutdown().await;
}

#[tokio::test]
async fn prompt_required_preserves_steer_input_when_prompted_normally() {
    let root = tempfile::tempdir().expect("fixture root");
    let (sender, mut events) = tokio::sync::mpsc::unbounded_channel();
    let client = AgentSessionClient::initialize(
        ExternalProviderLaunch {
            executable: PathBuf::from("python3"),
            arguments: vec![
                "-u".to_owned(),
                "-c".to_owned(),
                PROMPT_REQUIRED_FIXTURE.to_owned(),
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
    assert!(matches!(
        events.recv().await,
        Some(SessionEvent::SettingsChanged { .. })
    ));
    let input_id = InputId::new("steer-then-prompt").expect("input ID");
    let steering = client
        .steer_contents_with_input(
            session_id.clone(),
            input_id.clone(),
            vec![session_event_model::PromptContent::text("Hello".to_owned()).expect("text steer")],
        )
        .await
        .expect("steer response");
    assert_eq!(
        steering,
        acp_client_runtime::ProviderSteeringOutcome::PromptRequired
    );
    client
        .prompt_contents_with_approval_dispatch_for_input(
            session_id,
            input_id.clone(),
            vec![
                session_event_model::PromptContent::text("Hello".to_owned()).expect("text prompt"),
            ],
            (),
            None,
        )
        .await
        .expect("prompt ends");
    let started = events.recv().await.expect("turn start");
    let SessionEvent::TurnStarted {
        turn_id,
        input_id: observed_input,
    } = started
    else {
        panic!("expected TurnStarted, got {started:?}");
    };
    assert_eq!(observed_input, input_id);
    assert!(
        matches!(events.recv().await, Some(SessionEvent::TurnEnded { turn_id: ended_id,
        outcome: session_event_model::TurnOutcome::Ended { stop_reason: session_event_model::StopReason::EndTurn, .. }
    }) if ended_id == turn_id)
    );
    client.shutdown().await;
}

/// Oracle: R24 presents the same ordered text Items to every subscriber;
/// streaming chunks update one Item before a different kind starts.
#[tokio::test]
async fn text_and_thought_chunks_publish_ordered_items() {
    let root = tempfile::tempdir().expect("fixture root");
    let (sender, mut events) = tokio::sync::mpsc::unbounded_channel();
    let client = AgentSessionClient::initialize(
        ExternalProviderLaunch {
            executable: PathBuf::from("python3"),
            arguments: vec![
                "-u".to_owned(),
                "-c".to_owned(),
                STREAMED_ITEMS_FIXTURE.to_owned(),
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
    let outcome = client
        .prompt_contents_with_approval_dispatch_for_input(
            session_id,
            session_event_model::InputId::generate(),
            vec![
                session_event_model::PromptContent::text("Begin".to_owned()).expect("text prompt"),
            ],
            (),
            None,
        )
        .await
        .expect("prompt ends");
    assert_eq!(outcome.output, "Hello world");
    client.shutdown().await;
    let mut observed = Vec::new();
    while let Ok(event) = events.try_recv() {
        observed.push(event);
    }
    let mut items = observed.iter().filter(|event| {
        matches!(
            event,
            SessionEvent::ItemStarted { .. }
                | SessionEvent::ItemUpdated { .. }
                | SessionEvent::ItemCompleted { .. }
        )
    });
    let Some(SessionEvent::ItemStarted { item: message }) = items.next() else {
        panic!("agent message not started: {observed:?}")
    };
    assert_eq!(
        message.kind,
        session_event_model::SessionItemKind::AgentMessage
    );
    assert_eq!(message.text.as_deref(), Some("Hello"));
    let Some(SessionEvent::ItemUpdated { item: updated }) = items.next() else {
        panic!("agent message not updated: {observed:?}")
    };
    assert_eq!(updated.item_id, message.item_id);
    assert_eq!(updated.text.as_deref(), Some("Hello world"));
    assert_eq!(
        items.next(),
        Some(&SessionEvent::ItemCompleted {
            item_id: message.item_id.clone()
        })
    );
    let Some(SessionEvent::ItemStarted { item: thought }) = items.next() else {
        panic!("thought not started: {observed:?}")
    };
    assert_eq!(
        thought.kind,
        session_event_model::SessionItemKind::AgentThought
    );
    assert_eq!(thought.text.as_deref(), Some("Thinking"));
    assert_eq!(
        items.next(),
        Some(&SessionEvent::ItemCompleted {
            item_id: thought.item_id.clone()
        })
    );
    let Some(SessionEvent::ItemStarted { item: tool }) = items.next() else {
        panic!("tool not started: {observed:?}")
    };
    assert_eq!(tool.item_id, "tool-one");
    assert!(matches!(
        &tool.kind,
        session_event_model::SessionItemKind::ToolCall {
            status: session_event_model::ToolCallStatus::Pending,
            ..
        }
    ));
    let Some(SessionEvent::ItemUpdated { item: updated_tool }) = items.next() else {
        panic!("tool not updated: {observed:?}")
    };
    assert_eq!(updated_tool.item_id, tool.item_id);
    assert!(matches!(
        &updated_tool.kind,
        session_event_model::SessionItemKind::ToolCall {
            status: session_event_model::ToolCallStatus::Completed,
            ..
        }
    ));
    let Some(SessionEvent::ItemStarted { item: plan }) = items.next() else {
        panic!("plan not started: {observed:?}")
    };
    assert_eq!(plan.kind, session_event_model::SessionItemKind::Plan);
    assert!(
        plan.text
            .as_deref()
            .is_some_and(|text| text.contains("Build client"))
    );
    let Some(SessionEvent::ItemStarted { item: unknown }) = items.next() else {
        panic!("unknown update not recorded: {observed:?}")
    };
    assert_eq!(
        unknown.kind,
        session_event_model::SessionItemKind::Unknown {
            source_kind: "future_kind".to_owned()
        }
    );
    assert_eq!(unknown.text.as_deref(), Some("Future detail"));
    assert_eq!(
        items.next(),
        Some(&SessionEvent::ItemCompleted {
            item_id: unknown.item_id.clone()
        })
    );
    for expected_kind in [
        session_event_model::SessionItemKind::ModeChange,
        session_event_model::SessionItemKind::ConfigChange,
        session_event_model::SessionItemKind::SessionInfo,
        session_event_model::SessionItemKind::Usage,
        session_event_model::SessionItemKind::Notice,
    ] {
        let Some(SessionEvent::ItemStarted { item }) = items.next() else {
            panic!("metadata Item absent: {observed:?}")
        };
        assert_eq!(item.kind, expected_kind);
        assert_eq!(
            items.next(),
            Some(&SessionEvent::ItemCompleted {
                item_id: item.item_id.clone()
            })
        );
    }
    assert_eq!(
        items.next(),
        Some(&SessionEvent::ItemCompleted {
            item_id: tool.item_id.clone()
        })
    );
    assert_eq!(
        items.next(),
        Some(&SessionEvent::ItemCompleted {
            item_id: plan.item_id.clone()
        })
    );
    assert!(matches!(
        observed.iter().rev().nth(1),
        Some(SessionEvent::TurnEnded { .. })
    ));
    assert!(matches!(
        observed.last(),
        Some(SessionEvent::StateChanged {
            state: session_event_model::SessionState::Unloaded
        })
    ));
}

//! Failed tool and plan cursors must not complete inside a successor turn.

use super::*;
use session_event_model::{SessionItemKind, ToolCallStatus, TurnLostReason, TurnOutcome};

const FAILED_TOOL_PLAN_FIXTURE: &str = r#"
import json,pathlib,queue,socket,sys,threading
requests=queue.Queue()
prompt_texts=[]
stdin_closed=threading.Event()
receipt_path=pathlib.Path(sys.argv[2])
def receive_wire():
    try:
        for line in sys.stdin:
            message=json.loads(line)
            if message.get('method')=='session/prompt':
                prompt_texts.append(message['params']['prompt'][0]['text'])
                receipt_path.write_text(json.dumps({'promptCount':len(prompt_texts),'promptTexts':prompt_texts}))
            requests.put(message)
    finally:
        stdin_closed.set()
def read(): return requests.get(timeout=5)
def send(value): print(json.dumps(value),flush=True)
def update(value):
    send({'jsonrpc':'2.0','method':'session/update','params':{
        'sessionId':'failed-tool-plan-session','update':value}})
threading.Thread(target=receive_wire,daemon=True).start()
control=socket.socket(socket.AF_UNIX,socket.SOCK_STREAM)
control.settimeout(15)
control.connect(sys.argv[1])
request=read()
assert request['method']=='initialize',request
send({'jsonrpc':'2.0','id':request['id'],'result':{'protocolVersion':1,
    'agentCapabilities':{},'agentInfo':{'name':'failed-tool-plan-fixture','version':'1'}}})
request=read()
assert request['method']=='session/new',request
send({'jsonrpc':'2.0','id':request['id'],'result':{'sessionId':'failed-tool-plan-session'}})
first=read()
assert first['method']=='session/prompt' and first['params']['prompt']==[{'type':'text','text':'FIRST_REQUEST'}],first
update({'sessionUpdate':'agent_message_chunk','content':{'type':'text','text':'FIRST_PARTIAL'}})
update({'sessionUpdate':'tool_call','toolCallId':'failed-tool',
        'title':'FIRST_TOOL','kind':'execute','status':'pending'})
update({'sessionUpdate':'plan','entries':[{
        'content':'FIRST_PLAN','priority':'medium','status':'pending'}]})
# Release only after the actual actor emits the first text, tool, and plan.
assert control.recv(1)==b'e'
send({'jsonrpc':'2.0','id':first['id'],'error':{'code':-32603,'message':'first fixture prompt failed'}})
# The idle update deliberately omits kind/title, so retained lookup supplies them.
assert control.recv(1)==b't'
update({'sessionUpdate':'tool_call_update','toolCallId':'failed-tool','status':'completed'})
assert control.recv(1)==b's'
second=read()
assert second['method']=='session/prompt' and second['params']['prompt']==[{'type':'text','text':'SECOND_REQUEST'}],second
update({'sessionUpdate':'plan','entries':[{
        'content':'SECOND_PLAN','priority':'medium','status':'pending'}]})
update({'sessionUpdate':'agent_message_chunk','content':{'type':'text','text':'SECOND_ONLY'}})
send({'jsonrpc':'2.0','id':second['id'],'result':{'stopReason':'end_turn'}})
control.sendall(b'd')
assert control.recv(1)==b'q'
control.close()
assert stdin_closed.wait(timeout=15),'owner did not close fixture stdin'
"#;

async fn collect_until(
    events: &mut tokio::sync::mpsc::UnboundedReceiver<SessionEvent>,
    observed: &mut Vec<SessionEvent>,
    matches_event: impl Fn(&SessionEvent) -> bool,
) {
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let event = events
                .recv()
                .await
                .expect("actual item emitter remains open");
            let reached = matches_event(&event);
            observed.push(event);
            if reached {
                break;
            }
        }
    })
    .await
    .expect("bounded actual event observation");
}

#[tokio::test]
async fn failed_tool_plan_cursors_are_not_successor_turn_items() {
    let root = tempfile::tempdir_in("/tmp").expect("private short-path fixture root");
    let control_path = root.path().join("control.sock");
    let receipt_path = root.path().join("wire-prompts.json");
    let control_listener = tokio::net::UnixListener::bind(&control_path).expect("control socket");
    let (event_sender, mut events) = tokio::sync::mpsc::unbounded_channel();
    let client = Arc::new(
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            AgentSessionClient::initialize(
                ExternalProviderLaunch {
                    executable: PathBuf::from("/usr/bin/python3"),
                    arguments: vec![
                        "-u".into(),
                        "-c".into(),
                        FAILED_TOOL_PLAN_FIXTURE.into(),
                        control_path.to_string_lossy().into_owned(),
                        receipt_path.to_string_lossy().into_owned(),
                    ],
                    environment: Vec::new(),
                    persistence_target: crate::ProviderPersistenceTarget::Unspecified,
                },
                Arc::new(NoopInteractionPort),
                Arc::new(CapturedEventSink(event_sender)),
            ),
        )
        .await
        .expect("bounded fixture initialize")
        .expect("fixture initializes"),
    );
    let (mut control, _) =
        tokio::time::timeout(std::time::Duration::from_secs(2), control_listener.accept())
            .await
            .expect("bounded fixture control connection")
            .expect("control connection");
    let session_id = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        client.create_session(root.path().to_path_buf()),
    )
    .await
    .expect("bounded session creation")
    .expect("session opens");
    let first_input = InputId::generate();
    let second_input = InputId::generate();
    let first_client = Arc::clone(&client);
    let first_session = session_id.clone();
    let sent_first_input = first_input.clone();
    let first = tokio::spawn(async move {
        first_client
            .prompt_content(
                first_session,
                sent_first_input,
                Some(101),
                vec![text_block("FIRST_REQUEST")],
                None,
            )
            .await
    });
    let mut observed = Vec::new();
    collect_until(&mut events, &mut observed, |event| {
        matches!(event,
        SessionEvent::ItemStarted { item } if item.kind == SessionItemKind::Plan
            && item.text.as_deref() == Some("- Pending: FIRST_PLAN"))
    })
    .await;
    control
        .write_all(b"e")
        .await
        .expect("release original structured error");
    let first_result = tokio::time::timeout(std::time::Duration::from_secs(2), first)
        .await
        .expect("bounded first prompt settlement")
        .expect("first task joins");
    collect_until(&mut events, &mut observed, |event| {
        matches!(event, SessionEvent::TurnEnded { .. })
    })
    .await;
    let connection_survived_error = !client.retirement().is_cancelled();
    control
        .write_all(b"t")
        .await
        .expect("release old tool lookup update");
    collect_until(&mut events, &mut observed, |event| matches!(event,
        SessionEvent::ItemUpdated { item } if item.item_id == "failed-tool"
            && matches!(item.kind, SessionItemKind::ToolCall { status: ToolCallStatus::Completed, .. }))).await;
    let second_client = Arc::clone(&client);
    let sent_second_input = second_input.clone();
    let second = tokio::spawn(async move {
        second_client
            .prompt_content(
                session_id,
                sent_second_input,
                Some(202),
                vec![text_block("SECOND_REQUEST")],
                None,
            )
            .await
    });
    control
        .write_all(b"s")
        .await
        .expect("release explicit successor");
    let second_result = tokio::time::timeout(std::time::Duration::from_secs(2), second)
        .await
        .expect("bounded successor settlement")
        .expect("successor task joins");
    collect_until(&mut events, &mut observed, |event| {
        matches!(event, SessionEvent::TurnEnded { .. })
    })
    .await;
    let mut completed = [0u8; 1];
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        control.read_exact(&mut completed),
    )
    .await
    .expect("bounded fixture receipt")
    .expect("fixture completion receipt");
    control
        .write_all(b"q")
        .await
        .expect("release fixture idle phase");
    tokio::time::timeout(std::time::Duration::from_secs(5), client.shutdown())
        .await
        .expect("owned fixture actor and child shutdown");
    let wire: serde_json::Value =
        serde_json::from_slice(&std::fs::read(receipt_path).expect("wire prompt receipt"))
            .expect("wire receipt JSON");

    assert_eq!(completed, *b"d");
    assert_ne!(first_input, second_input);
    assert!(
        connection_survived_error,
        "structured error leaves transport alive"
    );
    assert!(matches!(
        first_result,
        Err(ExternalProviderRuntimeError::ProviderRejected { code: -32603, .. })
    ));
    assert_eq!(
        wire,
        serde_json::json!({"promptCount":2,
        "promptTexts":["FIRST_REQUEST","SECOND_REQUEST"]})
    );
    assert_eq!(
        second_result.expect("successor succeeds").output,
        "SECOND_ONLY"
    );
    assert_eq!(
        observed
            .iter()
            .filter(|event| matches!(event, SessionEvent::TurnEnded { .. }))
            .count(),
        2
    );
    let first_end = observed
        .iter()
        .position(|event| matches!(event, SessionEvent::TurnEnded { .. }))
        .expect("first turn terminal");
    assert!(matches!(
        observed[first_end],
        SessionEvent::TurnEnded {
            outcome: TurnOutcome::Lost {
                reason: TurnLostReason::ProviderTurnFailed
            },
            ..
        }
    ));
    let first_items = &observed[..first_end];
    assert!(first_items.iter().any(|event| matches!(event,
        SessionEvent::ItemStarted { item } if item.text.as_deref() == Some("FIRST_PARTIAL"))));
    assert!(first_items.iter().any(|event| matches!(event,
        SessionEvent::ItemStarted { item } if item.item_id == "failed-tool"
            && item.text.as_deref() == Some("FIRST_TOOL")
            && item.kind == (SessionItemKind::ToolCall { tool_kind:"execute".into(), status:ToolCallStatus::Pending }))));
    let first_plan = first_items
        .iter()
        .find_map(|event| match event {
            SessionEvent::ItemStarted { item } if item.kind == SessionItemKind::Plan => Some(item),
            _ => None,
        })
        .expect("actual first plan observed before original error");
    let second_start = observed
        .iter()
        .position(|event| {
            matches!(event,
        SessionEvent::TurnStarted { input_id, .. } if input_id == &second_input)
        })
        .expect("distinct successor admitted");
    assert!(observed[first_end..second_start].iter().any(|event| matches!(event,
        SessionEvent::ItemUpdated { item } if item.item_id == "failed-tool"
            && item.text.as_deref() == Some("FIRST_TOOL")
            && item.kind == (SessionItemKind::ToolCall { tool_kind:"execute".into(), status:ToolCallStatus::Completed }))),
        "idle tool update must retain actual kind/title lookup without a new tool item");
    let successor_items = &observed[second_start..];
    assert!(!successor_items.iter().any(|event| matches!(event,
        SessionEvent::ItemCompleted { item_id } if item_id == "failed-tool" || item_id == &first_plan.item_id)),
        "failed tool/plan completion crossed successor admission; old plan {}; actual successor events: {successor_items:?}", first_plan.item_id);
    assert!(
        successor_items.iter().any(|event| matches!(event,
        SessionEvent::ItemStarted { item } if item.kind == SessionItemKind::Plan
            && item.item_id != first_plan.item_id
            && item.text.as_deref() == Some("- Pending: SECOND_PLAN"))),
        "successor plan must start a distinct SECOND_PLAN item: {successor_items:?}"
    );
}

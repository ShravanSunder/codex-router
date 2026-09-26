use super::*;
use collaboration_protocol::ProviderPromptStopReason;
use std::path::PathBuf;

#[cfg(unix)]
fn replay_fixture(update_count: usize) -> ExternalProviderLaunch {
    let fixture = format!(
        r#"
import json,sys,time
request=json.loads(sys.stdin.readline())
print(json.dumps({{'jsonrpc':'2.0','id':request['id'],'result':{{'protocolVersion':1,'agentCapabilities':{{'loadSession':True}},'agentInfo':{{'name':'replay-fixture','version':'1'}}}}}})); sys.stdout.flush()
request=json.loads(sys.stdin.readline())
assert request['method']=='session/load'
for index in range({update_count}):
    update={{'jsonrpc':'2.0','method':'session/update','params':{{'sessionId':'fixture-session','update':{{'sessionUpdate':'agent_message_chunk','content':{{'type':'text','text':'historic-'+str(index)}}}}}}}}
    print(json.dumps(update))
print(json.dumps({{'jsonrpc':'2.0','id':request['id'],'result':{{}}}})); sys.stdout.flush()
request=json.loads(sys.stdin.readline())
assert request['method']=='session/prompt'
update={{'jsonrpc':'2.0','method':'session/update','params':{{'sessionId':'fixture-session','update':{{'sessionUpdate':'agent_message_chunk','content':{{'type':'text','text':'CURRENT_OUTPUT'}}}}}}}}
print(json.dumps(update)); print(json.dumps({{'jsonrpc':'2.0','id':request['id'],'result':{{'stopReason':'end_turn'}}}})); sys.stdout.flush()
sys.stdin.read()
"#
    );
    ExternalProviderLaunch {
        executable: PathBuf::from("/usr/bin/python3"),
        arguments: vec!["-c".to_owned(), fixture],
        environment: vec![],
    }
}

#[cfg(unix)]
fn many_prompt_updates_fixture(update_count: usize) -> ExternalProviderLaunch {
    let fixture = format!(
        r#"
import json,sys
request=json.loads(sys.stdin.readline())
print(json.dumps({{'jsonrpc':'2.0','id':request['id'],'result':{{'protocolVersion':1,'agentCapabilities':{{}},'agentInfo':{{'name':'many-updates-fixture','version':'1'}}}}}})); sys.stdout.flush()
request=json.loads(sys.stdin.readline())
print(json.dumps({{'jsonrpc':'2.0','id':request['id'],'result':{{'sessionId':'fixture-session'}}}})); sys.stdout.flush()
request=json.loads(sys.stdin.readline())
assert request['method']=='session/prompt'
for index in range({update_count}):
    update={{'jsonrpc':'2.0','method':'session/update','params':{{'sessionId':'fixture-session','update':{{'sessionUpdate':'agent_message_chunk','content':{{'type':'text','text':'x'}}}}}}}}
    print(json.dumps(update))
done={{'jsonrpc':'2.0','method':'session/update','params':{{'sessionId':'fixture-session','update':{{'sessionUpdate':'agent_message_chunk','content':{{'type':'text','text':'DONE'}}}}}}}}
print(json.dumps(done)); print(json.dumps({{'jsonrpc':'2.0','id':request['id'],'result':{{'stopReason':'end_turn'}}}})); sys.stdout.flush()
sys.stdin.read()
"#
    );
    ExternalProviderLaunch {
        executable: PathBuf::from("/usr/bin/python3"),
        arguments: vec!["-c".to_owned(), fixture],
        environment: vec![],
    }
}

#[cfg(unix)]
fn output_limit_and_late_update_fixture() -> ExternalProviderLaunch {
    let fixture = r#"
import json,sys,time
request=json.loads(sys.stdin.readline())
print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':{'protocolVersion':1,'agentCapabilities':{},'agentInfo':{'name':'output-limit-fixture','version':'1'}}})); sys.stdout.flush()
request=json.loads(sys.stdin.readline())
print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':{'sessionId':'fixture-session'}})); sys.stdout.flush()
first=json.loads(sys.stdin.readline())
assert first['method']=='session/prompt'
oversized={'jsonrpc':'2.0','method':'session/update','params':{'sessionId':'fixture-session','update':{'sessionUpdate':'agent_message_chunk','content':{'type':'text','text':'x'*(1024*1024+1)}}}}
print(json.dumps(oversized)); sys.stdout.flush()
cancel=json.loads(sys.stdin.readline())
assert cancel['method']=='session/cancel'
print(json.dumps({'jsonrpc':'2.0','id':first['id'],'result':{'stopReason':'cancelled'}})); sys.stdout.flush()
late={'jsonrpc':'2.0','method':'session/update','params':{'sessionId':'fixture-session','update':{'sessionUpdate':'agent_message_chunk','content':{'type':'text','text':'LATE_OUTPUT'}}}}
print(json.dumps(late)); sys.stdout.flush()
time.sleep(0.05)
second=json.loads(sys.stdin.readline())
assert second['method']=='session/prompt'
current={'jsonrpc':'2.0','method':'session/update','params':{'sessionId':'fixture-session','update':{'sessionUpdate':'agent_message_chunk','content':{'type':'text','text':'CURRENT_OUTPUT'}}}}
print(json.dumps(current)); print(json.dumps({'jsonrpc':'2.0','id':second['id'],'result':{'stopReason':'end_turn'}})); sys.stdout.flush()
sys.stdin.read()
"#;
    ExternalProviderLaunch {
        executable: PathBuf::from("/usr/bin/python3"),
        arguments: vec!["-c".to_owned(), fixture.to_owned()],
        environment: vec![],
    }
}

#[cfg(unix)]
fn unknown_request_fixture() -> ExternalProviderLaunch {
    let fixture = r#"
import json,sys
def read(): return json.loads(sys.stdin.readline())
def send(value): print(json.dumps(value)); sys.stdout.flush()
request=read()
send({'jsonrpc':'2.0','id':request['id'],'result':{'protocolVersion':1,'agentCapabilities':{},'agentInfo':{'name':'unknown-request-fixture','version':'1'}}})
request=read()
assert request['method']=='session/new'
send({'jsonrpc':'2.0','id':request['id'],'result':{'sessionId':'fixture-session'}})
send({'jsonrpc':'2.0','id':91,'method':'session/unknown','params':{'sessionId':'fixture-session'}})
idle_response=None
prompt=None
while idle_response is None or prompt is None:
    message=read()
    if message.get('method')=='session/prompt':
        prompt=message
    elif message.get('id')==91 and 'error' in message:
        idle_response=message
    else:
        raise AssertionError('unexpected message while awaiting unknown-request reply and prompt')
send({'jsonrpc':'2.0','id':91,'method':'session/unknown','params':{'sessionId':'fixture-session'}})
active_response=read()
report=json.dumps([idle_response,active_response])
send({'jsonrpc':'2.0','method':'session/update','params':{'sessionId':'fixture-session','update':{'sessionUpdate':'agent_message_chunk','content':{'type':'text','text':report}}}})
send({'jsonrpc':'2.0','id':prompt['id'],'result':{'stopReason':'end_turn'}})
sys.stdin.read()
"#;
    ExternalProviderLaunch {
        executable: PathBuf::from("/usr/bin/python3"),
        arguments: vec!["-c".to_owned(), fixture.to_owned()],
        environment: vec![],
    }
}

#[cfg(unix)]
fn sessionless_unknown_request_fixture() -> ExternalProviderLaunch {
    let fixture = r#"
import json,sys
def read(): return json.loads(sys.stdin.readline())
def send(value): print(json.dumps(value)); sys.stdout.flush()
request=read()
send({'jsonrpc':'2.0','id':request['id'],'result':{'protocolVersion':1,'agentCapabilities':{},'agentInfo':{'name':'sessionless-request-fixture','version':'1'}}})
send({'jsonrpc':'2.0','id':91,'method':'unknown/sessionless','params':{}})
response=None
prompt=None
while response is None or prompt is None:
    message=read()
    if message.get('method')=='session/new':
        send({'jsonrpc':'2.0','id':message['id'],'result':{'sessionId':'fixture-session'}})
    elif message.get('method')=='session/prompt':
        prompt=message
    elif message.get('id')==91 and 'error' in message:
        response=message
    else:
        raise AssertionError('unexpected message while awaiting sessionless reply and prompt')
report=json.dumps(response)
send({'jsonrpc':'2.0','method':'session/update','params':{'sessionId':'fixture-session','update':{'sessionUpdate':'agent_message_chunk','content':{'type':'text','text':report}}}})
send({'jsonrpc':'2.0','id':prompt['id'],'result':{'stopReason':'end_turn'}})
sys.stdin.read()
"#;
    ExternalProviderLaunch {
        executable: PathBuf::from("/usr/bin/python3"),
        arguments: vec!["-c".to_owned(), fixture.to_owned()],
        environment: vec![],
    }
}

#[cfg(unix)]
fn large_provider_frames_fixture() -> ExternalProviderLaunch {
    let fixture = r#"
import json,sys
def read(): return json.loads(sys.stdin.readline())
def send(value): print(json.dumps(value)); sys.stdout.flush()
request=read()
send({'jsonrpc':'2.0','id':request['id'],'result':{'protocolVersion':1,'agentCapabilities':{},'agentInfo':{'name':'large-frames-fixture','version':'1'}}})
for session_id in ['large-session','second-session']:
    request=read()
    assert request['method']=='session/new'
    send({'jsonrpc':'2.0','id':request['id'],'result':{'sessionId':session_id}})
prompts=[read(),read()]
assert all(request['method']=='session/prompt' for request in prompts)
prompt_by_session={request['params']['sessionId']:request for request in prompts}
first=prompt_by_session['large-session']
second=prompt_by_session['second-session']
send({'jsonrpc':'2.0','method':'session/update','params':{'sessionId':'large-session','update':{'sessionUpdate':'tool_call','toolCallId':'large-tool','title':'large result','kind':'other','status':'in_progress'}}})
large='x'*(2*1024*1024)
send({'jsonrpc':'2.0','method':'session/update','params':{'sessionId':'large-session','update':{'sessionUpdate':'tool_call_update','toolCallId':'large-tool','fields':{'rawOutput':{'payload':large},'status':'completed'}}}})
send({'jsonrpc':'2.0','id':first['id'],'result':{'stopReason':'end_turn','_meta':{'payload':large}}})
send({'jsonrpc':'2.0','method':'session/update','params':{'sessionId':'second-session','update':{'sessionUpdate':'agent_message_chunk','content':{'type':'text','text':'SECOND_SESSION_OK'}}}})
send({'jsonrpc':'2.0','id':second['id'],'result':{'stopReason':'end_turn'}})
sys.stdin.read()
"#;
    ExternalProviderLaunch {
        executable: PathBuf::from("/usr/bin/python3"),
        arguments: vec!["-c".to_owned(), fixture.to_owned()],
        environment: vec![],
    }
}

#[cfg(unix)]
fn large_permission_frame_fixture() -> ExternalProviderLaunch {
    let fixture = r#"
import json,sys
def read(): return json.loads(sys.stdin.readline())
def send(value): print(json.dumps(value)); sys.stdout.flush()
request=read()
send({'jsonrpc':'2.0','id':request['id'],'result':{'protocolVersion':1,'agentCapabilities':{},'agentInfo':{'name':'large-permission-fixture','version':'1'}}})
request=read()
send({'jsonrpc':'2.0','id':request['id'],'result':{'sessionId':'fixture-session'}})
prompt=read()
large='x'*(2*1024*1024)
send({'jsonrpc':'2.0','id':91,'method':'session/request_permission','params':{'sessionId':'fixture-session','toolCall':{'toolCallId':'large-tool','title':'large permission','kind':'execute','rawInput':{'payload':large}},'options':[{'optionId':'reject','name':'Reject','kind':'reject_once'}]}})
response=read()
assert response['id']==91
send({'jsonrpc':'2.0','id':prompt['id'],'result':{'stopReason':'end_turn'}})
sys.stdin.read()
"#;
    ExternalProviderLaunch {
        executable: PathBuf::from("/usr/bin/python3"),
        arguments: vec!["-c".to_owned(), fixture.to_owned()],
        environment: vec![],
    }
}

#[cfg(unix)]
fn oversized_active_frame_fixture() -> ExternalProviderLaunch {
    let fixture = format!(
        r#"
import json,sys,time
def read(): return json.loads(sys.stdin.readline())
def send(value): print(json.dumps(value)); sys.stdout.flush()
request=read()
send({{'jsonrpc':'2.0','id':request['id'],'result':{{'protocolVersion':1,'agentCapabilities':{{}},'agentInfo':{{'name':'frame-limit-fixture','version':'1'}}}}}})
request=read()
send({{'jsonrpc':'2.0','id':request['id'],'result':{{'sessionId':'fixture-session'}}}})
prompt=read()
time.sleep(0.1)
frame={{'jsonrpc':'2.0','method':'session/update','params':{{'sessionId':'fixture-session','update':{{'sessionUpdate':'agent_message_chunk','content':{{'type':'text','text':'x'*({} + 1)}}}}}}}}
send(frame)
sys.stdin.read()
"#,
        64 * 1024 * 1024,
    );
    ExternalProviderLaunch {
        executable: PathBuf::from("/usr/bin/python3"),
        arguments: vec!["-c".to_owned(), fixture],
        environment: vec![],
    }
}

#[cfg(unix)]
#[tokio::test]
async fn provider_prompt_observes_more_than_the_old_update_limit_and_settles() {
    let runtime = ExternalProviderRuntime::initialize(many_prompt_updates_fixture(4097))
        .await
        .expect("fixture initializes");
    runtime
        .create_session(PathBuf::from("/tmp"))
        .await
        .expect("session created");

    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        runtime.prompt("fixture-session".to_owned(), "many updates".to_owned()),
    )
    .await
    .expect("prompt settles")
    .expect("prompt succeeds");
    assert_eq!(outcome.output, "x".repeat(4097) + "DONE");
    assert_eq!(outcome.stop_reason, ProviderPromptStopReason::EndTurn);

    runtime.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn load_replay_drains_more_than_the_old_update_limit_without_leaking_history() {
    let runtime = ExternalProviderRuntime::initialize(replay_fixture(4097))
        .await
        .expect("fixture initializes");
    runtime
        .load_session("fixture-session".to_owned(), PathBuf::from("/tmp"))
        .await
        .expect("large replay loads");

    let outcome = runtime
        .prompt("fixture-session".to_owned(), "next".to_owned())
        .await
        .expect("following prompt settles");
    assert_eq!(outcome.output, "CURRENT_OUTPUT");

    runtime.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn load_replay_accepts_the_old_exact_update_boundary_without_history_leak() {
    let runtime = ExternalProviderRuntime::initialize(replay_fixture(4096))
        .await
        .expect("fixture initializes");
    runtime
        .load_session("fixture-session".to_owned(), PathBuf::from("/tmp"))
        .await
        .expect("exact-boundary replay loads");

    let outcome = runtime
        .prompt("fixture-session".to_owned(), "next".to_owned())
        .await
        .expect("following prompt settles");
    assert_eq!(outcome.output, "CURRENT_OUTPUT");

    runtime.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn output_limit_cancels_and_settles_before_late_updates_can_reach_next_prompt() {
    let runtime = ExternalProviderRuntime::initialize(output_limit_and_late_update_fixture())
        .await
        .expect("fixture initializes");
    runtime
        .create_session(PathBuf::from("/tmp"))
        .await
        .expect("session created");

    let error = runtime
        .prompt("fixture-session".to_owned(), "overflow".to_owned())
        .await
        .expect_err("retained output limit cancels prompt");
    assert!(matches!(
        error,
        ExternalProviderRuntimeError::PromptOutputLimitExceeded
    ));
    let next = runtime
        .prompt("fixture-session".to_owned(), "next".to_owned())
        .await
        .expect("next prompt settles");
    assert_eq!(next.output, "CURRENT_OUTPUT");

    runtime.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn unknown_session_requests_are_rejected_idle_and_during_prompt() {
    let runtime = ExternalProviderRuntime::initialize(unknown_request_fixture())
        .await
        .expect("fixture initializes");
    runtime
        .create_session(PathBuf::from("/tmp"))
        .await
        .expect("session created");

    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        runtime.prompt("fixture-session".to_owned(), "answer".to_owned()),
    )
    .await
    .expect("unknown requests do not park session actor")
    .expect("prompt settles");
    let responses: serde_json::Value =
        serde_json::from_str(&outcome.output).expect("request responses");
    for response in responses.as_array().expect("response list") {
        assert_eq!(response["error"]["code"], -32601, "{response}");
    }

    runtime.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn unknown_sessionless_requests_receive_method_not_found() {
    let runtime = ExternalProviderRuntime::initialize(sessionless_unknown_request_fixture())
        .await
        .expect("fixture initializes");
    runtime
        .create_session(PathBuf::from("/tmp"))
        .await
        .expect("session created");
    let outcome = runtime
        .prompt("fixture-session".to_owned(), "answer".to_owned())
        .await
        .expect("prompt settles");
    let response: serde_json::Value = serde_json::from_str(&outcome.output).expect("response");
    assert_eq!(response["error"]["code"], -32601, "{response}");
    runtime.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn request_naming_unknown_session_receives_method_not_found() {
    // ACP v1 JSON-RPC requests require a response. Specification R3 requires
    // -32601 for a request Router does not implement, even for an unknown ID.
    let root = tempfile::tempdir().expect("fixture root");
    let marker = root.path().join("unknown-session-answered");
    let fixture = acp_scripted_fixture::AcpFixtureScript::new()
        .expect_request("initialize", "initialize", serde_json::json!({"protocolVersion": 1}))
        .respond("initialize", serde_json::json!({"protocolVersion": 1, "agentCapabilities": {}, "agentInfo": {"name": "unknown-session-fixture", "version": "1"}}))
        .expect_request("create", "session/new", serde_json::json!({}))
        .respond("create", serde_json::json!({"sessionId": "fixture-session"}))
        .send(serde_json::json!({"jsonrpc": "2.0", "id": 91, "method": "session/unknown", "params": {"sessionId": "not-loaded"}}))
        .expect_message(serde_json::json!({"jsonrpc": "2.0", "id": 91, "error": {"code": -32601}}))
        .write_marker(&marker)
        .record_diagnostics(root.path().join("fixture-diagnostics.txt"))
        .launch();
    let runtime = ExternalProviderRuntime::initialize(fixture)
        .await
        .expect("fixture initializes");
    runtime
        .create_session(root.path().to_owned())
        .await
        .expect("session created");
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while !marker.exists() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "unknown session request was not answered: {}",
            std::fs::read_to_string(root.path().join("fixture-diagnostics.txt"))
                .unwrap_or_default()
        )
    });
    runtime.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn implemented_permission_request_during_load_replay_is_answered() {
    // ACP v1 session-setup.mdx:134-161 permits updates during session/load;
    // Specification R3 keeps implemented requests live during that replay.
    let fixture = acp_scripted_fixture::AcpFixtureScript::new()
        .expect_request("initialize", "initialize", serde_json::json!({"protocolVersion": 1}))
        .respond("initialize", serde_json::json!({"protocolVersion": 1, "agentCapabilities": {"loadSession": true}, "agentInfo": {"name": "load-permission-fixture", "version": "1"}}))
        .expect_request("load", "session/load", serde_json::json!({"sessionId": "fixture-session"}))
        .send(serde_json::json!({"jsonrpc": "2.0", "id": 91, "method": "session/request_permission", "params": {"sessionId": "fixture-session", "toolCall": {"toolCallId": "replay-tool", "title": "Replay permission", "kind": "execute"}, "options": [{"optionId": "reject", "name": "Reject", "kind": "reject_once"}]}}))
        .expect_message(serde_json::json!({"jsonrpc": "2.0", "id": 91, "result": {"outcome": {"outcome": "cancelled"}}}))
        .respond("load", serde_json::json!({}))
        .launch();
    let runtime = ExternalProviderRuntime::initialize(fixture)
        .await
        .expect("fixture initializes");
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        runtime.load_session("fixture-session".to_owned(), PathBuf::from("/tmp")),
    )
    .await
    .expect("permission request does not block load")
    .expect("session load succeeds");
    let observation = runtime.permission_observation();
    assert_eq!(observation.request_count, 1);
    assert_eq!(
        observation.last_outcome,
        Some(ExternalProviderPermissionOutcome::Cancelled)
    );
    runtime.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn permission_request_naming_unknown_session_receives_method_not_found() {
    // Specification R3: unknown session IDs receive -32601 even when the
    // method is implemented for loaded or currently loading Sessions.
    let root = tempfile::tempdir().expect("fixture root");
    let marker = root.path().join("unknown-permission-answered");
    let fixture = acp_scripted_fixture::AcpFixtureScript::new()
        .expect_request("initialize", "initialize", serde_json::json!({"protocolVersion": 1}))
        .respond("initialize", serde_json::json!({"protocolVersion": 1, "agentCapabilities": {}, "agentInfo": {"name": "unknown-permission-fixture", "version": "1"}}))
        .expect_request("create", "session/new", serde_json::json!({}))
        .respond("create", serde_json::json!({"sessionId": "fixture-session"}))
        .send(serde_json::json!({"jsonrpc": "2.0", "id": 92, "method": "session/request_permission", "params": {"sessionId": "not-loaded", "toolCall": {"toolCallId": "unknown-tool", "title": "Unknown session permission", "kind": "execute"}, "options": [{"optionId": "reject", "name": "Reject", "kind": "reject_once"}]}}))
        .expect_message(serde_json::json!({"jsonrpc": "2.0", "id": 92, "error": {"code": -32601}}))
        .write_marker(&marker)
        .record_diagnostics(root.path().join("fixture-diagnostics.txt"))
        .launch();
    let runtime = ExternalProviderRuntime::initialize(fixture)
        .await
        .expect("fixture initializes");
    runtime
        .create_session(root.path().to_owned())
        .await
        .expect("session created");
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while !marker.exists() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "unknown session permission was not rejected: {}",
            std::fs::read_to_string(root.path().join("fixture-diagnostics.txt"))
                .unwrap_or_default()
        )
    });
    runtime.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn large_tool_update_and_result_do_not_retire_other_provider_sessions() {
    let runtime = ExternalProviderRuntime::initialize(large_provider_frames_fixture())
        .await
        .expect("fixture initializes");
    runtime
        .create_session(PathBuf::from("/tmp"))
        .await
        .expect("first session created");
    runtime
        .create_session(PathBuf::from("/tmp"))
        .await
        .expect("second session created");

    let large_prompt = runtime.prompt("large-session".to_owned(), "large frames".to_owned());
    let second_prompt = runtime.prompt("second-session".to_owned(), "still alive".to_owned());
    let (large_result, second_result) = tokio::join!(large_prompt, second_prompt);
    let large_result = large_result.expect("large provider frames accepted");
    assert!(large_result.output.is_empty());
    assert_eq!(
        second_result
            .expect("second session remains available")
            .output,
        "SECOND_SESSION_OK"
    );

    runtime.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn large_permission_request_frame_is_accepted_and_answered() {
    let runtime = ExternalProviderRuntime::initialize(large_permission_frame_fixture())
        .await
        .expect("fixture initializes");
    runtime
        .create_session(PathBuf::from("/tmp"))
        .await
        .expect("session created");

    runtime
        .prompt("fixture-session".to_owned(), "large permission".to_owned())
        .await
        .expect("large permission request is answered");

    runtime.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn frame_limit_breach_returns_a_safe_typed_failure() {
    let runtime = ExternalProviderRuntime::initialize(oversized_active_frame_fixture())
        .await
        .expect("fixture initializes");
    runtime
        .create_session(PathBuf::from("/tmp"))
        .await
        .expect("session created");

    let error = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        runtime.prompt(
            "fixture-session".to_owned(),
            "trigger frame breach".to_owned(),
        ),
    )
    .await
    .expect("frame breach terminates prompt")
    .expect_err("oversized provider frame is rejected");
    assert!(
        matches!(error, ExternalProviderRuntimeError::FrameLimitExceeded),
        "{error:?}"
    );

    runtime.shutdown().await;
}

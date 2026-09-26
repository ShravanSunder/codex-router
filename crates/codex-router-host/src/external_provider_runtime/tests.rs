use super::*;

#[test]
fn provider_initialize_errors_expose_only_safe_metadata() {
    let account_sentinel = "synthetic-account-sentinel@example.invalid";
    let token_sentinel = "synthetic-token-sentinel-7f4e";
    let error = agent_client_protocol::Error::new(-32001, format!("denied {token_sentinel}"))
        .data(serde_json::json!({"account": account_sentinel, "token": token_sentinel}));

    let diagnostic = sanitized_initialization_error(&error);

    assert!(diagnostic.contains("initialize"));
    assert!(diagnostic.contains("stage=initialize"));
    assert!(diagnostic.contains("-32001"));
    assert!(diagnostic.contains("error_data_bytes="));
    assert!(!diagnostic.contains(account_sentinel));
    assert!(!diagnostic.contains(token_sentinel));

    let operation_error = sanitized_acp_error(&error, "session/update", "load replay");
    assert!(operation_error.contains("method=session/update"));
    assert!(operation_error.contains("stage=load replay"));
    assert!(operation_error.contains("-32001"));
    assert!(!operation_error.contains(account_sentinel));
    assert!(!operation_error.contains(token_sentinel));
}

#[cfg(unix)]
fn python_fixture(
    response_version: u16,
    process_id_path: Option<&std::path::Path>,
) -> ExternalProviderLaunch {
    let record_process_id = process_id_path.map_or_else(String::new, |path| {
        format!(
            "open({:?},'w').write(str(__import__('os').getpid())); ",
            path.display().to_string()
        )
    });
    let fixture = format!(
        "import json,sys; {record_process_id}request=json.loads(sys.stdin.readline()); print(json.dumps({{'jsonrpc':'2.0','id':request['id'],'result':{{'protocolVersion':{response_version},'agentCapabilities':{{'loadSession':True}},'agentInfo':{{'name':'fixture-agent','version':'1.2.3'}}}}}})); sys.stdout.flush(); sys.stdin.read()"
    );
    ExternalProviderLaunch {
        executable: PathBuf::from("/usr/bin/python3"),
        arguments: vec!["-c".to_owned(), fixture],
        environment: vec![],
    }
}

#[cfg(unix)]
async fn assert_process_reaped(process_id_path: &std::path::Path) {
    let process_id = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Ok(value) = std::fs::read_to_string(process_id_path)
                && let Ok(process_id) = value.trim().parse::<i32>()
            {
                break process_id;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("fixture wrote process id");
    let process_id = rustix::process::Pid::from_raw(process_id).expect("positive process id");
    assert!(matches!(
        rustix::process::test_kill_process(process_id),
        Err(rustix::io::Errno::SRCH)
    ));
}

#[cfg(unix)]
fn conversation_fixture() -> ExternalProviderLaunch {
    let fixture = r#"
import json,sys
request=json.loads(sys.stdin.readline())
print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':{'protocolVersion':1,'agentCapabilities':{'loadSession':True},'agentInfo':{'name':'conversation-fixture','version':'1'}}})); sys.stdout.flush()
request=json.loads(sys.stdin.readline())
assert request['method']=='session/new'
print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':{'sessionId':'fixture-session'}})); sys.stdout.flush()
request=json.loads(sys.stdin.readline())
assert request['method']=='session/prompt'
print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':{'stopReason':'end_turn'}})); sys.stdout.flush()
sys.stdin.read()
"#;
    ExternalProviderLaunch {
        executable: PathBuf::from("/usr/bin/python3"),
        arguments: vec!["-c".to_owned(), fixture.to_owned()],
        environment: vec![],
    }
}

#[cfg(unix)]
fn steering_fixture(observed_prompt_socket: &std::path::Path) -> ExternalProviderLaunch {
    let fixture = format!(
        r#"
import json,socket,sys
request=json.loads(sys.stdin.readline())
print(json.dumps({{'jsonrpc':'2.0','id':request['id'],'result':{{'protocolVersion':1,'agentCapabilities':{{'loadSession':True}},'agentInfo':{{'name':'steering-fixture','version':'1'}},'_meta':{{'steering':{{'supported':True}}}}}}}})); sys.stdout.flush()
request=json.loads(sys.stdin.readline())
assert request['method']=='session/new'
print(json.dumps({{'jsonrpc':'2.0','id':request['id'],'result':{{'sessionId':'fixture-session'}}}})); sys.stdout.flush()
prompt=json.loads(sys.stdin.readline())
assert prompt['method']=='session/prompt'
notice=socket.socket(socket.AF_UNIX,socket.SOCK_STREAM)
notice.connect({observed_socket:?})
notice.sendall(b'prompt')
notice.close()
steer=json.loads(sys.stdin.readline())
assert steer['method']=='_session/steering'
assert steer['params']['sessionId']=='fixture-session'
assert steer['params']['prompt'][0]['text']=='follow-up'
assert steer['params']['_meta']['steering']['idleBehavior']=='promptRequired'
print(json.dumps({{'jsonrpc':'2.0','id':steer['id'],'result':{{'outcome':'injected'}}}})); sys.stdout.flush()
print(json.dumps({{'jsonrpc':'2.0','id':prompt['id'],'result':{{'stopReason':'end_turn'}}}})); sys.stdout.flush()
idle=json.loads(sys.stdin.readline())
assert idle['method']=='_session/steering'
print(json.dumps({{'jsonrpc':'2.0','id':idle['id'],'result':{{'outcome':'promptRequired','reason':'noRunningTurn'}}}})); sys.stdout.flush()
sys.stdin.read()
"#,
        observed_socket = observed_prompt_socket.display().to_string(),
    );
    ExternalProviderLaunch {
        executable: PathBuf::from("/usr/bin/python3"),
        arguments: vec!["-c".to_owned(), fixture],
        environment: vec![],
    }
}

#[cfg(unix)]
#[tokio::test]
async fn steering_injects_during_prompt_and_returns_prompt_required_when_idle() {
    let root = tempfile::tempdir().expect("fixture root");
    let marker = root.path().join("prompt.sock");
    let listener = tokio::net::UnixListener::bind(&marker).expect("prompt event listener");
    let runtime = std::sync::Arc::new(
        ExternalProviderRuntime::initialize(steering_fixture(&marker))
            .await
            .expect("provider runtime"),
    );
    assert!(runtime.admission().supports_steering);
    runtime
        .create_session(PathBuf::from("/tmp"))
        .await
        .expect("session/new");
    let running_operation_id =
        OperationId::try_from("018f1f62-6571-7ef0-8f0c-001122334499".to_owned())
            .expect("running operation ID");
    let prompt_runtime = std::sync::Arc::clone(&runtime);
    let prompt_operation_id = running_operation_id.clone();
    let (dispatch, dispatched) = tokio::sync::oneshot::channel();
    let prompt = tokio::spawn(async move {
        prompt_runtime
            .prompt_for_operation(
                "fixture-session".to_owned(),
                Some(prompt_operation_id),
                "first".to_owned(),
                Some(dispatch),
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), dispatched)
        .await
        .expect("prompt dispatch notification")
        .expect("prompt was sent before settlement");
    tokio::time::timeout(Duration::from_secs(2), listener.accept())
        .await
        .expect("prompt observed before steer")
        .expect("prompt notification");
    assert_eq!(
        runtime
            .session_activity("fixture-session".to_owned())
            .await
            .expect("active session state"),
        ProviderSessionActivity::Running
    );
    let wait_runtime = std::sync::Arc::clone(&runtime);
    let waiting = tokio::spawn(async move {
        wait_runtime
            .wait_session_idle("fixture-session".to_owned())
            .await
    });

    let injected = runtime
        .steer_session("fixture-session".to_owned(), "follow-up".to_owned())
        .await
        .expect("steer active turn");
    assert_eq!(
        injected,
        ProviderSteeringOutcome::Injected {
            running_operation_id: Some(running_operation_id),
        }
    );
    prompt
        .await
        .expect("prompt task")
        .expect("prompt settlement");
    waiting.await.expect("idle wait task").expect("idle event");
    assert_eq!(
        runtime
            .session_activity("fixture-session".to_owned())
            .await
            .expect("idle state"),
        ProviderSessionActivity::Idle
    );
    let idle = runtime
        .steer_session("fixture-session".to_owned(), "later".to_owned())
        .await
        .expect("steer idle session");
    assert_eq!(idle, ProviderSteeringOutcome::PromptRequired);
    runtime.shutdown().await;
}

#[cfg(unix)]
fn cancellation_fixture() -> ExternalProviderLaunch {
    let fixture = r#"
import json,sys
request=json.loads(sys.stdin.readline())
print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':{'protocolVersion':1,'agentCapabilities':{},'agentInfo':{'name':'cancel-fixture','version':'1'}}})); sys.stdout.flush()
request=json.loads(sys.stdin.readline())
print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':{'sessionId':'fixture-session'}})); sys.stdout.flush()
prompt=json.loads(sys.stdin.readline())
assert prompt['method']=='session/prompt'
cancel=json.loads(sys.stdin.readline())
assert cancel['method']=='session/cancel'
print(json.dumps({'jsonrpc':'2.0','id':prompt['id'],'result':{'stopReason':'cancelled'}})); sys.stdout.flush()
sys.stdin.read()
"#;
    ExternalProviderLaunch {
        executable: PathBuf::from("/usr/bin/python3"),
        arguments: vec!["-c".to_owned(), fixture.to_owned()],
        environment: vec![],
    }
}

#[cfg(unix)]
fn successor_prompt_fixture() -> ExternalProviderLaunch {
    let fixture = r#"
import json,sys,time
request=json.loads(sys.stdin.readline())
print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':{'protocolVersion':1,'agentCapabilities':{},'agentInfo':{'name':'successor-fixture','version':'1'}}})); sys.stdout.flush()
request=json.loads(sys.stdin.readline())
print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':{'sessionId':'fixture-session'}})); sys.stdout.flush()
first=json.loads(sys.stdin.readline())
print(json.dumps({'jsonrpc':'2.0','id':first['id'],'result':{'stopReason':'end_turn'}})); sys.stdout.flush()
second=json.loads(sys.stdin.readline())
time.sleep(0.15)
print(json.dumps({'jsonrpc':'2.0','id':second['id'],'result':{'stopReason':'end_turn'}})); sys.stdout.flush()
sys.stdin.read()
"#;
    ExternalProviderLaunch {
        executable: PathBuf::from("/usr/bin/python3"),
        arguments: vec!["-c".to_owned(), fixture.to_owned()],
        environment: vec![],
    }
}

#[cfg(unix)]
fn concurrent_admission_cancel_fixture(
    admission_method: &str,
    admission_observed: &std::path::Path,
) -> ExternalProviderLaunch {
    let observed = admission_observed.display().to_string();
    let fixture = format!(
        r#"
import json,sys
request=json.loads(sys.stdin.readline())
print(json.dumps({{'jsonrpc':'2.0','id':request['id'],'result':{{'protocolVersion':1,'agentCapabilities':{{'loadSession':True}},'agentInfo':{{'name':'concurrent-fixture','version':'1'}}}}}})); sys.stdout.flush()
first=json.loads(sys.stdin.readline())
print(json.dumps({{'jsonrpc':'2.0','id':first['id'],'result':{{'sessionId':'active-session'}}}})); sys.stdout.flush()
prompt=json.loads(sys.stdin.readline())
admission=json.loads(sys.stdin.readline())
assert admission['method']=={admission_method:?}
open({observed:?},'w').write('observed')
cancel=json.loads(sys.stdin.readline())
assert cancel['method']=='session/cancel'
print(json.dumps({{'jsonrpc':'2.0','id':prompt['id'],'result':{{'stopReason':'cancelled'}}}})); sys.stdout.flush()
admission_result={{'sessionId':'new-session'}} if admission['method']=='session/new' else {{}}
print(json.dumps({{'jsonrpc':'2.0','id':admission['id'],'result':admission_result}})); sys.stdout.flush()
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
fn saturated_load_admission_fixture(
    admission_log: &std::path::Path,
    process_id_path: &std::path::Path,
) -> ExternalProviderLaunch {
    let fixture = format!(
        r#"
import json,os,sys
open({process_id:?},'w').write(str(os.getpid()))
request=json.loads(sys.stdin.readline())
print(json.dumps({{'jsonrpc':'2.0','id':request['id'],'result':{{'protocolVersion':1,'agentCapabilities':{{'loadSession':True}},'agentInfo':{{'name':'saturated-load-fixture','version':'1'}}}}}})); sys.stdout.flush()
for line in sys.stdin:
    request=json.loads(line)
    assert request['method']=='session/load'
    with open({admission_log:?},'a') as log:
        log.write(request['params']['sessionId']+'\n')
"#,
        process_id = process_id_path.display().to_string(),
        admission_log = admission_log.display().to_string(),
    );
    ExternalProviderLaunch {
        executable: PathBuf::from("/usr/bin/python3"),
        arguments: vec!["-c".to_owned(), fixture],
        environment: vec![],
    }
}

#[cfg(unix)]
fn permission_request_fixture() -> ExternalProviderLaunch {
    let fixture = r#"
import json,sys
request=json.loads(sys.stdin.readline())
print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':{'protocolVersion':1,'agentCapabilities':{},'agentInfo':{'name':'permission-fixture','version':'1'}}})); sys.stdout.flush()
request=json.loads(sys.stdin.readline())
print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':{'sessionId':'fixture-session'}})); sys.stdout.flush()
prompt=json.loads(sys.stdin.readline())
assert prompt['method']=='session/prompt'
print(json.dumps({'jsonrpc':'2.0','id':91,'method':'session/request_permission','params':{'sessionId':'fixture-session','toolCall':{'toolCallId':'permission-tool'},'options':[{'optionId':'reject','name':'Reject','kind':'reject_once'}]}})); sys.stdout.flush()
permission_response=json.loads(sys.stdin.readline())
assert permission_response['id']==91
assert permission_response['result']['outcome']['outcome']=='cancelled'
print(json.dumps({'jsonrpc':'2.0','id':prompt['id'],'result':{'stopReason':'end_turn'}})); sys.stdout.flush()
sys.stdin.read()
"#;
    ExternalProviderLaunch {
        executable: PathBuf::from("/usr/bin/python3"),
        arguments: vec!["-c".to_owned(), fixture.to_owned()],
        environment: vec![],
    }
}

#[cfg(unix)]
fn named_mcp_tool_call_fixture(
    raw_output: Option<serde_json::Value>,
    response_text: &str,
    split_result_before_status: bool,
) -> ExternalProviderLaunch {
    let raw_output_setup = raw_output.map_or_else(
        || "raw_output=None".to_owned(),
        |value| {
            let encoded = serde_json::to_string(&value).expect("MCP result JSON");
            format!("raw_output=json.loads({encoded:?})")
        },
    );
    let split_python = if split_result_before_status {
        "True"
    } else {
        "False"
    };
    let fixture = format!(
        r#"
import json,sys
request=json.loads(sys.stdin.readline())
print(json.dumps({{'jsonrpc':'2.0','id':request['id'],'result':{{'protocolVersion':1,'agentCapabilities':{{}},'agentInfo':{{'name':'tool-call-fixture','version':'1'}}}}}})); sys.stdout.flush()
request=json.loads(sys.stdin.readline())
print(json.dumps({{'jsonrpc':'2.0','id':request['id'],'result':{{'sessionId':'fixture-session'}}}})); sys.stdout.flush()
prompt=json.loads(sys.stdin.readline())
print(json.dumps({{'jsonrpc':'2.0','method':'session/update','params':{{'sessionId':'fixture-session','update':{{'sessionUpdate':'tool_call','toolCallId':'call-1','title':'router-collaboration: endpoints_list','kind':'other'}}}}}})); sys.stdout.flush()
{raw_output_setup}
if raw_output is not None and {split_python}:
    print(json.dumps({{'jsonrpc':'2.0','method':'session/update','params':{{'sessionId':'fixture-session','update':{{'sessionUpdate':'tool_call_update','toolCallId':'call-1','name':'router-collaboration-endpoints_list','rawOutput':raw_output}}}}}})); sys.stdout.flush()
terminal={{'sessionUpdate':'tool_call_update','toolCallId':'call-1','name':'router-collaboration-endpoints_list','status':'completed'}}
if raw_output is not None and not {split_python}: terminal['rawOutput']=raw_output
print(json.dumps({{'jsonrpc':'2.0','method':'session/update','params':{{'sessionId':'fixture-session','update':terminal}}}})); sys.stdout.flush()
if {response_text:?}:
    response_update={{'sessionUpdate':'agent_message_chunk','content':{{'type':'text','text':{response_text:?}}}}}
    print(json.dumps({{'jsonrpc':'2.0','method':'session/update','params':{{'sessionId':'fixture-session','update':response_update}}}})); sys.stdout.flush()
print(json.dumps({{'jsonrpc':'2.0','id':prompt['id'],'result':{{'stopReason':'end_turn'}}}})); sys.stdout.flush()
sys.stdin.read()
"#
    );
    ExternalProviderLaunch {
        executable: PathBuf::from("/usr/bin/python3"),
        arguments: vec!["-c".to_owned(), fixture],
        environment: vec![],
    }
}

fn approval_context(operation_id: &str) -> ExternalProviderApprovalContext {
    let service_id = "0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89";
    ExternalProviderApprovalContext {
        requester: serde_json::from_value(serde_json::json!({"endpoint":{"serviceId":service_id,"endpointId":"cursor-local"},"sessionId":"requester"})).expect("requester"),
        approver: serde_json::from_value(serde_json::json!({"endpoint":{"serviceId":service_id,"endpointId":"codex-local"},"sessionId":"approver"})).expect("approver"),
        target: serde_json::from_value(serde_json::json!({"endpoint":{"serviceId":service_id,"endpointId":"cursor-local"},"sessionId":"fixture-session"})).expect("target"),
        operation_id: operation_id.to_owned().try_into().expect("operation ID"),
        binding_generation: serde_json::from_value(serde_json::json!({"serviceEpoch":service_id,"generation":1})).expect("generation"),
        binding_retirement: CancellationToken::new(),
    }
}

#[test]
fn acp_error_codes_classify_authentication_without_message_matching() {
    let mut authentication = agent_client_protocol::Error::auth_required();
    authentication.message = "provider conversation is busy".to_owned();
    assert!(matches!(
        acp_operation_error(authentication),
        ExternalProviderRuntimeError::AuthenticationRequired { code: -32000 }
    ));
    for message in [
        "provider conversation is busy",
        "has not been created or loaded",
        "has no active prompt",
        "is no longer active",
    ] {
        let mut provider = agent_client_protocol::Error::internal_error();
        provider.message = message.to_owned();
        provider.data = Some(serde_json::json!({"privateText":"must not escape"}));
        let reason = acp_operation_error(provider);
        assert!(matches!(
            &reason,
            ExternalProviderRuntimeError::ProviderRejected { code } if *code == -32603
        ));
        assert!(!reason.to_string().contains("must not escape"));
        assert!(!reason.to_string().contains(message));
    }

    let mut missing = agent_client_protocol::Error::new(-32002, "private missing-session text");
    missing.data = Some(serde_json::json!({"privateText":"must not escape"}));
    let reason = acp_operation_error(missing);
    assert!(matches!(
        &reason,
        ExternalProviderRuntimeError::ProviderSessionNotFound { code } if *code == -32002
    ));
    assert!(!reason.to_string().contains("private"));
}

#[cfg(unix)]
fn aggregate_output_fixture() -> ExternalProviderLaunch {
    let fixture = r#"
import json,sys
request=json.loads(sys.stdin.readline())
print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':{'protocolVersion':1,'agentCapabilities':{},'agentInfo':{'name':'aggregate-fixture','version':'1'}}})); sys.stdout.flush()
request=json.loads(sys.stdin.readline())
print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':{'sessionId':'fixture-session'}})); sys.stdout.flush()
prompt=json.loads(sys.stdin.readline())
chunk='x'*(600*1024)
update={'jsonrpc':'2.0','method':'session/update','params':{'sessionId':'fixture-session','update':{'sessionUpdate':'agent_message_chunk','content':{'type':'text','text':chunk}}}}
print(json.dumps(update)); print(json.dumps(update)); sys.stdout.flush()
cancel=json.loads(sys.stdin.readline())
assert cancel['method']=='session/cancel'
print(json.dumps({'jsonrpc':'2.0','id':prompt['id'],'result':{'stopReason':'cancelled'}})); sys.stdout.flush()
sys.stdin.read()
"#;
    ExternalProviderLaunch {
        executable: PathBuf::from("/usr/bin/python3"),
        arguments: vec!["-c".to_owned(), fixture.to_owned()],
        environment: vec![],
    }
}

#[cfg(unix)]
fn load_replay_fixture() -> ExternalProviderLaunch {
    let fixture = r#"
import json,sys
request=json.loads(sys.stdin.readline())
print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':{'protocolVersion':1,'agentCapabilities':{'loadSession':True},'agentInfo':{'name':'load-fixture','version':'1'}}})); sys.stdout.flush()
request=json.loads(sys.stdin.readline())
assert request['method']=='session/load'
old={'jsonrpc':'2.0','method':'session/update','params':{'sessionId':'fixture-session','update':{'sessionUpdate':'agent_message_chunk','content':{'type':'text','text':'OLD_HISTORY'}}}}
print(json.dumps(old)); print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':{}})); sys.stdout.flush()
request=json.loads(sys.stdin.readline())
assert request['method']=='session/prompt'
new={'jsonrpc':'2.0','method':'session/update','params':{'sessionId':'fixture-session','update':{'sessionUpdate':'agent_message_chunk','content':{'type':'text','text':'NEW_OUTPUT'}}}}
print(json.dumps(new)); print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':{'stopReason':'end_turn'}})); sys.stdout.flush()
sys.stdin.read()
"#;
    ExternalProviderLaunch {
        executable: PathBuf::from("/usr/bin/python3"),
        arguments: vec!["-c".to_owned(), fixture.to_owned()],
        environment: vec![],
    }
}

#[cfg(unix)]
fn mcp_session_setup_fixture(method: &str) -> ExternalProviderLaunch {
    let fixture = format!(
        r#"
import json,sys
request=json.loads(sys.stdin.readline())
print(json.dumps({{'jsonrpc':'2.0','id':request['id'],'result':{{'protocolVersion':1,'agentCapabilities':{{'loadSession':True,'mcpCapabilities':{{'http':True}}}},'agentInfo':{{'name':'mcp-fixture','version':'1'}}}}}})); sys.stdout.flush()
request=json.loads(sys.stdin.readline())
assert request['method']=={method:?}
assert request['params']['mcpServers']==[{{'type':'http','name':'router-collaboration','url':'http://127.0.0.1:19090/mcp','headers':[]}}]
result={{'sessionId':'fixture-session'}} if request['method']=='session/new' else {{}}
print(json.dumps({{'jsonrpc':'2.0','id':request['id'],'result':result}})); sys.stdout.flush()
sys.stdin.read()
"#
    );
    ExternalProviderLaunch {
        executable: PathBuf::from("/usr/bin/python3"),
        arguments: vec!["-c".to_owned(), fixture],
        environment: vec![],
    }
}

#[tokio::test]
async fn missing_executable_fails_before_runtime_admission() {
    let error = ExternalProviderRuntime::initialize(ExternalProviderLaunch {
        executable: PathBuf::from("/definitely/missing/codex-router-acp-provider"),
        arguments: vec![],
        environment: vec![],
    })
    .await
    .expect_err("missing executable must fail");

    assert!(matches!(error, ExternalProviderRuntimeError::Launch(_)));
}

#[cfg(unix)]
#[tokio::test]
async fn stable_v1_initialize_admits_runtime_and_capabilities() {
    let runtime = ExternalProviderRuntime::initialize(python_fixture(1, None))
        .await
        .expect("fixture initializes");

    assert_eq!(
        runtime.admission(),
        &ExternalProviderAdmission {
            runtime_name: Some("fixture-agent".to_owned()),
            runtime_version: Some("1.2.3".to_owned()),
            supports_load: true,
            supports_mcp_http: false,
            supports_steering: false,
        }
    );

    runtime.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn configured_router_mcp_is_injected_into_create_and_load_requests() {
    let create_runtime = ExternalProviderRuntime::initialize_with_mcp_http(
        mcp_session_setup_fixture("session/new"),
        "router-collaboration",
        "http://127.0.0.1:19090/mcp",
    )
    .await
    .expect("create fixture initializes");
    assert!(create_runtime.admission().supports_mcp_http);
    assert_eq!(
        create_runtime
            .create_session(PathBuf::from("/tmp"))
            .await
            .expect("create with MCP binding"),
        "fixture-session"
    );
    create_runtime.shutdown().await;

    let load_runtime = ExternalProviderRuntime::initialize_with_mcp_http(
        mcp_session_setup_fixture("session/load"),
        "router-collaboration",
        "http://127.0.0.1:19090/mcp",
    )
    .await
    .expect("load fixture initializes");
    assert!(load_runtime.admission().supports_mcp_http);
    load_runtime
        .load_session("fixture-session".to_owned(), PathBuf::from("/tmp"))
        .await
        .expect("load with MCP binding");
    load_runtime.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn unsupported_protocol_version_is_rejected() {
    let fixture_root = tempfile::tempdir().expect("fixture root");
    let process_id_path = fixture_root.path().join("provider.pid");
    let error = ExternalProviderRuntime::initialize(python_fixture(0, Some(&process_id_path)))
        .await
        .expect_err("v0 must not be admitted");

    assert!(matches!(
        error,
        ExternalProviderRuntimeError::UnsupportedProtocol {
            actual: ProtocolVersion::V0
        }
    ));
    assert_process_reaped(&process_id_path).await;
}

#[cfg(unix)]
#[tokio::test]
async fn clean_eof_before_initialize_is_a_typed_failure() {
    let error = ExternalProviderRuntime::initialize(ExternalProviderLaunch {
        executable: PathBuf::from("/bin/sh"),
        arguments: vec!["-c".to_owned(), "exit 0".to_owned()],
        environment: vec![],
    })
    .await
    .expect_err("EOF must fail initialization");

    assert!(matches!(error, ExternalProviderRuntimeError::Initialize(_)));
}

#[cfg(unix)]
#[tokio::test]
async fn stdout_eof_after_initialize_retires_live_provider() {
    let fixture_root = tempfile::tempdir().expect("fixture root");
    let process_id_path = fixture_root.path().join("provider.pid");
    let close_marker = fixture_root.path().join("close-stdout");
    let fixture = format!(
        "import json,os,sys,time; open({:?},'w').write(str(os.getpid())); request=json.loads(sys.stdin.readline()); print(json.dumps({{'jsonrpc':'2.0','id':request['id'],'result':{{'protocolVersion':1,'agentCapabilities':{{}},'agentInfo':{{'name':'eof-fixture','version':'1'}}}}}})); sys.stdout.flush(); marker={:?};
while not os.path.exists(marker): time.sleep(0.01)
os.close(1); sys.stdin.read()",
        process_id_path.display().to_string(), close_marker.display().to_string()
    );
    let runtime = ExternalProviderRuntime::initialize(ExternalProviderLaunch {
        executable: PathBuf::from("/usr/bin/python3"),
        arguments: vec!["-c".to_owned(), fixture],
        environment: vec![],
    })
    .await
    .expect("fixture initializes before stdout closes");
    std::fs::write(&close_marker, b"close").expect("signal fixture to close stdout");

    tokio::time::timeout(Duration::from_secs(2), runtime.retirement().cancelled())
        .await
        .expect("stdout EOF must retire the connection while provider stays alive");
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let process_id = std::fs::read_to_string(&process_id_path)
                .expect("fixture wrote process id")
                .trim()
                .parse::<i32>()
                .expect("positive process id");
            let process_id =
                rustix::process::Pid::from_raw(process_id).expect("positive process id");
            if matches!(
                rustix::process::test_kill_process(process_id),
                Err(rustix::io::Errno::SRCH)
            ) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("retired provider process must be reaped");
}

#[cfg(unix)]
#[tokio::test]
async fn held_initialize_times_out_and_reaps_owned_process() {
    let fixture_root = tempfile::tempdir().expect("fixture root");
    let process_id_path = fixture_root.path().join("provider.pid");
    let fixture = format!(
        "import os,sys,time; open({:?},'w').write(str(os.getpid())); sys.stdin.readline(); time.sleep(60)",
        process_id_path.display().to_string()
    );
    let error = ExternalProviderRuntime::initialize_with_timeout(
        ExternalProviderLaunch {
            executable: PathBuf::from("/usr/bin/python3"),
            arguments: vec!["-c".to_owned(), fixture],
            environment: vec![],
        },
        Duration::from_millis(100),
    )
    .await
    .expect_err("held initialize must time out");

    assert!(matches!(
        error,
        ExternalProviderRuntimeError::InitializeTimeout
    ));
    assert_process_reaped(&process_id_path).await;
}

#[cfg(unix)]
#[tokio::test]
async fn oversized_initialize_frame_is_rejected_and_reaps_owned_process() {
    let fixture_root = tempfile::tempdir().expect("fixture root");
    let process_id_path = fixture_root.path().join("provider.pid");
    let fixture = format!(
        "import os,sys; open({:?},'w').write(str(os.getpid())); sys.stdin.readline(); print('x'*{}); sys.stdout.flush(); sys.stdin.read()",
        process_id_path.display().to_string(),
        MAX_ACP_FRAME_BYTES + 1
    );
    let error = ExternalProviderRuntime::initialize_with_timeout(
        ExternalProviderLaunch {
            executable: PathBuf::from("/usr/bin/python3"),
            arguments: vec!["-c".to_owned(), fixture],
            environment: vec![],
        },
        Duration::from_secs(2),
    )
    .await
    .expect_err("oversized frame must fail initialization");

    assert!(matches!(error, ExternalProviderRuntimeError::Initialize(_)));
    assert_process_reaped(&process_id_path).await;
}

#[cfg(unix)]
#[tokio::test]
async fn dropping_admitted_runtime_reaps_owned_process() {
    let fixture_root = tempfile::tempdir().expect("fixture root");
    let process_id_path = fixture_root.path().join("provider.pid");
    let runtime = ExternalProviderRuntime::initialize(python_fixture(1, Some(&process_id_path)))
        .await
        .expect("fixture initializes");

    drop(runtime);
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Ok(value) = std::fs::read_to_string(&process_id_path)
                && let Ok(raw) = value.trim().parse::<i32>()
            {
                let process_id = rustix::process::Pid::from_raw(raw).expect("positive process id");
                if matches!(
                    rustix::process::test_kill_process(process_id),
                    Err(rustix::io::Errno::SRCH)
                ) {
                    break;
                }
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("dropped runtime reaped provider");
}

#[cfg(unix)]
#[tokio::test]
async fn host_owned_connection_creates_and_prompts_through_official_sdk() {
    let runtime = ExternalProviderRuntime::initialize(conversation_fixture())
        .await
        .expect("fixture initializes");

    let provider_session_id = runtime
        .create_session(PathBuf::from("/tmp"))
        .await
        .expect("session created");
    assert_eq!(provider_session_id, "fixture-session");
    let output = runtime
        .prompt(provider_session_id, "hello".to_owned())
        .await
        .expect("prompt settled");
    assert!(output.output.is_empty());
    assert_eq!(output.stop_reason, ProviderPromptStopReason::EndTurn);

    runtime.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn explicit_cancel_remains_responsive_while_prompt_is_active() {
    let runtime = ExternalProviderRuntime::initialize(cancellation_fixture())
        .await
        .expect("fixture initializes");
    runtime
        .create_session(PathBuf::from("/tmp"))
        .await
        .expect("session created");

    {
        let prompt = runtime.prompt("fixture-session".to_owned(), "hold".to_owned());
        tokio::pin!(prompt);
        tokio::select! {
            biased;
            result = &mut prompt => panic!("prompt settled before cancellation: {result:?}"),
            () = tokio::time::sleep(Duration::from_millis(25)) => {}
        }
        runtime
            .cancel_active_prompt("fixture-session".to_owned())
            .await
            .expect("cancel notification accepted");
        let outcome = prompt.await.expect("cancelled prompt settles");
        assert!(outcome.output.is_empty());
        assert_eq!(outcome.stop_reason, ProviderPromptStopReason::Cancelled);
    }

    runtime.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn permission_request_is_counted_once_and_cancelled_without_payload_retention() {
    let runtime = ExternalProviderRuntime::initialize(permission_request_fixture())
        .await
        .expect("fixture initializes");
    runtime.set_endpoint_id("cursor-local".to_owned()).await;
    runtime
        .create_session(PathBuf::from("/tmp"))
        .await
        .expect("session created");

    let outcome = runtime
        .prompt(
            "fixture-session".to_owned(),
            "request permission".to_owned(),
        )
        .await
        .expect("prompt settles after permission cancellation");
    assert_eq!(outcome.stop_reason, ProviderPromptStopReason::EndTurn);
    assert_eq!(outcome.permission_refusal_reason, None);
    assert_eq!(
        runtime.approval_refusal_warnings(),
        vec![ExternalProviderApprovalRefusalWarning {
            endpoint: "cursor-local".to_owned(),
            provider_session_id: "fixture-session".to_owned(),
            method: "session/request_permission",
            reason_code: ExternalProviderApprovalRefusalReason::MissingPromptContext,
        }]
    );
    assert_eq!(
        runtime.permission_observation(),
        ExternalProviderPermissionObservation {
            method: "session/request_permission",
            request_count: 1,
            last_outcome: Some(ExternalProviderPermissionOutcome::Cancelled),
            execute_tool_call_count: 0,
        }
    );

    runtime.shutdown().await;
}

#[cfg(unix)]
async fn observe_mcp_fixture(
    raw_output: Option<serde_json::Value>,
    response_text: &str,
    split_result_before_status: bool,
) -> (Vec<ExternalProviderToolCall>, ExternalProviderPromptOutcome) {
    let runtime = ExternalProviderRuntime::initialize(named_mcp_tool_call_fixture(
        raw_output,
        response_text,
        split_result_before_status,
    ))
    .await
    .expect("fixture initializes");
    runtime
        .create_session(PathBuf::from("/tmp"))
        .await
        .expect("session created");
    let prompt_outcome = runtime
        .prompt("fixture-session".to_owned(), "invoke".to_owned())
        .await
        .expect("prompt settles");
    assert_eq!(runtime.permission_observation().execute_tool_call_count, 0);
    let tool_calls = runtime.take_test_tool_calls();
    runtime.shutdown().await;
    (tool_calls, prompt_outcome)
}

#[cfg(unix)]
#[tokio::test]
async fn decoded_mcp_success_split_before_completed_status_is_accepted() {
    let expected_service_id = "0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89";
    let expected_identity =
        collaboration_protocol::UuidIdentity::try_from(expected_service_id.to_owned())
            .expect("expected identity");
    let (tool_calls, prompt_outcome) = observe_mcp_fixture(
        Some(serde_json::json!({"success":true})),
        expected_service_id,
        true,
    )
    .await;
    assert_eq!(tool_calls.len(), 1);
    assert_eq!(
        tool_calls[0].name.as_deref(),
        Some("router-collaboration-endpoints_list")
    );
    assert_eq!(
        tool_calls[0].status,
        agent_client_protocol::schema::v1::ToolCallStatus::Completed
    );
    assert_eq!(tool_calls[0].kind, ToolKind::Other);
    assert_eq!(tool_calls[0].outcome, ExternalProviderToolOutcome::Success);
    assert!(has_completed_router_endpoints_call(&tool_calls));
    assert!(accepts_native_router_result(
        &tool_calls,
        &prompt_outcome.output,
        &expected_identity
    ));
}

#[cfg(unix)]
#[tokio::test]
async fn completed_mcp_non_success_outcomes_are_not_accepted() {
    let expected_service_id = "0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89";
    let expected_identity =
        collaboration_protocol::UuidIdentity::try_from(expected_service_id.to_owned())
            .expect("expected identity");
    for raw_output in [
        Some(serde_json::json!({"error":"failed"})),
        Some(serde_json::json!({"success":true,"error":"failed"})),
        Some(serde_json::json!({"rejected":true})),
        Some(serde_json::json!({"permissionDenied":true})),
        Some(serde_json::json!({"success":false})),
        None,
    ] {
        let (tool_calls, prompt_outcome) =
            observe_mcp_fixture(raw_output, expected_service_id, false).await;
        assert!(!accepts_native_router_result(
            &tool_calls,
            &prompt_outcome.output,
            &expected_identity
        ));
    }
}

#[cfg(unix)]
#[tokio::test]
async fn completed_mcp_success_with_wrong_returned_service_is_not_accepted() {
    let expected_identity = collaboration_protocol::UuidIdentity::try_from(
        "0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89".to_owned(),
    )
    .expect("expected identity");
    let (tool_calls, prompt_outcome) = observe_mcp_fixture(
        Some(serde_json::json!({"success":true})),
        "2ff962c5-7fa3-4c18-a5ca-1bbe8db09e81",
        false,
    )
    .await;
    assert!(!accepts_native_router_result(
        &tool_calls,
        &prompt_outcome.output,
        &expected_identity
    ));
}

#[cfg(unix)]
#[tokio::test]
async fn completed_mcp_success_accepts_one_bounded_owned_identity_in_presentation() {
    let expected_service_id = "0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89";
    let expected_identity =
        collaboration_protocol::UuidIdentity::try_from(expected_service_id.to_owned())
            .expect("expected identity");
    for response in [
        expected_service_id.to_owned(),
        format!("`{expected_service_id}`"),
        format!("The service ID is {expected_service_id}."),
    ] {
        let (tool_calls, prompt_outcome) =
            observe_mcp_fixture(Some(serde_json::json!({"success":true})), &response, false).await;
        assert!(accepts_native_router_result(
            &tool_calls,
            &prompt_outcome.output,
            &expected_identity
        ));
    }
}

#[test]
fn echoed_router_marker_without_typed_tool_event_is_not_native_execution() {
    let expected_identity = collaboration_protocol::UuidIdentity::try_from(
        "0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89".to_owned(),
    )
    .expect("expected identity");
    assert!(!accepts_native_router_result(
        &[],
        "0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89",
        &expected_identity
    ));
}

#[cfg(unix)]
#[tokio::test]
async fn overlapping_prompt_cannot_replace_context_and_dropped_waiter_cleans_it() {
    let runtime = ExternalProviderRuntime::initialize(cancellation_fixture())
        .await
        .expect("fixture initializes");
    runtime
        .create_session(PathBuf::from("/tmp"))
        .await
        .expect("session created");
    let first_operation = "019f0000-0000-7000-8000-000000002001";
    let mut first = Box::pin(runtime.prompt_with_approval_context(
        "fixture-session".to_owned(),
        "hold".to_owned(),
        approval_context(first_operation),
    ));
    tokio::select! {
        result = &mut first => panic!("first prompt settled unexpectedly: {result:?}"),
        () = tokio::time::sleep(Duration::from_millis(25)) => {}
    }
    let second = runtime
        .prompt_with_approval_context(
            "fixture-session".to_owned(),
            "overlap".to_owned(),
            approval_context("019f0000-0000-7000-8000-000000002002"),
        )
        .await
        .expect_err("overlapping prompt must not replace active authorization context");
    assert!(matches!(second, ExternalProviderRuntimeError::LocalBusy));
    assert_eq!(
        runtime
            .approval_contexts
            .lock()
            .expect("approval contexts")
            .get("fixture-session")
            .map(|context| String::from(context.operation_id.clone())),
        Some(first_operation.to_owned())
    );
    drop(first);
    assert!(
        runtime
            .approval_contexts
            .lock()
            .expect("approval contexts")
            .is_empty()
    );
    runtime.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn shutdown_joins_runtime_and_settles_held_prompt() {
    let runtime = ExternalProviderRuntime::initialize(cancellation_fixture())
        .await
        .expect("fixture initializes");
    runtime
        .create_session(PathBuf::from("/tmp"))
        .await
        .expect("session created");
    let mut prompt = Box::pin(runtime.prompt_with_approval_context(
        "fixture-session".to_owned(),
        "hold".to_owned(),
        approval_context("019f0000-0000-7000-8000-000000002099"),
    ));
    tokio::select! {
        result = &mut prompt => panic!("held prompt settled unexpectedly: {result:?}"),
        () = tokio::time::sleep(Duration::from_millis(25)) => {}
    }
    runtime.shutdown().await;
    assert!(prompt.await.is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn shutdown_records_runtime_owner_join_failure() {
    let runtime = ExternalProviderRuntime::initialize(conversation_fixture())
        .await
        .expect("fixture initializes");
    runtime
        .task
        .lock()
        .await
        .as_ref()
        .expect("runtime owner")
        .abort();
    runtime.shutdown().await;
    assert!(runtime.shutdown_failed());
}

#[cfg(unix)]
#[tokio::test]
async fn shutdown_drains_more_than_completion_channel_capacity_of_held_loads() {
    let root = tempfile::tempdir().expect("fixture root");
    let admission_log = root.path().join("load-admissions.log");
    let process_id_path = root.path().join("provider.pid");
    let runtime = Arc::new(
        ExternalProviderRuntime::initialize(saturated_load_admission_fixture(
            &admission_log,
            &process_id_path,
        ))
        .await
        .expect("fixture initializes"),
    );
    let mut loads = Vec::new();
    for index in 0..40 {
        let runtime = Arc::clone(&runtime);
        loads.push(tokio::spawn(async move {
            runtime
                .load_session(format!("held-load-{index}"), PathBuf::from("/tmp"))
                .await
        }));
    }
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let admissions = std::fs::read_to_string(&admission_log).unwrap_or_default();
            if admissions.lines().count() == 40 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("all held loads reached the provider");

    tokio::time::timeout(Duration::from_secs(2), runtime.shutdown())
        .await
        .expect("shutdown drains the saturated completion path");
    for load in loads {
        assert!(matches!(
            load.await.expect("load task"),
            Err(ExternalProviderRuntimeError::TransportFailure)
        ));
    }
    assert_process_reaped(&process_id_path).await;
}

#[cfg(unix)]
#[tokio::test]
async fn prompt_cancel_remains_responsive_during_create_admission() {
    let root = tempfile::tempdir().expect("temporary root");
    let observed = root.path().join("create-observed");
    let runtime = ExternalProviderRuntime::initialize(concurrent_admission_cancel_fixture(
        "session/new",
        &observed,
    ))
    .await
    .expect("fixture initializes");
    runtime
        .create_session(PathBuf::from("/tmp"))
        .await
        .expect("active session");
    let prompt_id: OperationId = "019f0000-0000-7000-8000-000000002101"
        .to_owned()
        .try_into()
        .expect("operation ID");
    let mut prompt = Box::pin(runtime.prompt_for_operation(
        "active-session".to_owned(),
        Some(prompt_id.clone()),
        "hold".to_owned(),
        None,
    ));
    assert!(futures_util::poll!(&mut prompt).is_pending());
    let mut create = Box::pin(runtime.create_session(PathBuf::from("/tmp")));
    assert!(futures_util::poll!(&mut create).is_pending());
    tokio::time::timeout(Duration::from_secs(2), async {
        while !observed.exists() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("create reached provider");
    runtime
        .cancel_prompt_operation("active-session".to_owned(), prompt_id)
        .await
        .expect("cancel routed");
    assert_eq!(
        prompt.await.expect("prompt settles").stop_reason,
        ProviderPromptStopReason::Cancelled
    );
    assert_eq!(create.await.expect("create settles"), "new-session");
    runtime.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn prompt_cancel_remains_responsive_during_load_admission() {
    let root = tempfile::tempdir().expect("temporary root");
    let observed = root.path().join("load-observed");
    let runtime = ExternalProviderRuntime::initialize(concurrent_admission_cancel_fixture(
        "session/load",
        &observed,
    ))
    .await
    .expect("fixture initializes");
    runtime
        .create_session(PathBuf::from("/tmp"))
        .await
        .expect("active session");
    let prompt_id: OperationId = "019f0000-0000-7000-8000-000000002102"
        .to_owned()
        .try_into()
        .expect("operation ID");
    let mut prompt = Box::pin(runtime.prompt_for_operation(
        "active-session".to_owned(),
        Some(prompt_id.clone()),
        "hold".to_owned(),
        None,
    ));
    assert!(futures_util::poll!(&mut prompt).is_pending());
    let mut load =
        Box::pin(runtime.load_session("loaded-session".to_owned(), PathBuf::from("/tmp")));
    assert!(futures_util::poll!(&mut load).is_pending());
    tokio::time::timeout(Duration::from_secs(2), async {
        while !observed.exists() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("load reached provider");
    runtime
        .cancel_prompt_operation("active-session".to_owned(), prompt_id)
        .await
        .expect("cancel routed");
    assert_eq!(
        prompt.await.expect("prompt settles").stop_reason,
        ProviderPromptStopReason::Cancelled
    );
    load.await.expect("load settles");
    runtime.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn late_cancel_for_prior_operation_cannot_cancel_successor_prompt() {
    let runtime = ExternalProviderRuntime::initialize(successor_prompt_fixture())
        .await
        .expect("fixture initializes");
    runtime
        .create_session(PathBuf::from("/tmp"))
        .await
        .expect("session created");
    let first_id = "019f0000-0000-7000-8000-000000002101";
    runtime
        .prompt_with_approval_context(
            "fixture-session".to_owned(),
            "first".to_owned(),
            approval_context(first_id),
        )
        .await
        .expect("first prompt");
    let second_id = "019f0000-0000-7000-8000-000000002102";
    let mut second = Box::pin(runtime.prompt_with_approval_context(
        "fixture-session".to_owned(),
        "second".to_owned(),
        approval_context(second_id),
    ));
    tokio::select! {
        result = &mut second => panic!("successor settled too early: {result:?}"),
        () = tokio::time::sleep(Duration::from_millis(25)) => {}
    }
    let stale_cancel = runtime
        .cancel_prompt_operation(
            "fixture-session".to_owned(),
            first_id.to_owned().try_into().expect("operation ID"),
        )
        .await
        .expect_err("stale cancel must not reach successor prompt");
    assert!(stale_cancel.to_string().contains("no longer active"));
    assert_eq!(
        second.await.expect("successor settles").stop_reason,
        ProviderPromptStopReason::EndTurn
    );
    runtime.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn aggregate_prompt_output_is_bounded_across_valid_frames() {
    let runtime = ExternalProviderRuntime::initialize(aggregate_output_fixture())
        .await
        .expect("fixture initializes");
    runtime
        .create_session(PathBuf::from("/tmp"))
        .await
        .expect("session created");

    let error = runtime
        .prompt("fixture-session".to_owned(), "overflow".to_owned())
        .await
        .expect_err("aggregate output must be bounded");
    assert!(matches!(
        error,
        ExternalProviderRuntimeError::PromptOutputLimitExceeded
    ));

    runtime.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn load_history_replay_is_not_returned_as_the_next_prompt_output() {
    let runtime = ExternalProviderRuntime::initialize(load_replay_fixture())
        .await
        .expect("fixture initializes");
    runtime
        .load_session("fixture-session".to_owned(), PathBuf::from("/tmp"))
        .await
        .expect("session loads");

    let outcome = runtime
        .prompt("fixture-session".to_owned(), "next".to_owned())
        .await
        .expect("prompt settles");
    assert_eq!(outcome.output, "NEW_OUTPUT");

    runtime.shutdown().await;
}

#[tokio::test]
#[ignore = "requires an explicitly selected authenticated external provider runtime"]
async fn live_external_provider_create_and_prompt() {
    let executable = std::env::var_os("CODEX_ROUTER_TEST_EXTERNAL_ACP_EXECUTABLE")
        .map(PathBuf::from)
        .expect("external ACP executable");
    let arguments = std::env::var("CODEX_ROUTER_TEST_EXTERNAL_ACP_ARGUMENTS")
        .ok()
        .map(|encoded| serde_json::from_str::<Vec<String>>(&encoded).expect("JSON argument array"))
        .unwrap_or_default();
    let cwd = std::env::var_os("CODEX_ROUTER_TEST_EXTERNAL_ACP_CWD")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().expect("current directory"));
    let launch = ExternalProviderLaunch {
        executable,
        arguments,
        environment: Vec::new(),
    };
    let runtime = ExternalProviderRuntime::initialize(launch.clone())
        .await
        .expect("provider initializes");
    eprintln!("provider admission: {:?}", runtime.admission());
    let provider_session_id = runtime
        .create_session(cwd.clone())
        .await
        .expect("provider conversation created");
    let outcome = runtime
        .prompt(
            provider_session_id.clone(),
            "Reply with exactly PR2_LIVE_PROVIDER_OK and no other text.".to_owned(),
        )
        .await
        .expect("provider prompt settled");
    eprintln!(
        "provider target={provider_session_id} stop={:?} output={:?}",
        outcome.stop_reason, outcome.output
    );
    assert_eq!(outcome.stop_reason, ProviderPromptStopReason::EndTurn);
    assert_eq!(outcome.output.trim(), "PR2_LIVE_PROVIDER_OK");
    runtime.shutdown().await;

    let resumed = ExternalProviderRuntime::initialize(launch)
        .await
        .expect("provider reinitializes for load");
    resumed
        .load_session(provider_session_id.clone(), cwd)
        .await
        .expect("provider conversation loads");
    let resumed_outcome = resumed
        .prompt(
            provider_session_id.clone(),
            "Reply with exactly PR2_LIVE_PROVIDER_RESUMED and no other text.".to_owned(),
        )
        .await
        .expect("loaded provider prompt settled");
    eprintln!(
        "loaded provider target={provider_session_id} stop={:?} output={:?}",
        resumed_outcome.stop_reason, resumed_outcome.output
    );
    assert_eq!(
        resumed_outcome.stop_reason,
        ProviderPromptStopReason::EndTurn
    );
    assert_eq!(resumed_outcome.output.trim(), "PR2_LIVE_PROVIDER_RESUMED");
    resumed.shutdown().await;
}

#[tokio::test]
#[ignore = "requires an explicitly selected authenticated external provider runtime"]
async fn live_external_provider_explicit_cancel() {
    let executable = std::env::var_os("CODEX_ROUTER_TEST_EXTERNAL_ACP_EXECUTABLE")
        .map(PathBuf::from)
        .expect("external ACP executable");
    let arguments = std::env::var("CODEX_ROUTER_TEST_EXTERNAL_ACP_ARGUMENTS")
        .ok()
        .map(|encoded| serde_json::from_str::<Vec<String>>(&encoded).expect("JSON argument array"))
        .unwrap_or_default();
    let cwd = std::env::var_os("CODEX_ROUTER_TEST_EXTERNAL_ACP_CWD")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().expect("current directory"));
    let runtime = ExternalProviderRuntime::initialize(ExternalProviderLaunch {
        executable,
        arguments,
        environment: Vec::new(),
    })
    .await
    .expect("provider initializes");
    let provider_session_id = runtime
        .create_session(cwd)
        .await
        .expect("provider conversation created");

    {
        let prompt = runtime.prompt(
            provider_session_id.clone(),
            "Without using tools, write the integers from 1 through 100000, one per line."
                .to_owned(),
        );
        tokio::pin!(prompt);
        tokio::select! {
            result = &mut prompt => panic!("provider prompt settled before explicit cancel: {result:?}"),
            () = tokio::time::sleep(Duration::from_millis(500)) => {}
        }
        runtime
            .cancel_active_prompt(provider_session_id.clone())
            .await
            .expect("provider accepts cancel notification");
        let outcome = tokio::time::timeout(Duration::from_secs(15), prompt)
            .await
            .expect("cancelled prompt settles")
            .expect("cancelled prompt outcome");
        eprintln!(
            "cancelled provider target={provider_session_id} stop={:?}",
            outcome.stop_reason
        );
        assert_eq!(outcome.stop_reason, ProviderPromptStopReason::Cancelled);
    }

    runtime.shutdown().await;
}

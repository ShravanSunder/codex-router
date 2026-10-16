//! Real public MCP and ACP pipes; only the external provider response is a stand-in.
use codex_router_host::{
    CollaborationRuntime, CollaborationRuntimeInputs, ExternalProviderLaunchBinding,
    ExternalProviderStartup,
};
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use serde_json::{Value, json};
use std::{
    io::Write,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

const CREATE_OPERATION: &str = "019f0000-0000-7000-8000-000000000301";
const PROMPT_OPERATION: &str = "019f0000-0000-7000-8000-000000000302";
const CANCEL_OPERATION: &str = "019f0000-0000-7000-8000-000000000303";
const FULL_INPUT: &str = "Complete the SQLite proof.\nKeep this exact second line PRIVATE_CANARY.";
const PROVIDER_SCRIPT: &str = r#"
import json,os,sys
kind,receipt,expected_native_executable=sys.argv[1:]
assert os.environ.get('CLAUDE_CODE_EXECUTABLE')==expected_native_executable
def read(): return json.loads(sys.stdin.readline())
def send(value): print(json.dumps(value),flush=True)
sys.stderr.write('PRIVATE_CANARY:'+('x'*131072));sys.stderr.flush()
request=read()
assert request['method']=='initialize'
send({'jsonrpc':'2.0','id':request['id'],'result':{'protocolVersion':1,'agentCapabilities':{},'agentInfo':{'name':'stdio-fixture','version':'1'}}})
request=read()
assert request['method']=='session/new'
send({'jsonrpc':'2.0','id':request['id'],'result':{'sessionId':'stdio-session'}})
prompt=read()
assert prompt['method']=='session/prompt'
# Independent literal oracle: never generated from the Router's renderer.
assert prompt['params']['prompt']==[{'type':'text','text':'Complete the SQLite proof.\nKeep this exact second line PRIVATE_CANARY.'}]
if kind=='approval':
    method='session/request_permission'
    params={'sessionId':'stdio-session','toolCall':{'toolCallId':'private-tool','title':'PRIVATE_CANARY','kind':'execute','rawInput':{'command':'PRIVATE_CANARY'}},'options':[{'optionId':'allow','name':'Allow once','kind':'allow_once'}]}
else:
    method='elicitation/create'
    params={'mode':'form','sessionId':'stdio-session','message':'PRIVATE_CANARY','requestedSchema':{'type':'object','properties':{'answer':{'type':'string'}}}}
send({'jsonrpc':'2.0','id':'callback','method':method,'params':params})
seen_cancel=False
seen_callback=False
while not(seen_cancel and seen_callback):
    message=read()
    if message.get('method')=='session/cancel': seen_cancel=True
    elif message.get('id')=='callback':
        assert 'result' in message,message
        expected={'outcome':{'outcome':'cancelled'}} if kind=='approval' else {'action':'cancel'}
        assert message['result']==expected,message
        seen_callback=True
    else: raise AssertionError('unexpected wire input')
with open(receipt,'w') as output: json.dump({'pid':os.getpid(),'fullInputMatched':True,'callbackCancelled':seen_callback,'cancelReceived':seen_cancel},output)
send({'jsonrpc':'2.0','id':prompt['id'],'result':{'stopReason':'cancelled'}})
sys.stdin.read()
"#;

#[derive(Clone, Default)]
struct DiagnosticCapture(Arc<Mutex<Vec<u8>>>);
impl Write for DiagnosticCapture {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .expect("diagnostic capture")
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl<'writer> tracing_subscriber::fmt::MakeWriter<'writer> for DiagnosticCapture {
    type Writer = Self;
    fn make_writer(&'writer self) -> Self::Writer {
        self.clone()
    }
}

async fn tool(client: &reqwest::Client, url: &str, name: &str, arguments: Value) -> Value {
    let response = client.post(url)
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream")
        .header("mcp-protocol-version", "2025-11-25")
        .json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":arguments}}))
        .send().await.expect("public MCP call");
    let body = response.text().await.expect("MCP body");
    let payload = body
        .lines()
        .find_map(|line| line.strip_prefix("data: "))
        .unwrap_or(&body);
    let envelope: Value = serde_json::from_str(payload).expect("MCP result");
    assert_eq!(envelope["result"]["isError"], false, "{name}: {envelope}");
    envelope["result"]["structuredContent"].clone()
}

async fn exercise_public_stdio(message_kind: &str, interaction_kind: &str) {
    use tracing_subscriber::{Layer, layer::SubscriberExt};
    let capture = DiagnosticCapture::default();
    let subscriber = tracing_subscriber::registry().with(
        tracing_subscriber::fmt::layer()
            .json()
            .with_current_span(false)
            .with_span_list(false)
            .with_writer(capture.clone())
            .with_filter(tracing_subscriber::filter::filter_fn(|metadata| {
                metadata.target() == "acp_client_runtime::provider_diagnostics"
            })),
    );
    let _diagnostic_guard = tracing::subscriber::set_default(subscriber);
    let root = tempfile::tempdir().expect("isolated runtime root");
    std::fs::create_dir_all(root.path().join("service")).expect("service directory");
    std::fs::create_dir_all(root.path().join("unused-codex-home"))
        .expect("fixture metadata directory");
    let global_directory = root.path().join("fixture-global-bin");
    std::fs::create_dir(&global_directory).expect("owned global executable directory");
    let global_executable = global_directory.join("claude");
    std::fs::write(&global_executable, "#!/bin/sh\nexit 99\n")
        .expect("explicit scripted provider's native binding");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for directory in [
            root.path().to_path_buf(),
            root.path().join("service"),
            root.path().join("unused-codex-home"),
            global_directory.clone(),
        ] {
            std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))
                .expect("private fixture directory");
        }
        std::fs::set_permissions(&global_executable, std::fs::Permissions::from_mode(0o700))
            .expect("executable fixture native binding");
    }
    let expected_native_executable = global_executable
        .canonicalize()
        .expect("canonical fixture executable");
    let receipt_path = root.path().join("wire-receipt.json");
    let mut binding = ExternalProviderLaunchBinding::claude(
        PathBuf::from("/usr/bin/python3"),
        vec![
            "-u".into(),
            "-c".into(),
            PROVIDER_SCRIPT.into(),
            interaction_kind.into(),
            receipt_path.to_string_lossy().into_owned(),
            expected_native_executable
                .to_str()
                .expect("UTF-8 fixture executable")
                .to_owned(),
        ],
    )
    .expect("fixture binding");
    binding.launch.environment = vec![
        (
            "PATH".to_owned(),
            global_directory
                .to_str()
                .expect("UTF-8 fixture launch path")
                .to_owned(),
        ),
        ("CLAUDE_CODE_EXECUTABLE".to_owned(), String::new()),
    ];
    let runtime = CollaborationRuntime::start_with_external_providers(
        CollaborationRuntimeInputs {
            directory: root.path().join("service"),
            codex_home: root.path().join("unused-codex-home"),
            backend_socket: root.path().join("unused-backend.sock"),
            mcp_bind: ([127, 0, 0, 1], 0).into(),
            native_schema: None,
            peer_registry_directory: None,
            remote_control_server_name: None,
            owner_human_id: None,
        },
        vec![ExternalProviderStartup::Launch(binding)],
    )
    .await
    .expect("owning collaboration runtime");
    let manifest: collaboration_protocol::ServiceManifest = serde_json::from_slice(
        &std::fs::read(root.path().join("service/service.json")).expect("published service"),
    )
    .expect("typed manifest");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("bounded client");
    let inventory = tool(&client, &manifest.mcp.url, "endpoints_list", json!({})).await;
    let endpoint = inventory["endpoints"]
        .as_array()
        .expect("endpoint inventory")
        .iter()
        .find(|record| record["endpoint"]["endpointId"] == "claude-local")
        .expect("provider endpoint")["endpoint"]
        .clone();
    let actor = json!({"endpoint":{"serviceId":endpoint["serviceId"],"endpointId":"codex-local"},"sessionId":"test-requester"});
    let human = json!({"kind":"human","humanId":"test-owner"});
    let created = tool(
        &client,
        &manifest.mcp.url,
        "conversation_create",
        json!({
            "operationId":CREATE_OPERATION,"endpoint":endpoint,"workingDirectory":root.path(),
            "createdBy":human,"approver":human,"access":"workspace-write","timeoutSeconds":2
        }),
    )
    .await;
    assert_eq!(created["kind"], "created", "{created}");
    let target = created["target"].clone();
    let message = match message_kind {
        "agent" => json!({"kind":"agent","sender":actor,"text":FULL_INPUT}),
        "humanUser" => json!({"kind":"humanUser","text":FULL_INPUT}),
        _ => panic!("unsupported fixture message"),
    };
    let pending = tool(
        &client,
        &manifest.mcp.url,
        "conversation_prompt",
        json!({
            "operationId":PROMPT_OPERATION,"target":target,"requestedBy":actor,"approver":human,
            "message":message,"timeoutSeconds":1
        }),
    )
    .await;
    assert_eq!(
        pending["kind"], "pending",
        "caller wait detaches: {pending}"
    );
    let (list_tool, list_field) = if interaction_kind == "approval" {
        ("approval_list", "approvals")
    } else {
        ("question_list", "questions")
    };
    let list_args = if interaction_kind == "approval" {
        json!({"pending":true,"includeOptions":true})
    } else {
        json!({"pending":true})
    };
    let pending_interactions = tool(&client, &manifest.mcp.url, list_tool, list_args).await;
    assert_eq!(
        pending_interactions[list_field]
            .as_array()
            .expect("pending interactions")
            .len(),
        1,
        "{pending_interactions}"
    );
    tool(&client,&manifest.mcp.url,"conversation_cancel",json!({
        "operationId":CANCEL_OPERATION,"targetOperationId":PROMPT_OPERATION,"target":target,"requestedBy":actor,"approver":human
    })).await;
    let settled = tool(
        &client,
        &manifest.mcp.url,
        "conversation_operation_wait",
        json!({"operationId":PROMPT_OPERATION,"timeoutSeconds":2}),
    )
    .await;
    assert_eq!(settled["output"]["kind"], "available", "{settled}");
    assert_eq!(
        settled["output"]["settlement"]["stopReason"], "cancelled",
        "{settled}"
    );
    let wire: Value =
        serde_json::from_slice(&std::fs::read(&receipt_path).expect("real provider wire receipt"))
            .expect("wire JSON");
    assert_eq!(wire["fullInputMatched"], true);
    assert_eq!(wire["callbackCancelled"], true);
    assert_eq!(wire["cancelReceived"], true);
    runtime
        .shutdown()
        .await
        .expect("joined owning runtime shutdown");
    let process_id = wire["pid"].as_u64().expect("actual provider PID");
    assert!(
        !std::process::Command::new("/bin/kill")
            .args(["-0", &process_id.to_string()])
            .status()
            .expect("provider liveness check")
            .success(),
        "provider child must be reaped"
    );
    let output =
        String::from_utf8(capture.0.lock().expect("capture").clone()).expect("diagnostic UTF8");
    for witness in [
        "promptEnqueued",
        "outgoingFlushed",
        "stderrBytes",
        "incomingFrame",
        "callbackEntered",
    ] {
        assert!(
            output.contains(witness),
            "missing fixed {witness} witness: {output}"
        );
    }
    assert!(
        !output.contains("PRIVATE_CANARY"),
        "diagnostics leaked payload: {output}"
    );
    assert!(
        !output.contains("stdio-session"),
        "diagnostics leaked session identity: {output}"
    );
}

#[tokio::test]
async fn public_agent_input_reaches_real_stdio_and_permission_cancel_is_observed() {
    exercise_public_stdio("agent", "approval").await;
}

#[tokio::test]
async fn public_human_input_reaches_real_stdio_and_question_cancel_is_observed() {
    exercise_public_stdio("humanUser", "question").await;
}

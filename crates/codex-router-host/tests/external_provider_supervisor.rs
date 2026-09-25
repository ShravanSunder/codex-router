use codex_router_host::{
    ExternalProviderBinding, ExternalProviderLaunch, ExternalProviderRuntime,
    ExternalProviderSupervisor,
};
use collaboration_protocol::{
    ApprovalDecideParams, ApprovalDecision, CodexGeneration, ConversationAdmissionState,
    ConversationCancelRequest, ConversationCreateRequest, ConversationLoadRequest,
    ConversationOperationFailureKind, ConversationOperationReconcileRequest,
    ConversationOperationSettlement, ConversationOperationShowRequest,
    ConversationOperationWaitOutput, ConversationOperationWaitRequest, ConversationPromptRequest,
    EndpointDescription, EndpointId, EndpointRef, GenerationNumber, MessageContent, MessageText,
    NonEmptyText, OperationId, PositiveSeconds, ProviderBindingId, ProviderBindingIdentity,
    ProviderCapabilities, ProviderCapability, ProviderCapabilityEvidence, ProviderCapabilityName,
    ProviderCapabilityStatus, ProviderKind, ProviderOperationEffect, ProviderOperationStage,
    ProviderPromptStopReason, ProviderReconciliationState, ProviderRequestedPolicy,
    ProviderRuntimeIdentity, ProviderTransport, ProviderWorkingDirectory, RouterAccess, SessionId,
    SessionRef, UuidIdentity,
};
use collaboration_service::{
    EndpointDirectory, NativeControlBackend, NativeGenerationGate, ProviderConversationBackend,
    ProviderOperationStore, ServiceApprovalBroker,
};
use futures_util::{SinkExt as _, StreamExt as _};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc};
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Message;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

macro_rules! ensure {
    ($condition:expr) => {
        if !$condition {
            return Err(format!("assertion failed: {}", stringify!($condition)).into());
        }
    };
}

macro_rules! ensure_eq {
    ($left:expr, $right:expr) => {{
        let left = &$left;
        let right = &$right;
        if left != right {
            return Err(format!(
                "assertion failed: {} == {}: left={left:?}, right={right:?}",
                stringify!($left),
                stringify!($right)
            )
            .into());
        }
    }};
}

fn endpoint(name: &str) -> TestResult<EndpointRef> {
    Ok(EndpointRef {
        service_id: UuidIdentity::try_from("0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89".to_owned())?,
        endpoint_id: EndpointId::try_from(name.to_owned())?,
    })
}

fn generation() -> TestResult<CodexGeneration> {
    Ok(CodexGeneration {
        service_epoch: UuidIdentity::try_from("1ff962c5-7fa3-4c18-a5ca-1bbe8db09e80".to_owned())?,
        generation: GenerationNumber::try_from(1)?,
    })
}

fn binding(endpoint: EndpointRef, name: &str) -> TestResult<ProviderBindingIdentity> {
    Ok(ProviderBindingIdentity {
        endpoint,
        binding_id: ProviderBindingId::try_from(format!("{name}-binding"))?,
        runtime: ProviderRuntimeIdentity {
            provider: ProviderKind::ClaudeCode,
            runtime_name: NonEmptyText::try_from(name.to_owned())?,
            runtime_version: Some(NonEmptyText::try_from("1".to_owned())?),
        },
        transport: ProviderTransport::StdioAcp,
        generation: generation()?,
        capabilities: ProviderCapabilities::try_from(vec![
            ProviderCapability {
                name: ProviderCapabilityName::Create,
                status: ProviderCapabilityStatus::Supported,
                evidence: ProviderCapabilityEvidence::Advertised,
            },
            ProviderCapability {
                name: ProviderCapabilityName::Prompt,
                status: ProviderCapabilityStatus::Supported,
                evidence: ProviderCapabilityEvidence::Advertised,
            },
            ProviderCapability {
                name: ProviderCapabilityName::Cancel,
                status: ProviderCapabilityStatus::Supported,
                evidence: ProviderCapabilityEvidence::Advertised,
            },
        ])?,
    })
}

fn actor(endpoint: EndpointRef, id: &str) -> TestResult<SessionRef> {
    Ok(SessionRef {
        endpoint,
        session_id: SessionId::try_from(id.to_owned())?,
    })
}

fn policy() -> ProviderRequestedPolicy {
    ProviderRequestedPolicy {
        access: RouterAccess::WriteRestricted,
    }
}

fn working_directory() -> TestResult<ProviderWorkingDirectory> {
    Ok(ProviderWorkingDirectory::try_from("/tmp".to_owned())?)
}

fn launch(script: &str) -> ExternalProviderLaunch {
    ExternalProviderLaunch {
        executable: PathBuf::from("/usr/bin/python3"),
        arguments: vec!["-c".to_owned(), script.to_owned()],
        environment: vec![],
    }
}

fn cancel_fixture(dispatch_log: &std::path::Path) -> ExternalProviderLaunch {
    let script = format!(
        r#"
import json,sys
log={:?}
request=json.loads(sys.stdin.readline())
print(json.dumps({{'jsonrpc':'2.0','id':request['id'],'result':{{'protocolVersion':1,'agentCapabilities':{{'loadSession':True}},'agentInfo':{{'name':'supervisor-fixture','version':'1'}}}}}})); sys.stdout.flush()
request=json.loads(sys.stdin.readline())
assert request['method']=='session/new'
open(log,'a').write('new\n')
print(json.dumps({{'jsonrpc':'2.0','id':request['id'],'result':{{'sessionId':'fixture-session'}}}})); sys.stdout.flush()
prompt=json.loads(sys.stdin.readline())
assert prompt['method']=='session/prompt'
open(log,'a').write('prompt\n')
cancel=json.loads(sys.stdin.readline())
assert cancel['method']=='session/cancel'
open(log,'a').write('cancel\n')
print(json.dumps({{'jsonrpc':'2.0','id':prompt['id'],'result':{{'stopReason':'cancelled'}}}})); sys.stdout.flush()
sys.stdin.read()
"#,
        dispatch_log.display().to_string()
    );
    launch(&script)
}

fn load_fixture() -> ExternalProviderLaunch {
    launch(
        r#"
import json,sys
request=json.loads(sys.stdin.readline())
print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':{'protocolVersion':1,'agentCapabilities':{'loadSession':True},'agentInfo':{'name':'load-fixture','version':'1'}}})); sys.stdout.flush()
request=json.loads(sys.stdin.readline())
assert request['method']=='session/load'
print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':{}})); sys.stdout.flush()
sys.stdin.read()
"#,
    )
}

fn permission_allow_fixture() -> ExternalProviderLaunch {
    launch(
        r#"
import json,sys
request=json.loads(sys.stdin.readline())
print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':{'protocolVersion':1,'agentCapabilities':{},'agentInfo':{'name':'permission-fixture','version':'1'}}})); sys.stdout.flush()
request=json.loads(sys.stdin.readline())
assert request['method']=='session/new'
print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':{'sessionId':'fixture-session'}})); sys.stdout.flush()
prompt=json.loads(sys.stdin.readline())
assert prompt['method']=='session/prompt'
print(json.dumps({'jsonrpc':'2.0','id':91,'method':'session/request_permission','params':{'sessionId':'fixture-session','toolCall':{'toolCallId':'permission-tool'},'options':[{'optionId':'allow-exact-once','name':'Allow once','kind':'allow_once'},{'optionId':'deny-exact-once','name':'Deny once','kind':'reject_once'}]}})); sys.stdout.flush()
permission_response=json.loads(sys.stdin.readline())
assert permission_response['id']==91
assert permission_response['result']['outcome']['outcome']=='selected'
assert permission_response['result']['outcome']['optionId']=='allow-exact-once'
print(json.dumps({'jsonrpc':'2.0','id':prompt['id'],'result':{'stopReason':'end_turn'}})); sys.stdout.flush()
sys.stdin.read()
"#,
    )
}

fn permission_cancel_fixture() -> ExternalProviderLaunch {
    launch(
        r#"
import json,sys
request=json.loads(sys.stdin.readline())
print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':{'protocolVersion':1,'agentCapabilities':{},'agentInfo':{'name':'permission-retirement-fixture','version':'1'}}})); sys.stdout.flush()
request=json.loads(sys.stdin.readline())
print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':{'sessionId':'fixture-session'}})); sys.stdout.flush()
prompt=json.loads(sys.stdin.readline())
print(json.dumps({'jsonrpc':'2.0','id':92,'method':'session/request_permission','params':{'sessionId':'fixture-session','toolCall':{'toolCallId':'permission-tool'},'options':[{'optionId':'allow-exact-once','name':'Allow once','kind':'allow_once'}]}})); sys.stdout.flush()
permission_response=json.loads(sys.stdin.readline())
assert permission_response['id']==92
assert permission_response['result']['outcome']['outcome']=='cancelled'
print(json.dumps({'jsonrpc':'2.0','id':prompt['id'],'result':{'stopReason':'end_turn'}})); sys.stdout.flush()
sys.stdin.read()
"#,
    )
}

fn authentication_then_create_fixture() -> ExternalProviderLaunch {
    launch(
        r#"
import json,sys
request=json.loads(sys.stdin.readline())
print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':{'protocolVersion':1,'agentCapabilities':{},'agentInfo':{'name':'auth-fixture','version':'1'}}})); sys.stdout.flush()
first=json.loads(sys.stdin.readline())
print(json.dumps({'jsonrpc':'2.0','id':first['id'],'error':{'code':-32000,'message':'authentication required'}})); sys.stdout.flush()
second=json.loads(sys.stdin.readline())
print(json.dumps({'jsonrpc':'2.0','id':second['id'],'result':{'sessionId':'authenticated-session'}})); sys.stdout.flush()
sys.stdin.read()
"#,
    )
}

fn authentication_then_prompt_fixture() -> ExternalProviderLaunch {
    launch(
        r#"
import json,sys
request=json.loads(sys.stdin.readline())
print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':{'protocolVersion':1,'agentCapabilities':{},'agentInfo':{'name':'prompt-auth-fixture','version':'1'}}})); sys.stdout.flush()
create=json.loads(sys.stdin.readline())
print(json.dumps({'jsonrpc':'2.0','id':create['id'],'result':{'sessionId':'authenticated-session'}})); sys.stdout.flush()
first=json.loads(sys.stdin.readline())
assert first['method']=='session/prompt'
print(json.dumps({'jsonrpc':'2.0','id':first['id'],'error':{'code':-32000,'message':'authentication required'}})); sys.stdout.flush()
second=json.loads(sys.stdin.readline())
assert second['method']=='session/prompt'
print(json.dumps({'jsonrpc':'2.0','method':'session/update','params':{'sessionId':'authenticated-session','update':{'sessionUpdate':'agent_message_chunk','content':{'type':'text','text':'final text retained'}}}})); sys.stdout.flush()
print(json.dumps({'jsonrpc':'2.0','id':second['id'],'result':{'stopReason':'end_turn'}})); sys.stdout.flush()
sys.stdin.read()
"#,
    )
}

fn provider_prompt_rejection_fixture() -> ExternalProviderLaunch {
    launch(
        r#"
import json,sys
request=json.loads(sys.stdin.readline())
print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':{'protocolVersion':1,'agentCapabilities':{},'agentInfo':{'name':'prompt-rejection-fixture','version':'1'}}})); sys.stdout.flush()
create=json.loads(sys.stdin.readline())
print(json.dumps({'jsonrpc':'2.0','id':create['id'],'result':{'sessionId':'rejection-session'}})); sys.stdout.flush()
prompt=json.loads(sys.stdin.readline())
print(json.dumps({'jsonrpc':'2.0','id':prompt['id'],'error':{'code':-32603,'message':'provider conversation is busy'}})); sys.stdout.flush()
sys.stdin.read()
"#,
    )
}

fn saturated_load_fixture(
    admission_log: &std::path::Path,
    process_id_path: &std::path::Path,
) -> ExternalProviderLaunch {
    launch(&format!(
        r#"
import json,os,sys
open({process_id:?},'w').write(str(os.getpid()))
request=json.loads(sys.stdin.readline())
print(json.dumps({{'jsonrpc':'2.0','id':request['id'],'result':{{'protocolVersion':1,'agentCapabilities':{{'loadSession':True}},'agentInfo':{{'name':'saturated-supervisor-fixture','version':'1'}}}}}})); sys.stdout.flush()
for line in sys.stdin:
    request=json.loads(line)
    assert request['method']=='session/load'
    with open({admission_log:?},'a') as log:
        log.write(request['params']['sessionId']+'\n')
"#,
        process_id = process_id_path.display().to_string(),
        admission_log = admission_log.display().to_string(),
    ))
}

async fn assert_process_reaped(process_id_path: &std::path::Path) -> TestResult {
    let process_id = std::fs::read_to_string(process_id_path)?.parse::<i32>()?;
    let process_id = rustix::process::Pid::from_raw(process_id).ok_or("invalid provider PID")?;
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if matches!(
                rustix::process::test_kill_process(process_id),
                Err(rustix::io::Errno::SRCH)
            ) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await?;
    Ok(())
}

async fn approval_broker_fixture(
    root: &tempfile::TempDir,
    service_id: &UuidIdentity,
    approver: &SessionRef,
) -> TestResult<(
    Arc<ServiceApprovalBroker>,
    tokio::task::JoinHandle<Result<Vec<Value>, Box<dyn std::error::Error + Send + Sync>>>,
)> {
    let socket_path = root.path().join("native-approver.sock");
    let listener = tokio::net::UnixListener::bind(&socket_path)?;
    let native_generation = generation()?;
    let mut definitions = serde_json::Map::new();
    for operation in [
        "ThreadRead",
        "ThreadResume",
        "ThreadStart",
        "ThreadLoadedList",
        "TurnStart",
        "TurnSteer",
        "TurnInterrupt",
        "ThreadQueueAdd",
    ] {
        definitions.insert(format!("{operation}Params"), json!({"type":"object"}));
        definitions.insert(format!("{operation}Response"), json!({"type":"object"}));
    }
    let bundle = codex_native_integration::NativeSchemaBundle::from_documents(BTreeMap::from([(
        "codex_app_server_protocol.schemas.json".to_owned(),
        serde_json::to_vec(&json!({"definitions":{"v2":definitions}}))?,
    )]))?;
    let schemas = Arc::new(codex_native_integration::NativePayloadSchemas::from_bundle(
        &bundle,
    )?);
    let gate = NativeGenerationGate::default();
    gate.activate(
        native_generation.clone(),
        socket_path,
        Some(schemas.clone()),
    )?;
    let endpoint_description: EndpointDescription = serde_json::from_value(json!({
        "endpoint": approver.endpoint,
        "label": "Fixture approver",
        "availability": {"state":"available","observedAt":"2026-09-20T00:00:00Z"},
        "channels": [{
            "kind":"nativeCodex",
            "transport":"unixWebSocket",
            "path":"native-approver.sock",
            "schemaDigest":schemas.schema_digest(),
            "generation":native_generation
        }]
    }))?;
    let endpoints = EndpointDirectory::new(service_id.clone());
    endpoints.publish(endpoint_description)?;
    let native_backend = NativeControlBackend {
        codex_home: root.path().to_path_buf(),
        endpoint: approver.endpoint.clone(),
        gate,
    };
    let broker = ServiceApprovalBroker::load(
        service_id.clone(),
        native_backend.clone(),
        root.path().join("approval-routes.json"),
    )
    .await?;
    let route: Arc<dyn collaboration_service::SessionDeliveryRoute> =
        Arc::new(collaboration_service::CodexAppServerDeliveryRoute::new(
            service_id.clone(),
            endpoints,
            native_backend,
            Arc::new(collaboration_service::UnmaterializedThreadHolder::new()),
        ));
    broker.install_session_delivery(Arc::new(
        collaboration_service::SessionDeliveryRouter::new(vec![route]),
    ))?;
    let backend = tokio::spawn(async move {
        let (stream, _) = listener.accept().await?;
        let mut socket = tokio_tungstenite::accept_async(stream).await?;
        let mut requests = Vec::new();
        for expected_method in ["initialize", "initialized", "thread/read", "turn/start"] {
            let frame = tokio::time::timeout(std::time::Duration::from_secs(2), socket.next())
                .await
                .map_err(|error| {
                    std::io::Error::other(format!(
                        "native approver timed out waiting for {expected_method}: {error}"
                    ))
                })?
                .ok_or_else(|| {
                    std::io::Error::other(format!(
                        "native approver connection closed while waiting for {expected_method}"
                    ))
                })?
                .map_err(|error| {
                    std::io::Error::other(format!(
                        "native approver websocket failed while waiting for {expected_method}: {error}"
                    ))
                })?;
            let request: Value = serde_json::from_str(frame.to_text()?)?;
            ensure_eq!(
                request.get("method").and_then(Value::as_str),
                Some(expected_method)
            );
            requests.push(request.clone());
            if expected_method == "initialized" {
                continue;
            }
            let result = match expected_method {
                "thread/read" => json!({"thread":{"id":"approver","status":{"type":"idle"}}}),
                "turn/start" => json!({"turn":{"id":"approval-turn"}}),
                _ => json!({}),
            };
            socket
                .send(Message::Text(
                    json!({"id":request.get("id").ok_or("native request ID missing")?,"result":result})
                        .to_string()
                        .into(),
                ))
                .await?;
        }
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(requests)
    });
    Ok((broker, backend))
}

async fn supervisor(
    root: &tempfile::TempDir,
    bindings: Vec<(ProviderBindingIdentity, ExternalProviderRuntime)>,
) -> TestResult<ExternalProviderSupervisor> {
    let store =
        ProviderOperationStore::open(&root.path().join("provider-operations.sqlite")).await?;
    Ok(ExternalProviderSupervisor::new(
        bindings
            .into_iter()
            .map(|(identity, runtime)| ExternalProviderBinding { identity, runtime })
            .collect(),
        Arc::new(Mutex::new(store)),
    )?)
}

#[allow(clippy::expect_used, clippy::result_large_err)]
async fn wait(
    backend: &dyn ProviderConversationBackend,
    operation_id: OperationId,
) -> Result<
    collaboration_protocol::ConversationOperationWaitResult,
    collaboration_protocol::ConversationOperationFailure,
> {
    backend
        .wait(ConversationOperationWaitRequest {
            operation_id,
            timeout_seconds: PositiveSeconds::try_from(2).expect("positive timeout"),
        })
        .await
}

fn operation<T>(
    result: Result<T, collaboration_protocol::ConversationOperationFailure>,
) -> TestResult<T> {
    result.map_err(|error| format!("provider operation failed: {error:?}").into())
}

async fn await_admission(
    backend: &dyn ProviderConversationBackend,
    operation_id: &OperationId,
) -> TestResult {
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            match backend
                .show(ConversationOperationShowRequest {
                    operation_id: operation_id.clone(),
                })
                .await
            {
                Ok(_) => return Ok(()),
                Err(error) if error.kind == ConversationOperationFailureKind::NotFound => {
                    tokio::task::yield_now().await;
                }
                Err(error) => {
                    return Err(format!("provider admission failed: {error:?}").into());
                }
            }
        }
    })
    .await
    .map_err(|_| "provider admission timed out")?
}

#[cfg(unix)]
#[tokio::test]
async fn supplied_id_is_admitted_once_and_cancelled_prompt_settles_after_detach() -> TestResult {
    let root = tempfile::tempdir()?;
    let dispatch_log = root.path().join("dispatch.log");
    let endpoint = endpoint("claude-local")?;
    let runtime = ExternalProviderRuntime::initialize(cancel_fixture(&dispatch_log)).await?;
    let backend = supervisor(&root, vec![(binding(endpoint.clone(), "claude")?, runtime)]).await?;
    let requester = actor(endpoint.clone(), "requester")?;
    let operation_id = OperationId::generate();
    let create_request = ConversationCreateRequest {
        operation_id: operation_id.clone(),
        endpoint: endpoint.clone(),
        generation: Some(generation()?),
        working_directory: working_directory()?,
        created_by: requester.clone(),
        approver: requester.clone(),
        requested_policy: policy(),
    };
    let detached_create = backend.create(create_request.clone());
    drop(detached_create);
    await_admission(&backend, &operation_id).await?;
    let duplicate = operation(backend.create(create_request).await)?;
    ensure_eq!(duplicate.admission, ConversationAdmissionState::Existing);
    let created = operation(wait(&backend, operation_id).await)?;
    let target = match created.output {
        ConversationOperationWaitOutput::Available {
            settlement: ConversationOperationSettlement::Created { target, .. },
        } => target,
        output => return Err(format!("unexpected create output: {output:?}").into()),
    };
    let mut stored =
        ProviderOperationStore::open(&root.path().join("provider-operations.sqlite")).await?;
    let session_record = stored
        .session_record(&target)
        .await?
        .ok_or("created session record missing")?;
    ensure_eq!(session_record.created_by, requester);
    ensure_eq!(session_record.approver, requester);
    stored.close().await?;

    let prompt_operation_id = OperationId::generate();
    let prompt = operation(
        backend
            .prompt(ConversationPromptRequest {
                operation_id: prompt_operation_id.clone(),
                target: target.clone(),
                generation: Some(generation()?),
                requested_by: requester.clone(),
                approver: requester.clone(),
                prompt: MessageContent::HumanUser {
                    text: MessageText::try_from("hold".to_owned())?,
                },
            })
            .await,
    )?;
    ensure_eq!(prompt.operation.effect, ProviderOperationEffect::Unknown);
    drop(prompt);

    let cancel_operation_id = OperationId::generate();
    operation(
        backend
            .cancel(ConversationCancelRequest {
                operation_id: cancel_operation_id.clone(),
                target_operation_id: prompt_operation_id.clone(),
                target: target.clone(),
                generation: Some(generation()?),
                requested_by: requester.clone(),
                approver: requester.clone(),
            })
            .await,
    )?;
    let cancel = operation(wait(&backend, cancel_operation_id.clone()).await)?;
    ensure!(matches!(
        cancel.output,
        ConversationOperationWaitOutput::Available {
            settlement: ConversationOperationSettlement::CancelRequested { .. }
        }
    ));
    let prompt = operation(wait(&backend, prompt_operation_id.clone()).await)?;
    ensure!(matches!(
        prompt.output,
        ConversationOperationWaitOutput::Available {
            settlement: ConversationOperationSettlement::PromptCompleted {
                stop_reason: ProviderPromptStopReason::Cancelled,
                ..
            }
        }
    ));
    ensure_eq!(
        std::fs::read_to_string(dispatch_log)?,
        "new\nprompt\ncancel\n"
    );
    let duplicate_cancel = operation(
        backend
            .cancel(ConversationCancelRequest {
                operation_id: cancel_operation_id,
                target_operation_id: prompt_operation_id,
                target,
                generation: Some(generation()?),
                requested_by: requester.clone(),
                approver: requester.clone(),
            })
            .await,
    )?;
    ensure_eq!(
        duplicate_cancel.admission,
        ConversationAdmissionState::Existing
    );
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn supervisor_shutdown_joins_runtime_and_settles_held_work() -> TestResult {
    let root = tempfile::tempdir()?;
    let provider_endpoint = endpoint("claude-local")?;
    let runtime =
        ExternalProviderRuntime::initialize(cancel_fixture(&root.path().join("shutdown.log")))
            .await?;
    let backend = supervisor(
        &root,
        vec![(binding(provider_endpoint.clone(), "claude")?, runtime)],
    )
    .await?;
    let requester = actor(provider_endpoint.clone(), "requester")?;
    let create_id = OperationId::generate();
    operation(
        backend
            .create(ConversationCreateRequest {
                operation_id: create_id.clone(),
                endpoint: provider_endpoint,
                generation: Some(generation()?),
                working_directory: working_directory()?,
                created_by: requester.clone(),
                approver: requester.clone(),
                requested_policy: policy(),
            })
            .await,
    )?;
    let created = operation(wait(&backend, create_id).await)?;
    let target = match created.output {
        ConversationOperationWaitOutput::Available {
            settlement: ConversationOperationSettlement::Created { target, .. },
        } => target,
        other => return Err(format!("unexpected create output: {other:?}").into()),
    };
    let prompt_id = OperationId::generate();
    operation(
        backend
            .prompt(ConversationPromptRequest {
                operation_id: prompt_id.clone(),
                target,
                generation: Some(generation()?),
                requested_by: requester.clone(),
                approver: requester,
                prompt: MessageContent::HumanUser {
                    text: MessageText::try_from("hold".to_owned())?,
                },
            })
            .await,
    )?;
    await_admission(&backend, &prompt_id).await?;
    backend
        .shutdown()
        .await
        .map_err(|message| message.to_owned())?;
    let failure = wait(&backend, prompt_id)
        .await
        .expect_err("shutdown-held prompt cannot succeed");
    ensure_eq!(failure.effect, ProviderOperationEffect::Unknown);
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn supervisor_shutdown_drains_saturated_load_completions_and_preserves_unknown_effect()
-> TestResult {
    let root = tempfile::tempdir()?;
    let admission_log = root.path().join("load-admissions.log");
    let process_id_path = root.path().join("provider.pid");
    let provider_endpoint = endpoint("claude-local")?;
    let runtime = ExternalProviderRuntime::initialize(saturated_load_fixture(
        &admission_log,
        &process_id_path,
    ))
    .await?;
    let backend = supervisor(
        &root,
        vec![(binding(provider_endpoint.clone(), "claude")?, runtime)],
    )
    .await?;
    let requester = actor(provider_endpoint.clone(), "requester")?;
    let mut operation_ids = Vec::new();
    for index in 0..40 {
        let operation_id = OperationId::generate();
        operation(
            backend
                .load(ConversationLoadRequest {
                    operation_id: operation_id.clone(),
                    target: actor(provider_endpoint.clone(), &format!("held-load-{index}"))?,
                    generation: Some(generation()?),
                    working_directory: working_directory()?,
                    requested_by: requester.clone(),
                    approver: requester.clone(),
                    requested_policy: policy(),
                })
                .await,
        )?;
        operation_ids.push(operation_id);
    }
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if std::fs::read_to_string(&admission_log)
                .unwrap_or_default()
                .lines()
                .count()
                == 40
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await?;
    backend
        .shutdown()
        .await
        .map_err(|message| message.to_owned())?;
    for operation_id in operation_ids {
        let failure = wait(&backend, operation_id)
            .await
            .expect_err("shutdown-held load cannot succeed");
        ensure_eq!(failure.effect, ProviderOperationEffect::Unknown);
    }
    assert_process_reaped(&process_id_path).await?;
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn supervisor_permission_callback_uses_installed_broker_and_exact_selected_option()
-> TestResult {
    let root = tempfile::tempdir()?;
    let provider_endpoint = endpoint("cursor-local")?;
    let requester = actor(provider_endpoint.clone(), "requester")?;
    let approver = actor(endpoint("codex-local")?, "approver")?;
    let runtime = ExternalProviderRuntime::initialize(permission_allow_fixture()).await?;
    let backend = supervisor(
        &root,
        vec![(binding(provider_endpoint.clone(), "cursor")?, runtime)],
    )
    .await?;
    let (broker, native_backend) =
        approval_broker_fixture(&root, &provider_endpoint.service_id, &approver).await?;
    backend.install_approval_broker(Arc::clone(&broker)).await;

    let create_operation_id = OperationId::generate();
    operation(
        backend
            .create(ConversationCreateRequest {
                operation_id: create_operation_id.clone(),
                endpoint: provider_endpoint,
                generation: Some(generation()?),
                working_directory: working_directory()?,
                created_by: requester.clone(),
                approver: approver.clone(),
                requested_policy: policy(),
            })
            .await,
    )?;
    let created = operation(wait(&backend, create_operation_id).await)?;
    let target = match created.output {
        ConversationOperationWaitOutput::Available {
            settlement: ConversationOperationSettlement::Created { target, .. },
        } => target,
        output => return Err(format!("unexpected create output: {output:?}").into()),
    };

    let prompt_operation_id = OperationId::generate();
    operation(
        backend
            .prompt(ConversationPromptRequest {
                operation_id: prompt_operation_id.clone(),
                target,
                generation: Some(generation()?),
                requested_by: requester,
                approver: approver.clone(),
                prompt: MessageContent::HumanUser {
                    text: MessageText::try_from("request permission".to_owned())?,
                },
            })
            .await,
    )?;
    let pending = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if let Some(record) = broker.list(true).await.approvals.into_iter().next() {
                break record;
            }
            tokio::task::yield_now().await;
        }
    })
    .await?;
    let while_live = operation(
        backend
            .reconcile(ConversationOperationReconcileRequest {
                operation_id: prompt_operation_id.clone(),
            })
            .await,
    )?;
    ensure_eq!(while_live.stage, ProviderOperationStage::MayHaveDispatched);
    ensure_eq!(
        while_live.reconciliation,
        ProviderReconciliationState::Unresolved
    );
    ensure_eq!(
        pending.requester,
        actor(endpoint("cursor-local")?, "requester")?
    );
    ensure_eq!(pending.approver, approver.clone());
    ensure_eq!(
        pending.operation["operationId"],
        String::from(prompt_operation_id.clone())
    );
    ensure_eq!(pending.operation["method"], "session/request_permission");
    let native_requests = native_backend
        .await?
        .map_err(|error| format!("native approver fixture failed: {error}"))?;
    ensure_eq!(native_requests.len(), 4);
    let decision = broker
        .decide(ApprovalDecideParams {
            request_id: pending.request_id,
            decision: ApprovalDecision::Allow,
            actor: approver,
        })
        .await
        .map_err(|error| format!("approval decision failed: {error}"))?;
    ensure_eq!(decision.scope, None);

    let settled = operation(wait(&backend, prompt_operation_id).await)?;
    ensure!(matches!(
        settled.output,
        ConversationOperationWaitOutput::Available {
            settlement: ConversationOperationSettlement::PromptCompleted {
                stop_reason: ProviderPromptStopReason::EndTurn,
                ..
            }
        }
    ));
    ensure_eq!(broker.list(false).await.approvals.len(), 1);
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn retired_provider_binding_cancels_pending_approval_before_selection() -> TestResult {
    let root = tempfile::tempdir()?;
    let provider_endpoint = endpoint("cursor-local")?;
    let requester = actor(provider_endpoint.clone(), "requester")?;
    let approver = actor(endpoint("codex-local")?, "approver")?;
    let runtime = ExternalProviderRuntime::initialize(permission_cancel_fixture()).await?;
    let binding_retirement = runtime.retirement();
    let backend = supervisor(
        &root,
        vec![(binding(provider_endpoint.clone(), "cursor")?, runtime)],
    )
    .await?;
    let (broker, native_backend) =
        approval_broker_fixture(&root, &provider_endpoint.service_id, &approver).await?;
    backend.install_approval_broker(Arc::clone(&broker)).await;
    let create_operation_id = OperationId::generate();
    operation(
        backend
            .create(ConversationCreateRequest {
                operation_id: create_operation_id.clone(),
                endpoint: provider_endpoint,
                generation: Some(generation()?),
                working_directory: working_directory()?,
                created_by: requester.clone(),
                approver: approver.clone(),
                requested_policy: policy(),
            })
            .await,
    )?;
    let created = operation(wait(&backend, create_operation_id).await)?;
    let target = match created.output {
        ConversationOperationWaitOutput::Available {
            settlement: ConversationOperationSettlement::Created { target, .. },
        } => target,
        output => return Err(format!("unexpected create output: {output:?}").into()),
    };
    let prompt_operation_id = OperationId::generate();
    operation(
        backend
            .prompt(ConversationPromptRequest {
                operation_id: prompt_operation_id.clone(),
                target,
                generation: Some(generation()?),
                requested_by: requester,
                approver: approver.clone(),
                prompt: MessageContent::HumanUser {
                    text: MessageText::try_from("request permission".to_owned())?,
                },
            })
            .await,
    )?;
    let pending = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if let Some(record) = broker.list(true).await.approvals.into_iter().next() {
                break record;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .map_err(|_| "approval did not enter pending state")?;
    let native_requests = native_backend
        .await?
        .map_err(|error| format!("native approver fixture failed: {error}"))?;
    ensure_eq!(native_requests.len(), 4);
    binding_retirement.cancel();
    let retirement_settled = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if broker.list(true).await.approvals.is_empty() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    if retirement_settled.is_err() {
        return Err(format!(
            "retired approval remained pending: {:?}",
            broker.list(false).await.approvals
        )
        .into());
    }
    ensure!(matches!(
        broker
            .decide(ApprovalDecideParams {
                request_id: pending.request_id,
                decision: ApprovalDecision::Allow,
                actor: approver,
            })
            .await,
        Err("approvalNotPending")
    ));
    let history = broker.list(false).await.approvals;
    ensure_eq!(
        history.first().map(|record| record.state),
        Some(collaboration_protocol::ApprovalState::Cancelled)
    );
    let failure = wait(&backend, prompt_operation_id)
        .await
        .expect_err("retired runtime cannot claim a successful prompt settlement");
    ensure_eq!(
        failure.kind,
        ConversationOperationFailureKind::OutcomeUnknown
    );
    backend.shutdown().await?;
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn load_and_multiple_endpoint_bindings_are_supported() -> TestResult {
    let root = tempfile::tempdir()?;
    let claude_endpoint = endpoint("claude-local")?;
    let cursor_endpoint = endpoint("cursor-local")?;
    let claude_runtime = ExternalProviderRuntime::initialize(load_fixture()).await?;
    let cursor_runtime = ExternalProviderRuntime::initialize(load_fixture()).await?;
    let backend = supervisor(
        &root,
        vec![
            (binding(claude_endpoint.clone(), "claude")?, claude_runtime),
            (binding(cursor_endpoint.clone(), "cursor")?, cursor_runtime),
        ],
    )
    .await?;
    ensure!(backend.binding(&claude_endpoint).is_some());
    ensure!(backend.binding(&cursor_endpoint).is_some());

    let target = actor(cursor_endpoint.clone(), "restored-session")?;
    let requester = actor(cursor_endpoint, "requester")?;
    let operation_id = OperationId::generate();
    operation(
        backend
            .load(ConversationLoadRequest {
                operation_id: operation_id.clone(),
                target: target.clone(),
                generation: Some(generation()?),
                working_directory: working_directory()?,
                requested_by: requester.clone(),
                approver: requester.clone(),
                requested_policy: policy(),
            })
            .await,
    )?;
    let loaded = operation(wait(&backend, operation_id).await)?;
    ensure!(matches!(
        loaded.output,
        ConversationOperationWaitOutput::Available {
            settlement: ConversationOperationSettlement::Loaded { .. }
        }
    ));
    let mut stored =
        ProviderOperationStore::open(&root.path().join("provider-operations.sqlite")).await?;
    let session_record = stored
        .session_record(&target)
        .await?
        .ok_or("loaded session record missing")?;
    ensure_eq!(session_record.created_by, requester);
    ensure_eq!(session_record.approver, requester);
    stored.close().await?;
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn response_loss_is_unknown_and_reconciliation_never_replays() -> TestResult {
    let root = tempfile::tempdir()?;
    let endpoint = endpoint("claude-local")?;
    let runtime = ExternalProviderRuntime::initialize(launch(
        r#"
import json,sys
request=json.loads(sys.stdin.readline())
print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':{'protocolVersion':1,'agentCapabilities':{},'agentInfo':{'name':'loss-fixture','version':'1'}}})); sys.stdout.flush()
sys.stdin.readline()
"#,
    ))
    .await?;
    let backend = supervisor(&root, vec![(binding(endpoint.clone(), "claude")?, runtime)]).await?;
    let requester = actor(endpoint.clone(), "requester")?;
    let operation_id = OperationId::generate();
    operation(
        backend
            .create(ConversationCreateRequest {
                operation_id: operation_id.clone(),
                endpoint,
                generation: Some(generation()?),
                working_directory: working_directory()?,
                created_by: requester.clone(),
                approver: requester,
                requested_policy: policy(),
            })
            .await,
    )?;
    let failure = match wait(&backend, operation_id.clone()).await {
        Ok(result) => return Err(format!("lost response unexpectedly settled: {result:?}").into()),
        Err(failure) => failure,
    };
    ensure_eq!(failure.effect, ProviderOperationEffect::Unknown);
    let reconciled = operation(
        backend
            .reconcile(ConversationOperationReconcileRequest { operation_id })
            .await,
    )?;
    ensure_eq!(
        reconciled.reconciliation,
        ProviderReconciliationState::NotReconcilable
    );
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn authentication_required_is_no_effect_and_does_not_poison_fresh_create() -> TestResult {
    let root = tempfile::tempdir()?;
    let provider_endpoint = endpoint("cursor-local")?;
    let requester = actor(provider_endpoint.clone(), "requester")?;
    let classification_runtime =
        ExternalProviderRuntime::initialize(authentication_then_create_fixture()).await?;
    let classification = classification_runtime
        .create_session(PathBuf::from("/tmp"))
        .await;
    if !matches!(
        classification,
        Err(
            codex_router_host::ExternalProviderRuntimeError::AuthenticationRequired {
                code: -32000
            }
        )
    ) {
        return Err(format!("unexpected authentication classification: {classification:?}").into());
    }
    classification_runtime.shutdown().await;
    let runtime = ExternalProviderRuntime::initialize(authentication_then_create_fixture()).await?;
    let backend = supervisor(
        &root,
        vec![(binding(provider_endpoint.clone(), "cursor")?, runtime)],
    )
    .await?;
    let first_id = OperationId::generate();
    operation(
        backend
            .create(ConversationCreateRequest {
                operation_id: first_id.clone(),
                endpoint: provider_endpoint.clone(),
                generation: Some(generation()?),
                working_directory: working_directory()?,
                created_by: requester.clone(),
                approver: requester.clone(),
                requested_policy: policy(),
            })
            .await,
    )?;
    let failure = wait(&backend, first_id.clone())
        .await
        .expect_err("authentication required");
    ensure_eq!(
        failure.kind,
        ConversationOperationFailureKind::AuthenticationRequired
    );
    ensure_eq!(failure.effect, ProviderOperationEffect::None);
    let duplicate = operation(
        backend
            .create(ConversationCreateRequest {
                operation_id: first_id,
                endpoint: provider_endpoint.clone(),
                generation: Some(generation()?),
                working_directory: working_directory()?,
                created_by: requester.clone(),
                approver: requester.clone(),
                requested_policy: policy(),
            })
            .await,
    )?;
    ensure_eq!(duplicate.admission, ConversationAdmissionState::Existing);
    let second_id = OperationId::generate();
    operation(
        backend
            .create(ConversationCreateRequest {
                operation_id: second_id.clone(),
                endpoint: provider_endpoint,
                generation: Some(generation()?),
                working_directory: working_directory()?,
                created_by: requester.clone(),
                approver: requester,
                requested_policy: policy(),
            })
            .await,
    )?;
    let created = operation(wait(&backend, second_id).await)?;
    ensure!(matches!(
        created.output,
        ConversationOperationWaitOutput::Available {
            settlement: ConversationOperationSettlement::Created { .. }
        }
    ));
    backend
        .shutdown()
        .await
        .map_err(|message| message.to_owned())?;
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn prompt_authentication_required_is_no_effect_and_fresh_prompt_retains_final_text()
-> TestResult {
    let root = tempfile::tempdir()?;
    let provider_endpoint = endpoint("cursor-local")?;
    let requester = actor(provider_endpoint.clone(), "requester")?;
    let runtime = ExternalProviderRuntime::initialize(authentication_then_prompt_fixture()).await?;
    let backend = supervisor(
        &root,
        vec![(binding(provider_endpoint.clone(), "cursor")?, runtime)],
    )
    .await?;
    let create_id = OperationId::generate();
    operation(
        backend
            .create(ConversationCreateRequest {
                operation_id: create_id.clone(),
                endpoint: provider_endpoint,
                generation: Some(generation()?),
                working_directory: working_directory()?,
                created_by: requester.clone(),
                approver: requester.clone(),
                requested_policy: policy(),
            })
            .await,
    )?;
    let created = operation(wait(&backend, create_id).await)?;
    let ConversationOperationWaitOutput::Available {
        settlement: ConversationOperationSettlement::Created { target, .. },
    } = created.output
    else {
        return Err("provider session was not created".into());
    };

    let first_id = OperationId::generate();
    let prompt_request = |operation_id: OperationId| ConversationPromptRequest {
        operation_id,
        target: target.clone(),
        generation: Some(generation().expect("generation")),
        requested_by: requester.clone(),
        approver: requester.clone(),
        prompt: MessageContent::HumanUser {
            text: MessageText::try_from("continue".to_owned()).expect("prompt"),
        },
    };
    operation(backend.prompt(prompt_request(first_id.clone())).await)?;
    let failure = wait(&backend, first_id.clone())
        .await
        .expect_err("prompt authentication required");
    ensure_eq!(
        failure.kind,
        ConversationOperationFailureKind::AuthenticationRequired
    );
    ensure_eq!(failure.effect, ProviderOperationEffect::None);
    let duplicate = operation(backend.prompt(prompt_request(first_id)).await)?;
    ensure_eq!(duplicate.admission, ConversationAdmissionState::Existing);

    let second_id = OperationId::generate();
    operation(backend.prompt(prompt_request(second_id.clone())).await)?;
    let completed = operation(wait(&backend, second_id).await)?;
    ensure!(matches!(
        completed.output,
        ConversationOperationWaitOutput::Available {
            settlement: ConversationOperationSettlement::PromptCompleted {
                response: Some(response),
                stop_reason: ProviderPromptStopReason::EndTurn,
                ..
            }
        } if String::from(response.clone()) == "final text retained"
    ));
    backend
        .shutdown()
        .await
        .map_err(|message| message.to_owned())?;
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn provider_prompt_error_text_cannot_become_a_local_no_effect_rejection() -> TestResult {
    let root = tempfile::tempdir()?;
    let provider_endpoint = endpoint("cursor-local")?;
    let requester = actor(provider_endpoint.clone(), "requester")?;
    let runtime = ExternalProviderRuntime::initialize(provider_prompt_rejection_fixture()).await?;
    let backend = supervisor(
        &root,
        vec![(binding(provider_endpoint.clone(), "cursor")?, runtime)],
    )
    .await?;
    let create_id = OperationId::generate();
    operation(
        backend
            .create(ConversationCreateRequest {
                operation_id: create_id.clone(),
                endpoint: provider_endpoint,
                generation: Some(generation()?),
                working_directory: working_directory()?,
                created_by: requester.clone(),
                approver: requester.clone(),
                requested_policy: policy(),
            })
            .await,
    )?;
    let created = operation(wait(&backend, create_id).await)?;
    let ConversationOperationWaitOutput::Available {
        settlement: ConversationOperationSettlement::Created { target, .. },
    } = created.output
    else {
        return Err("provider session was not created".into());
    };
    let prompt_id = OperationId::generate();
    operation(
        backend
            .prompt(ConversationPromptRequest {
                operation_id: prompt_id.clone(),
                target,
                generation: Some(generation()?),
                requested_by: requester.clone(),
                approver: requester,
                prompt: MessageContent::HumanUser {
                    text: MessageText::try_from("continue".to_owned())?,
                },
            })
            .await,
    )?;
    let failure = wait(&backend, prompt_id)
        .await
        .expect_err("provider rejection");
    ensure_eq!(
        failure.kind,
        ConversationOperationFailureKind::ProviderRejected
    );
    ensure_eq!(failure.effect, ProviderOperationEffect::Unknown);
    backend
        .shutdown()
        .await
        .map_err(|message| message.to_owned())?;
    Ok(())
}

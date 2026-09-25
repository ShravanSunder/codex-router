use super::*;
use collaboration_client::ControlClient;
use collaboration_protocol::{
    ApprovalDecideParams, ApprovalDecision, EndpointDescription, EndpointId, EndpointRef,
    GenerationNumber, OperationId, SessionId, UuidIdentity,
};
use collaboration_service::{
    EndpointDirectory, NativeControlBackend, NativeGenerationGate, ServiceApprovalBroker,
};
use futures_util::{SinkExt as _, StreamExt as _};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc};
use tokio_tungstenite::tungstenite::Message;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[test]
fn provider_output_classifier_reports_only_bounded_uuid_metadata() {
    let expected = UuidIdentity::try_from("0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89".to_owned())
        .expect("expected service ID");
    let different = "2ff962c5-7fa3-4c18-a5ca-1bbe8db09e81";
    let cases = [
        (
            String::from(expected.clone()),
            ProviderOutputClassification {
                output_empty: false,
                exact_identity_equal: true,
                expected_identity_token_present: true,
                returned_uuid_count: 1,
                sole_uuid_matches_expected: true,
                output_byte_count: 36,
            },
        ),
        (
            format!("`{}`", String::from(expected.clone())),
            ProviderOutputClassification {
                output_empty: false,
                exact_identity_equal: false,
                expected_identity_token_present: true,
                returned_uuid_count: 1,
                sole_uuid_matches_expected: true,
                output_byte_count: 38,
            },
        ),
        (
            format!("service id: {}", String::from(expected.clone())),
            ProviderOutputClassification {
                output_empty: false,
                exact_identity_equal: false,
                expected_identity_token_present: true,
                returned_uuid_count: 1,
                sole_uuid_matches_expected: true,
                output_byte_count: 48,
            },
        ),
        (
            different.to_owned(),
            ProviderOutputClassification {
                output_empty: false,
                exact_identity_equal: false,
                expected_identity_token_present: false,
                returned_uuid_count: 1,
                sole_uuid_matches_expected: false,
                output_byte_count: 36,
            },
        ),
        (
            String::new(),
            ProviderOutputClassification {
                output_empty: true,
                exact_identity_equal: false,
                expected_identity_token_present: false,
                returned_uuid_count: 0,
                sole_uuid_matches_expected: false,
                output_byte_count: 0,
            },
        ),
        (
            "no identity".to_owned(),
            ProviderOutputClassification {
                output_empty: false,
                exact_identity_equal: false,
                expected_identity_token_present: false,
                returned_uuid_count: 0,
                sole_uuid_matches_expected: false,
                output_byte_count: 11,
            },
        ),
        (
            format!("{} {different}", String::from(expected.clone())),
            ProviderOutputClassification {
                output_empty: false,
                exact_identity_equal: false,
                expected_identity_token_present: true,
                returned_uuid_count: 2,
                sole_uuid_matches_expected: false,
                output_byte_count: 73,
            },
        ),
    ];
    for (output, expected_classification) in cases {
        assert_eq!(
            classify_provider_output(&output, &expected),
            expected_classification
        );
    }
    let expected_text = String::from(expected.clone());
    for embedded in [
        format!("x{expected_text}"),
        format!("{expected_text}x"),
        format!("_{expected_text}"),
        format!("{expected_text}_"),
        format!("-{expected_text}"),
        format!("{expected_text}-"),
        format!("0{expected_text}0"),
    ] {
        let classification = classify_provider_output(&embedded, &expected);
        assert_eq!(classification.returned_uuid_count, 0);
        assert!(!classification.expected_identity_token_present);
        assert!(!classification.sole_uuid_matches_expected);
    }
}

fn owned_host_endpoint_fixture() -> ExternalProviderLaunch {
    ExternalProviderLaunch {
        executable: PathBuf::from("/usr/bin/python3"),
        arguments: vec![
            "-c".to_owned(),
            r#"
import json,sys
request=json.loads(sys.stdin.readline())
print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':{'protocolVersion':1,'agentCapabilities':{},'agentInfo':{'name':'owned-result-fixture','version':'1'}}})); sys.stdout.flush()
sys.stdin.read()
"#
            .to_owned(),
        ],
        environment: Vec::new(),
    }
}

fn session_ref(
    service_id: &UuidIdentity,
    endpoint_id: &str,
    session_id: &str,
) -> TestResult<SessionRef> {
    Ok(SessionRef {
        endpoint: EndpointRef {
            service_id: service_id.clone(),
            endpoint_id: EndpointId::try_from(endpoint_id.to_owned())?,
        },
        session_id: SessionId::try_from(session_id.to_owned())?,
    })
}

async fn approval_broker_fixture(
    root: &tempfile::TempDir,
    service_id: &UuidIdentity,
    approver: &SessionRef,
) -> TestResult<(
    Arc<ServiceApprovalBroker>,
    tokio::task::JoinHandle<Result<(), Box<dyn std::error::Error + Send + Sync>>>,
)> {
    let socket_path = root.path().join("native-approver.sock");
    let listener = tokio::net::UnixListener::bind(&socket_path)?;
    let native_generation = CodexGeneration {
        service_epoch: service_id.clone(),
        generation: GenerationNumber::try_from(1)?,
    };
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
        Some(Arc::clone(&schemas)),
    )?;
    let endpoint_description: EndpointDescription = serde_json::from_value(json!({
        "endpoint": approver.endpoint,
        "label": "Fixture approver",
        "availability": {"state":"available","observedAt":"2026-09-21T00:00:00Z"},
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
    let approver_task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await?;
        let mut socket = tokio_tungstenite::accept_async(stream).await?;
        for expected_method in ["initialize", "initialized", "thread/read", "turn/start"] {
            let frame = tokio::time::timeout(Duration::from_secs(5), socket.next())
                .await?
                .ok_or("native approver connection closed")??;
            let request: Value = serde_json::from_str(frame.to_text()?)?;
            if request.get("method").and_then(Value::as_str) != Some(expected_method) {
                return Err(format!("unexpected native method: {expected_method}").into());
            }
            if expected_method == "turn/start" {
                let delivered_text = request
                    .pointer("/params/input/0/text")
                    .and_then(Value::as_str)
                    .ok_or("approval notice text missing")?;
                if !delivered_text.contains("session/request_permission")
                    || !delivered_text.contains("externalProviderPermission")
                {
                    return Err(
                        "approval notice did not reach the approver with its request details"
                            .into(),
                    );
                }
            }
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
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
    });
    Ok((broker, approver_task))
}

fn fixture_acp_permission_request_agent(provider_name: &str) -> ExternalProviderLaunch {
    let fixture = r#"
import json,sys
provider=sys.argv[1]
def send(value):
    print(json.dumps(value)); sys.stdout.flush()
initialize=json.loads(sys.stdin.readline())
send({'jsonrpc':'2.0','id':initialize['id'],'result':{'protocolVersion':1,'agentCapabilities':{},'agentInfo':{'name':provider+'-permission-fixture','version':'1'}}})
create=json.loads(sys.stdin.readline())
send({'jsonrpc':'2.0','id':create['id'],'result':{'sessionId':'fixture-'+provider+'-session'}})
prompt=json.loads(sys.stdin.readline())
assert prompt['method']=='session/prompt'
send({'jsonrpc':'2.0','id':91,'method':'session/request_permission','params':{'sessionId':'fixture-'+provider+'-session','toolCall':{'toolCallId':'echo-command','title':'Run echo cursor-ok','kind':'execute'},'options':[{'optionId':'allow','name':'Allow once','kind':'allow_once'}]}})
permission=json.loads(sys.stdin.readline())
assert permission['id']==91
assert permission['result']['outcome']['outcome']=='selected'
assert permission['result']['outcome']['optionId']=='allow'
send({'jsonrpc':'2.0','method':'session/update','params':{'sessionId':'fixture-'+provider+'-session','update':{'sessionUpdate':'agent_message_chunk','content':{'type':'text','text':'cursor-ok'}}}})
send({'jsonrpc':'2.0','id':prompt['id'],'result':{'stopReason':'end_turn'}})
sys.stdin.read()
"#;
    ExternalProviderLaunch {
        executable: PathBuf::from("/usr/bin/python3"),
        arguments: vec![
            "-c".to_owned(),
            fixture.to_owned(),
            provider_name.to_owned(),
        ],
        environment: Vec::new(),
    }
}

fn fixture_acp_permission_without_allow_agent() -> ExternalProviderLaunch {
    let fixture = r#"
import json,sys
def send(value):
    print(json.dumps(value)); sys.stdout.flush()
initialize=json.loads(sys.stdin.readline())
send({'jsonrpc':'2.0','id':initialize['id'],'result':{'protocolVersion':1,'agentCapabilities':{},'agentInfo':{'name':'permission-refusal-fixture','version':'1'}}})
create=json.loads(sys.stdin.readline())
send({'jsonrpc':'2.0','id':create['id'],'result':{'sessionId':'fixture-refusal-session'}})
prompt=json.loads(sys.stdin.readline())
send({'jsonrpc':'2.0','id':92,'method':'session/request_permission','params':{'sessionId':'fixture-refusal-session','toolCall':{'toolCallId':'refused-command','title':'Run an unapproved command','kind':'execute'},'options':[{'optionId':'reject','name':'Reject once','kind':'reject_once'}]}})
permission=json.loads(sys.stdin.readline())
assert permission['id']==92
assert permission['result']['outcome']['outcome']=='cancelled'
send({'jsonrpc':'2.0','id':prompt['id'],'result':{'stopReason':'cancelled'}})
sys.stdin.read()
"#;
    ExternalProviderLaunch {
        executable: PathBuf::from("/usr/bin/python3"),
        arguments: vec!["-c".to_owned(), fixture.to_owned()],
        environment: Vec::new(),
    }
}

#[cfg(unix)]
#[tokio::test]
async fn fixture_acp_permission_notice_reaches_approver_and_allow_executes_command() -> TestResult {
    let service_id = UuidIdentity::try_from("0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89".to_owned())?;
    let approver = session_ref(&service_id, "codex-local", "approver")?;
    let generation = CodexGeneration {
        service_epoch: service_id.clone(),
        generation: GenerationNumber::try_from(1)?,
    };
    for endpoint_id in ["cursor-local", "claude-local"] {
        let root = tempfile::tempdir()?;
        let runtime =
            ExternalProviderRuntime::initialize(fixture_acp_permission_request_agent(endpoint_id))
                .await?;
        let (broker, mut approver_task) =
            approval_broker_fixture(&root, &service_id, &approver).await?;
        runtime.install_approval_broker(Arc::clone(&broker)).await;

        let provider_session_id = runtime.create_session(root.path().to_owned()).await?;
        let target = session_ref(&service_id, endpoint_id, &provider_session_id)?;
        let prompt = runtime.prompt_with_approval_context(
            provider_session_id,
            format!("Run `echo {endpoint_id}-ok` and report its output."),
            ExternalProviderApprovalContext {
                // The creator is also the prompt sender and default approver.
                requester: approver.clone(),
                approver: approver.clone(),
                target,
                operation_id: OperationId::generate(),
                binding_generation: generation.clone(),
                binding_retirement: CancellationToken::new(),
            },
        );
        tokio::pin!(prompt);
        tokio::select! {
            biased;
            result = &mut prompt => return Err(format!("{endpoint_id} fixture prompt settled before approval notice: {result:?}").into()),
            result = &mut approver_task => result.map_err(|error| format!("approver fixture task: {error}"))??,
        }
        let pending = broker
            .list(true)
            .await
            .approvals
            .into_iter()
            .next()
            .ok_or("delivered approval missing from approval list")?;
        if pending.approver != approver {
            return Err("approval notice delivery did not reach the configured approver".into());
        }
        broker
            .decide(ApprovalDecideParams {
                request_id: pending.request_id.clone(),
                decision: ApprovalDecision::Allow,
                actor: approver.clone(),
            })
            .await
            .map_err(|error| format!("approval decide: {error}"))?;
        let outcome = tokio::time::timeout(Duration::from_secs(5), prompt)
            .await
            .map_err(|_| "fixture ACP permission prompt timed out")??;

        if outcome.output != "cursor-ok" {
            return Err(format!("fixture command output differed: {:?}", outcome.output).into());
        }
        if runtime.permission_observation().last_outcome
            != Some(ExternalProviderPermissionOutcome::Selected)
        {
            return Err(format!("{endpoint_id} fixture ACP permission was not selected").into());
        }
        let history = broker.list(false).await.approvals;
        if history.len() != 1 || history[0].state != collaboration_protocol::ApprovalState::Decided
        {
            return Err(format!(
                "{endpoint_id} approval history did not record the decision: {history:?}"
            )
            .into());
        }
        runtime.shutdown().await;
    }
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn fixture_acp_permission_without_allow_records_visible_refusal_reason() -> TestResult {
    let root = tempfile::tempdir()?;
    let service_id = UuidIdentity::try_from("0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89".to_owned())?;
    let approver = session_ref(&service_id, "codex-local", "approver")?;
    let generation = CodexGeneration {
        service_epoch: service_id.clone(),
        generation: GenerationNumber::try_from(1)?,
    };
    let runtime =
        ExternalProviderRuntime::initialize(fixture_acp_permission_without_allow_agent()).await?;
    let (broker, _approver_task) = approval_broker_fixture(&root, &service_id, &approver).await?;
    runtime.install_approval_broker(Arc::clone(&broker)).await;

    let provider_session_id = runtime.create_session(root.path().to_owned()).await?;
    let target = session_ref(&service_id, "cursor-local", &provider_session_id)?;
    let outcome = runtime
        .prompt_with_approval_context(
            provider_session_id,
            "Run a command requiring permission.".to_owned(),
            ExternalProviderApprovalContext {
                requester: approver.clone(),
                approver,
                target,
                operation_id: OperationId::generate(),
                binding_generation: generation,
                binding_retirement: CancellationToken::new(),
            },
        )
        .await?;

    let history = broker.list(false).await.approvals;
    if history.len() != 1
        || history[0].state != collaboration_protocol::ApprovalState::Cancelled
        || history[0].reason.as_deref() != Some("no one-time allow option is available")
    {
        return Err(
            format!("permission refusal was not recorded with its reason: {history:?}").into(),
        );
    }
    if history[0].offered_options.len() != 1
        || history[0].offered_options[0].option_id != "reject"
        || history[0].offered_options[0].scope
            != collaboration_protocol::ApprovalOptionScope::RejectOnce
    {
        return Err(
            format!("refusal history did not preserve the offered option: {history:?}").into(),
        );
    }
    if outcome.stop_reason != ProviderPromptStopReason::Cancelled {
        return Err(
            format!("provider did not observe permission cancellation: {outcome:?}").into(),
        );
    }
    runtime.shutdown().await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires an explicitly selected authenticated Cursor runtime and live Router MCP"]
async fn live_composed_cursor_native_mcp_requires_typed_call_and_router_result() -> TestResult {
    let executable = std::env::var_os("CODEX_ROUTER_TEST_EXTERNAL_ACP_EXECUTABLE")
        .map(PathBuf::from)
        .ok_or("external ACP executable missing")?;
    let arguments = std::env::var("CODEX_ROUTER_TEST_EXTERNAL_ACP_ARGUMENTS")
        .ok()
        .map(|encoded| serde_json::from_str::<Vec<String>>(&encoded))
        .transpose()?
        .unwrap_or_default();
    let cwd = std::env::var_os("CODEX_ROUTER_TEST_EXTERNAL_ACP_CWD")
        .map(PathBuf::from)
        .unwrap_or(std::env::current_dir()?);
    let root = tempfile::tempdir()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    }
    let host_directory = root.path().join("router");
    let codex_home = root.path().join("codex-home");
    std::fs::create_dir_all(&host_directory)?;
    std::fs::create_dir_all(&codex_home)?;
    #[cfg(unix)]
    for directory in [&host_directory, &codex_home] {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))?;
    }
    let host_runtime = crate::CollaborationRuntime::start_with_external_providers(
        crate::CollaborationRuntimeInputs {
            directory: host_directory.clone(),
            codex_home,
            backend_socket: root.path().join("backend.sock"),
            mcp_bind: std::net::SocketAddr::from(([127, 0, 0, 1], 0)),
            native_schema: None,
            peer_registry_directory: None,
        },
        vec![crate::ExternalProviderStartup::Launch(
            crate::ExternalProviderLaunchBinding::claude(
                owned_host_endpoint_fixture().executable,
                owned_host_endpoint_fixture().arguments,
            )?,
        )],
    )
    .await?;
    let manifest: collaboration_protocol::ServiceManifest =
        serde_json::from_slice(&std::fs::read(host_directory.join("service.json"))?)?;
    let service_id = manifest.service_id.clone();
    let mut control =
        ControlClient::connect(&host_directory, "cursor-native-live-proof", "1").await?;
    let actual_inventory = control.list_endpoints().await?;
    let actual_result_has_owned_service = actual_inventory
        .endpoints
        .iter()
        .any(|description| description.endpoint.service_id == service_id);
    if !actual_result_has_owned_service {
        host_runtime.shutdown().await?;
        return Err("owned endpoints_list result omitted the selected Router identity".into());
    }
    let requester = session_ref(&service_id, "cursor-local", "requester")?;
    let approver = session_ref(&service_id, "codex-local", "approver")?;
    let generation = CodexGeneration {
        service_epoch: service_id.clone(),
        generation: GenerationNumber::try_from(1)?,
    };
    let runtime = ExternalProviderRuntime::initialize_with_mcp_http(
        ExternalProviderLaunch {
            executable,
            arguments,
            environment: Vec::new(),
        },
        "router-collaboration",
        manifest.mcp.url,
    )
    .await;
    let runtime = match runtime {
        Ok(runtime) => runtime,
        Err(error) => {
            host_runtime.shutdown().await?;
            return Err(error.into());
        }
    };
    let (broker, approver_task) = match approval_broker_fixture(&root, &service_id, &approver).await
    {
        Ok(fixture) => fixture,
        Err(error) => {
            runtime.shutdown().await;
            host_runtime.shutdown().await?;
            return Err(error);
        }
    };
    runtime.install_approval_broker(Arc::clone(&broker)).await;
    let live_result: TestResult = async {
        let provider_session_id = runtime.create_session(cwd).await?;
        let target = session_ref(&service_id, "cursor-local", &provider_session_id)?;
        let operation_id = OperationId::generate();
        let retirement = CancellationToken::new();
        let prompt = runtime.prompt_with_approval_context(
            provider_session_id,
            "Call router-collaboration-endpoints_list once. In its JSON result, select the endpoints item whose endpoint.endpointId is claude-local, then return only endpoint.serviceId, without backticks or explanation. Do not use serviceEpoch."
                .to_owned(),
            ExternalProviderApprovalContext {
                requester,
                approver: approver.clone(),
                target,
                operation_id,
                binding_generation: generation,
                binding_retirement: retirement,
            },
        );
        tokio::pin!(prompt);
        let prompt_result = tokio::time::timeout(Duration::from_secs(90), async {
            loop {
                tokio::select! {
                    result = &mut prompt => break result,
                    () = tokio::time::sleep(Duration::from_millis(25)) => {
                        if let Some(pending) = broker.list(true).await.approvals.into_iter().next() {
                            broker.decide(ApprovalDecideParams {
                                request_id: pending.request_id,
                                decision: ApprovalDecision::Allow,
                                actor: approver.clone(),
                            }).await.map_err(|error| {
                                ExternalProviderRuntimeError::Operation(error.to_owned())
                            })?;
                        }
                    }
                }
            }
        })
        .await;
        let tool_calls = runtime.take_test_tool_calls();
        let expected_service_id = String::from(service_id.clone());
        let output_classification = prompt_result
            .as_ref()
            .ok()
            .and_then(|result| result.as_ref().ok())
            .map(|outcome| classify_provider_output(&outcome.output, &service_id));
        let matching_tool = tool_calls.iter().find(|tool_call| {
            tool_call.name.as_deref() == Some("router-collaboration-endpoints_list")
                || tool_call.title == "router-collaboration: endpoints_list"
        });
        let stop_reason = prompt_result
            .as_ref()
            .ok()
            .and_then(|result| result.as_ref().ok())
            .map(|outcome| outcome.stop_reason);
        let identity_corroborated = prompt_result
            .as_ref()
            .ok()
            .and_then(|result| result.as_ref().ok())
            .is_some_and(|outcome| outcome.output.trim() == expected_service_id);
        let permission = runtime.permission_observation();
        eprintln!(
            "native_mcp_receipt tool_observed={} tool_status={:?} tool_outcome={:?} identity_corroborated={} output_empty={:?} exact_identity_equal={:?} expected_identity_token_present={:?} returned_uuid_count={:?} sole_uuid_matches_expected={:?} output_byte_count={:?} permission_count={} permission_outcome={:?} stop_reason={stop_reason:?}",
            matching_tool.is_some(),
            matching_tool.map(|tool_call| tool_call.status),
            matching_tool.map(|tool_call| tool_call.outcome),
            identity_corroborated,
            output_classification.as_ref().map(|output| output.output_empty),
            output_classification.as_ref().map(|output| output.exact_identity_equal),
            output_classification.as_ref().map(|output| output.expected_identity_token_present),
            output_classification.as_ref().map(|output| output.returned_uuid_count),
            output_classification.as_ref().map(|output| output.sole_uuid_matches_expected),
            output_classification.as_ref().map(|output| output.output_byte_count),
            permission.request_count,
            permission.last_outcome,
        );
        match prompt_result {
            Ok(Ok(outcome))
                if accepts_native_router_result(
                    &tool_calls,
                    &outcome.output,
                    &service_id,
                ) =>
            {
                Ok(())
            }
            Ok(Ok(_)) if !has_completed_router_endpoints_call(&tool_calls) => Err(
                "native MCP acceptance requires a completed exact kindOther tool call with successful rawOutput"
                    .into(),
            ),
            Ok(Ok(_)) => Err("provider answer did not corroborate the owned service ID".into()),
            Ok(Err(error)) => Err(error.into()),
            Err(_) => Err("provider prompt timed out".into()),
        }
    }
    .await;

    runtime.shutdown().await;
    let approver_result: TestResult = if approver_task.is_finished() {
        approver_task.await.map_err(|error| error.to_string())?
    } else {
        approver_task.abort();
        let joined = approver_task.await;
        if joined.is_err_and(|error| !error.is_cancelled()) {
            Err("approver task failed during cleanup".into())
        } else {
            Ok(())
        }
    };
    let host_result = host_runtime.shutdown().await.map_err(Into::into);
    live_result?;
    approver_result?;
    host_result
}

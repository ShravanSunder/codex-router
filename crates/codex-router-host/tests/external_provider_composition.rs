use codex_router_host::{
    CollaborationRuntime, CollaborationRuntimeInputs, ExternalProviderLaunchBinding,
    ExternalProviderStartup,
};
use collaboration_client::{ControlClient, MessageSendRequest, PublicMessageContent};
use collaboration_protocol::{
    ChannelDescription, CodexGeneration, ConversationCreateRequest,
    ConversationOperationSettlement, ConversationOperationWaitOutput,
    ConversationOperationWaitRequest, ConversationPromptRequest, DeliveryOutcome,
    EndpointAvailability, EndpointId, EndpointRef, MessageContent, MessageText, OperationId,
    PositiveSeconds, ProviderRequestedPolicy, ProviderWorkingDirectory, RouterAccess, SessionId,
    SessionRef,
};
use std::os::unix::fs::PermissionsExt as _;

#[tokio::test]
async fn initialized_control_reaches_host_owned_provider_and_reuses_target() {
    let root = tempfile::tempdir().expect("runtime root");
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))
        .expect("private runtime root");
    let provider = root.path().join("provider.py");
    std::fs::write(
        &provider,
        r#"#!/usr/bin/python3
import json,sys
request=json.loads(sys.stdin.readline())
print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':{'protocolVersion':1,'agentCapabilities':{'loadSession':True},'agentInfo':{'name':'composition-fixture','version':'1'},'_meta':{'steering':{'supported':True}}}})); sys.stdout.flush()
request=json.loads(sys.stdin.readline())
assert request['method']=='session/new'
print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':{'sessionId':'provider-session'}})); sys.stdout.flush()
request=json.loads(sys.stdin.readline())
assert request['method']=='session/prompt'
print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':{'stopReason':'end_turn'}})); sys.stdout.flush()
steer=json.loads(sys.stdin.readline())
assert steer['method']=='_session/steering'
print(json.dumps({'jsonrpc':'2.0','id':steer['id'],'result':{'outcome':'promptRequired'}})); sys.stdout.flush()
prompt=json.loads(sys.stdin.readline())
assert prompt['method']=='session/prompt'
print(json.dumps({'jsonrpc':'2.0','id':prompt['id'],'result':{'stopReason':'end_turn'}})); sys.stdout.flush()
sys.stdin.read()
"#,
    )
    .expect("provider fixture");
    std::fs::set_permissions(&provider, std::fs::Permissions::from_mode(0o700))
        .expect("provider executable");

    let runtime = CollaborationRuntime::start_with_external_providers(
        CollaborationRuntimeInputs {
            directory: root.path().to_owned(),
            codex_home: root.path().to_owned(),
            backend_socket: root.path().join("backend.sock"),
            mcp_bind: std::net::SocketAddr::from(([127, 0, 0, 1], 0)),
            native_schema: None,
            peer_registry_directory: None,
        },
        vec![ExternalProviderStartup::Launch(
            ExternalProviderLaunchBinding::claude(provider, Vec::new()).expect("provider binding"),
        )],
    )
    .await
    .expect("collaboration runtime");
    let mut client = ControlClient::connect(root.path(), "provider-composition", "1")
        .await
        .expect("Control client");
    let inventory = client.list_endpoints().await.expect("endpoint inventory");
    let provider_endpoint = inventory
        .endpoints
        .iter()
        .find(|endpoint| String::from(endpoint.endpoint.endpoint_id.clone()) == "claude-local")
        .expect("provider endpoint");
    let generation_number = provider_endpoint
        .channels
        .iter()
        .find_map(|channel| match channel {
            ChannelDescription::ExternalProvider {
                binding_generation, ..
            } => Some(*binding_generation),
            _ => None,
        })
        .expect("provider generation");
    let generation = CodexGeneration {
        service_epoch: inventory.service_epoch,
        generation: generation_number,
    };
    let actor = SessionRef {
        endpoint: EndpointRef {
            service_id: provider_endpoint.endpoint.service_id.clone(),
            endpoint_id: EndpointId::try_from("codex-local".to_owned()).expect("actor endpoint"),
        },
        session_id: SessionId::try_from("composition-caller".to_owned()).expect("actor session"),
    };
    let create_operation = OperationId::generate();
    client
        .create_provider_conversation(ConversationCreateRequest {
            operation_id: create_operation.clone(),
            endpoint: provider_endpoint.endpoint.clone(),
            generation: Some(generation.clone()),
            working_directory: ProviderWorkingDirectory::try_from(
                root.path().display().to_string(),
            )
            .expect("working directory"),
            created_by: actor.clone(),
            approver: actor.clone(),
            requested_policy: ProviderRequestedPolicy {
                access: RouterAccess::WriteRestricted,
            },
        })
        .await
        .expect("create admitted");
    let create = client
        .wait_for_provider_conversation_operation(ConversationOperationWaitRequest {
            operation_id: create_operation,
            timeout_seconds: PositiveSeconds::try_from(3).expect("timeout"),
        })
        .await
        .expect("create settlement");
    let ConversationOperationWaitOutput::Available {
        settlement: ConversationOperationSettlement::Created { target, .. },
    } = create.output
    else {
        panic!("create did not return its provider target")
    };

    let prompt_operation = OperationId::generate();
    client
        .prompt_provider_conversation(ConversationPromptRequest {
            operation_id: prompt_operation.clone(),
            target: target.clone(),
            generation: Some(generation),
            requested_by: actor.clone(),
            approver: actor.clone(),
            prompt: MessageContent::Agent {
                sender: actor,
                text: MessageText::try_from("composition prompt".to_owned()).expect("prompt"),
            },
        })
        .await
        .expect("prompt admitted");
    let prompt = client
        .wait_for_provider_conversation_operation(ConversationOperationWaitRequest {
            operation_id: prompt_operation,
            timeout_seconds: PositiveSeconds::try_from(3).expect("timeout"),
        })
        .await
        .expect("prompt settlement");
    assert!(matches!(
        prompt.output,
        ConversationOperationWaitOutput::Available {
            settlement: ConversationOperationSettlement::PromptCompleted {
                target: settled_target,
                ..
            }
        } if settled_target == target
    ));

    let receipt = client
        .send_message(MessageSendRequest {
            target,
            message: PublicMessageContent::HumanUser {
                text: MessageText::try_from("routed provider message".to_owned()).expect("message"),
            },
            delivery: collaboration_protocol::MessageDelivery::Auto,
            generation_guard: None,
            correlation: None,
        })
        .await
        .expect("message receipt");
    assert!(
        matches!(receipt.outcome, DeliveryOutcome::Started),
        "{receipt:?}"
    );

    runtime.shutdown().await.expect("runtime shutdown");
}

#[tokio::test]
async fn provider_process_exit_retires_advertisement_and_rejects_new_admission() {
    let root = tempfile::tempdir().expect("runtime root");
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))
        .expect("private runtime root");
    let provider = root.path().join("exit-provider.py");
    std::fs::write(&provider, r#"#!/usr/bin/python3
import json,sys
request=json.loads(sys.stdin.readline())
print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':{'protocolVersion':1,'agentCapabilities':{},'agentInfo':{'name':'exit-fixture','version':'1'}}})); sys.stdout.flush()
"#).expect("provider fixture");
    std::fs::set_permissions(&provider, std::fs::Permissions::from_mode(0o700))
        .expect("provider executable");
    let mut runtime = CollaborationRuntime::start_with_external_providers(
        CollaborationRuntimeInputs {
            directory: root.path().to_owned(),
            codex_home: root.path().to_owned(),
            backend_socket: root.path().join("backend.sock"),
            mcp_bind: std::net::SocketAddr::from(([127, 0, 0, 1], 0)),
            native_schema: None,
            peer_registry_directory: None,
        },
        vec![ExternalProviderStartup::Launch(
            ExternalProviderLaunchBinding::claude(provider, Vec::new()).expect("binding"),
        )],
    )
    .await
    .expect("runtime");
    let service_id = runtime.service_id().clone();
    let service_epoch = runtime.service_epoch().clone();
    let mut client = ControlClient::connect(root.path(), "retirement-proof", "1")
        .await
        .expect("client");
    let mut listener_failure = Box::pin(runtime.listener_failure());
    let provider_endpoint = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let inventory = tokio::select! {
                failure = &mut listener_failure => panic!("provider retirement killed collaboration listeners: {failure}"),
                inventory = client.list_endpoints() => inventory.expect("inventory"),
            };
            if let Some(endpoint) = inventory.endpoints.into_iter().find(|endpoint| {
                String::from(endpoint.endpoint.endpoint_id.clone()) == "claude-local"
                    && matches!(
                        endpoint.availability,
                        EndpointAvailability::Unavailable { .. }
                    )
            }) {
                break endpoint;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("provider retirement published");
    let failure = client
        .create_provider_conversation(ConversationCreateRequest {
            operation_id: OperationId::generate(),
            endpoint: provider_endpoint.endpoint,
            generation: Some(CodexGeneration {
                service_epoch,
                generation: 1_u64.try_into().expect("generation"),
            }),
            working_directory: ProviderWorkingDirectory::try_from(
                root.path().display().to_string(),
            )
            .expect("cwd"),
            created_by: SessionRef {
                endpoint: EndpointRef {
                    service_id: service_id.clone(),
                    endpoint_id: EndpointId::try_from("codex-local".to_owned()).expect("endpoint"),
                },
                session_id: SessionId::try_from("caller".to_owned()).expect("session"),
            },
            approver: SessionRef {
                endpoint: EndpointRef {
                    service_id,
                    endpoint_id: EndpointId::try_from("codex-local".to_owned()).expect("endpoint"),
                },
                session_id: SessionId::try_from("caller".to_owned()).expect("session"),
            },
            requested_policy: ProviderRequestedPolicy {
                access: RouterAccess::WriteRestricted,
            },
        })
        .await
        .expect_err("retired provider rejects admission");
    assert!(matches!(
        failure,
        collaboration_client::ClientError::Rejected { .. }
    ));
    let mut surviving_client = ControlClient::connect(root.path(), "retirement-survivor", "1")
        .await
        .expect("fresh Control connection survives provider retirement");
    let inventory = tokio::select! {
        failure = &mut listener_failure => panic!("provider retirement killed collaboration listeners: {failure}"),
        inventory = surviving_client.list_endpoints() => inventory.expect("Control remains usable"),
    };
    assert!(inventory.endpoints.iter().any(|endpoint| {
        String::from(endpoint.endpoint.endpoint_id.clone()) == "claude-local"
            && matches!(
                endpoint.availability,
                EndpointAvailability::Unavailable { .. }
            )
    }));
    drop(listener_failure);
    runtime.shutdown().await.expect("shutdown");
}

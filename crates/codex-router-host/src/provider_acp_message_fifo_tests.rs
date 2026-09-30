#![allow(clippy::expect_used)]

use super::*;
use crate::{
    ExternalProviderBinding, ExternalProviderLaunch, ExternalProviderRuntime,
    ExternalProviderSupervisor, LiveSessionOwnership, LiveSessionOwnershipCheck,
    ProviderAcpDeliveryRoute, ProviderSessionActivity,
};
use agent_automation::RouteEffectEvidence;
use collaboration_protocol::{
    ChannelDescription, CodexGeneration, ConversationCloseRequest, ConversationOperationQueueState,
    ConversationOperationSettlement, ConversationOperationShowRequest,
    ConversationOperationWaitOutput, ConversationOperationWaitRequest, DeliveryCorrelationId,
    DeliveryOutcome, EndpointAvailability, EndpointDescription, EndpointId, EndpointRef,
    GenerationNumber, MessageContent, MessageDelivery, MessageText, NonEmptyText,
    ObservationTimestamp, OperationId, PositiveSeconds, ProviderBindingId, ProviderBindingIdentity,
    ProviderCapabilities, ProviderCapability, ProviderCapabilityEvidence, ProviderCapabilityName,
    ProviderCapabilityStatus, ProviderKind, ProviderRequestedPolicy, ProviderRuntimeIdentity,
    ProviderTransport, ProviderWorkingDirectory, RouterAccess, SessionId,
};
use collaboration_service::{
    AttemptEvidenceSink, DeliveryContractError, DeliveryFuture, DeliveryPrecondition,
    DeliveryRequest, EndpointDirectory, LoadPolicy, ProviderConversationBackend,
    ProviderOperationStore, ProviderSessionRecord, SessionDeliveryRoute,
};
use std::{path::PathBuf, sync::Arc};

struct NoLivePeer;

impl LiveSessionOwnershipCheck for NoLivePeer {
    fn check<'a>(&'a self, _target: &'a SessionRef) -> DeliveryFuture<'a, LiveSessionOwnership> {
        Box::pin(async { Ok(LiveSessionOwnership::NotLive) })
    }
}

fn target() -> SessionRef {
    SessionRef {
        endpoint: EndpointRef {
            service_id: "0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89"
                .to_owned()
                .try_into()
                .expect("service ID"),
            endpoint_id: EndpointId::try_from("cursor-local".to_owned()).expect("endpoint ID"),
        },
        session_id: SessionId::try_from("fifo-session".to_owned()).expect("session ID"),
    }
}

fn binding(target: &SessionRef) -> ProviderBindingIdentity {
    ProviderBindingIdentity {
        endpoint: target.endpoint.clone(),
        binding_id: ProviderBindingId::try_from("fifo-binding".to_owned()).expect("binding ID"),
        runtime: ProviderRuntimeIdentity {
            provider: ProviderKind::Cursor,
            runtime_name: NonEmptyText::try_from("fifo-fixture".to_owned()).expect("runtime name"),
            runtime_version: None,
        },
        transport: ProviderTransport::StdioAcp,
        generation: CodexGeneration {
            service_epoch: "1ff962c5-7fa3-4c18-a5ca-1bbe8db09e80"
                .to_owned()
                .try_into()
                .expect("service epoch"),
            generation: GenerationNumber::try_from(1).expect("generation"),
        },
        capabilities: ProviderCapabilities::try_from(vec![
            ProviderCapability {
                name: ProviderCapabilityName::Prompt,
                status: ProviderCapabilityStatus::Supported,
                evidence: ProviderCapabilityEvidence::Advertised,
            },
            ProviderCapability {
                name: ProviderCapabilityName::Load,
                status: ProviderCapabilityStatus::Supported,
                evidence: ProviderCapabilityEvidence::Advertised,
            },
        ])
        .expect("capabilities"),
    }
}

fn close_and_load_fixture(
    close_marker: &std::path::Path,
    load_marker: &std::path::Path,
) -> ExternalProviderLaunch {
    let script = format!(
        r#"
import json,sys
def read(): return json.loads(sys.stdin.readline())
def send(value): print(json.dumps(value),flush=True)
request=read()
assert request['method']=='initialize'
send({{'jsonrpc':'2.0','id':request['id'],'result':{{'protocolVersion':1,
 'agentCapabilities':{{'loadSession':True,'sessionCapabilities':{{'close':{{}}}}}},
 'agentInfo':{{'name':'fifo-close-fixture','version':'1'}}}}}})
request=read()
assert request['method']=='session/new'
send({{'jsonrpc':'2.0','id':request['id'],'result':{{'sessionId':'fifo-session'}}}})
for line in sys.stdin:
 request=json.loads(line)
 if request['method']=='session/close':
  with open({:?},'w') as marker: marker.write('closed')
 elif request['method']=='session/load':
  with open({:?},'w') as marker: marker.write('loaded')
 else:
  raise AssertionError(request['method'])
 send({{'jsonrpc':'2.0','id':request['id'],'result':{{}}}})
"#,
        close_marker.display().to_string(),
        load_marker.display().to_string(),
    );
    ExternalProviderLaunch {
        persistence_target: acp_client_runtime::ProviderPersistenceTarget::Unspecified,
        executable: PathBuf::from("/usr/bin/python3"),
        arguments: vec!["-c".to_owned(), script],
        environment: Vec::new(),
    }
}

#[tokio::test]
async fn queued_loaded_only_item_not_submitted_if_close_happens_before_fifo_drain() {
    let root = tempfile::tempdir().expect("fixture root");
    let close_marker = root.path().join("close-marker.txt");
    let load_marker = root.path().join("load-marker.txt");
    let target = target();
    let binding = binding(&target);
    let runtime =
        ExternalProviderRuntime::initialize(close_and_load_fixture(&close_marker, &load_marker))
            .await
            .expect("fixture provider");
    assert_eq!(
        runtime
            .create_session(root.path().to_owned())
            .await
            .expect("session/new"),
        "fifo-session"
    );
    let store = Arc::new(Mutex::new(
        ProviderOperationStore::open(&root.path().join("operations.sqlite"))
            .await
            .expect("store"),
    ));
    store
        .lock()
        .await
        .record_session(&ProviderSessionRecord {
            target: target.clone(),
            working_directory: ProviderWorkingDirectory::try_from(
                root.path().display().to_string(),
            )
            .expect("working directory"),
            requested_policy: ProviderRequestedPolicy {
                access: RouterAccess::WriteRestricted,
            },
            created_by: target.clone().into(),
            approver: target.clone().into(),
            updated_at_ms: 1,
        })
        .await
        .expect("session record");
    let supervisor = Arc::new(
        ExternalProviderSupervisor::new(
            vec![ExternalProviderBinding {
                identity: binding.clone(),
                runtime,
            }],
            Arc::clone(&store),
        )
        .expect("supervisor"),
    );

    let operation_id = OperationId::generate();
    let input_id = session_event_model::InputId::generate();
    let prompt = MessageContent::Router {
        text: MessageText::try_from("held batch".to_owned()).expect("prompt"),
    };
    supervisor.queued_operation_registry().record_queued(
        operation_id.clone(),
        target.clone(),
        binding.clone(),
        input_id.clone(),
        &prompt,
    );
    let (sender, receiver) = mpsc::channel(1);
    sender
        .send(ProviderQueuedPrompt::MessageWithHeader {
            request: ConversationPromptRequest {
                operation_id: operation_id.clone(),
                input_id: Some(input_id),
                target: target.clone(),
                generation: Some(binding.generation.clone()),
                requested_by: target.clone().into(),
                approver: target.clone().into(),
                prompt,
            },
            header_context: collaboration_protocol::MessageHeaderContext::default(),
            load_policy: LoadPolicy::LoadedOnly,
        })
        .await
        .expect("accepted queue item");
    drop(sender);

    supervisor
        .runtime_for(&target.endpoint)
        .expect("provider runtime")
        .close_session(String::from(target.session_id.clone()))
        .await
        .expect("ConversationClose unloads provider session");
    assert_eq!(
        supervisor
            .runtime_for(&target.endpoint)
            .expect("provider runtime")
            .session_activity(String::from(target.session_id.clone()))
            .await
            .expect("session activity"),
        ProviderSessionActivity::NotLoaded
    );

    let worker = tokio::spawn(run_provider_message_fifo(
        target.clone(),
        receiver,
        Arc::clone(&supervisor),
        Arc::clone(&store),
        Arc::new(NoLivePeer),
        CancellationToken::new(),
    ));
    tokio::time::timeout(std::time::Duration::from_secs(2), worker)
        .await
        .expect("FIFO worker drains accepted item")
        .expect("FIFO worker completes");

    let snapshot = supervisor
        .show(ConversationOperationShowRequest { operation_id })
        .await
        .expect("queued operation snapshot");
    assert_eq!(
        snapshot.queue_state,
        Some(
            collaboration_protocol::ConversationOperationQueueState::NotSubmitted {
                reason: NOT_LOADED_REASON.to_owned(),
            }
        )
    );
    assert!(
        close_marker.exists(),
        "fixture close must complete before FIFO drain"
    );
    assert!(
        !load_marker.exists(),
        "LoadedOnly FIFO must not call session/load"
    );
    supervisor.shutdown().await.expect("supervisor shutdown");
}

struct CloseDuringQueueAcceptance {
    supervisor: Arc<ExternalProviderSupervisor>,
    target: SessionRef,
}

impl AttemptEvidenceSink for CloseDuringQueueAcceptance {
    fn record(
        &self,
        _: RouteEffectEvidence<SessionRef, CodexGeneration>,
    ) -> DeliveryFuture<'_, ()> {
        let supervisor = Arc::clone(&self.supervisor);
        let target = self.target.clone();
        Box::pin(async move {
            let operation_id = OperationId::generate();
            supervisor
                .close(ConversationCloseRequest {
                    operation_id: operation_id.clone(),
                    target: target.clone(),
                    generation: None,
                    requested_by: target.clone().into(),
                    approver: target.clone().into(),
                })
                .await
                .map_err(|_| DeliveryContractError::ClientOperation)?;
            let timeout_seconds =
                PositiveSeconds::try_from(5).map_err(|_| DeliveryContractError::ClientOperation)?;
            let closed = supervisor
                .wait(ConversationOperationWaitRequest {
                    operation_id,
                    timeout_seconds,
                })
                .await
                .map_err(|_| DeliveryContractError::ClientOperation)?;
            if !matches!(
                closed.output,
                ConversationOperationWaitOutput::Available {
                    settlement: ConversationOperationSettlement::Closed { target: closed_target },
                } if closed_target == target
            ) {
                return Err(DeliveryContractError::ClientOperation);
            }
            Ok(())
        })
    }
}

#[tokio::test]
async fn route_queue_loaded_only_delivery_refuses_after_close_before_fifo_submission() {
    let root = tempfile::tempdir().expect("fixture root");
    let close_marker = root.path().join("close-marker.txt");
    let load_marker = root.path().join("load-marker.txt");
    let target = target();
    let provider_binding = binding(&target);
    let runtime =
        ExternalProviderRuntime::initialize(close_and_load_fixture(&close_marker, &load_marker))
            .await
            .expect("fixture provider");
    assert_eq!(
        runtime
            .create_session(root.path().to_owned())
            .await
            .expect("session/new"),
        "fifo-session"
    );
    let store = Arc::new(Mutex::new(
        ProviderOperationStore::open(&root.path().join("operations.sqlite"))
            .await
            .expect("store"),
    ));
    store
        .lock()
        .await
        .record_session(&ProviderSessionRecord {
            target: target.clone(),
            working_directory: ProviderWorkingDirectory::try_from(
                root.path().display().to_string(),
            )
            .expect("working directory"),
            requested_policy: ProviderRequestedPolicy {
                access: RouterAccess::WriteRestricted,
            },
            created_by: target.clone().into(),
            approver: target.clone().into(),
            updated_at_ms: 1,
        })
        .await
        .expect("session record");
    let supervisor = Arc::new(
        ExternalProviderSupervisor::new(
            vec![ExternalProviderBinding {
                identity: provider_binding.clone(),
                runtime,
            }],
            Arc::clone(&store),
        )
        .expect("supervisor"),
    );
    let directory = EndpointDirectory::new(target.endpoint.service_id.clone());
    directory
        .publish(EndpointDescription {
            endpoint: target.endpoint.clone(),
            label: NonEmptyText::try_from("Cursor fixture".to_owned()).expect("label"),
            availability: EndpointAvailability::Available {
                observed_at: ObservationTimestamp::try_from("2026-09-24T12:00:00Z".to_owned())
                    .expect("observation time"),
            },
            channels: vec![ChannelDescription::ExternalProvider {
                transport: ProviderTransport::StdioAcp,
                binding_id: provider_binding.binding_id.clone(),
                binding_generation: provider_binding.generation.generation,
                runtime: provider_binding.runtime.clone(),
                capabilities: provider_binding.capabilities.clone(),
            }],
        })
        .expect("endpoint publication");
    let route = ProviderAcpDeliveryRoute::new(
        target.endpoint.service_id.clone(),
        std::iter::once(target.endpoint.clone()).collect(),
        directory,
        Arc::clone(&supervisor),
        store,
        Arc::new(NoLivePeer),
    );
    let attempt_id = agent_automation::AttemptId::generate();
    let operation_id =
        OperationId::try_from(attempt_id.as_str().to_owned()).expect("queue operation ID");
    let request = DeliveryRequest {
        target: target.clone(),
        message: MessageContent::Router {
            text: MessageText::try_from("held batch".to_owned()).expect("message"),
        },
        header_context: collaboration_protocol::MessageHeaderContext::default(),
        mode: MessageDelivery::Queue,
        load_policy: LoadPolicy::LoadedOnly,
        precondition: DeliveryPrecondition::Unpinned,
        correlation: DeliveryCorrelationId::generate(),
        attempt: attempt_id,
    };
    let receipt = route
        .deliver(
            request,
            &CloseDuringQueueAcceptance {
                supervisor: Arc::clone(&supervisor),
                target: target.clone(),
            },
        )
        .await
        .expect("queued delivery receipt");
    assert!(matches!(receipt.outcome, DeliveryOutcome::Queued));
    assert!(close_marker.exists(), "session close precedes FIFO enqueue");

    let queue_state = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        supervisor
            .queued_operation_registry()
            .wait_for_not_submitted(&operation_id),
    )
    .await
    .expect("FIFO observes the closed session");
    assert_eq!(
        queue_state,
        ConversationOperationQueueState::NotSubmitted {
            reason: collaboration_service::NOT_LOADED_REASON.to_owned(),
        }
    );
    assert!(
        !load_marker.exists(),
        "LoadedOnly queue submission must not call session/load"
    );
    route.shutdown_queue().await;
    supervisor.shutdown().await.expect("supervisor shutdown");
}

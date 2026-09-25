#![allow(clippy::expect_used)]
//! Provider route decisions before any client effect.

use agent_automation::{
    ProviderAcpEffectEvidence, ProviderBindingReference, ProviderSettlementEffect,
    RouteEffectEvidence, SubmissionEffect,
};
use codex_router_host::{
    ExternalProviderBinding, ExternalProviderLaunch, ExternalProviderRuntime,
    ExternalProviderSupervisor, LiveSessionOwnership, LiveSessionOwnershipCheck,
    ProviderAcpDeliveryRoute,
};
use collaboration_protocol::{
    AttemptId, ChannelDescription, CodexGeneration, DeliveryCorrelationId, DeliveryOutcome,
    DeliveryRejectionReason, EndpointAvailability, EndpointDescription, EndpointId, EndpointRef,
    GenerationNumber, MessageContent, MessageDelivery, MessageText, NonEmptyText,
    ObservationTimestamp, ProviderBindingId, ProviderBindingIdentity, ProviderCapabilities,
    ProviderCapability, ProviderCapabilityEvidence, ProviderCapabilityName,
    ProviderCapabilityStatus, ProviderKind, ProviderOperationEffect, ProviderOperationKind,
    ProviderReconciliationState, ProviderRequestedPolicy, ProviderRuntimeIdentity,
    ProviderTransport, ProviderWorkingDirectory, RouterAccess, SessionId, SessionRef, UuidIdentity,
};
use collaboration_service::{
    AttemptEvidenceSink, AttemptReconciliation, AttemptReconciliationContext, DeliveryFuture,
    DeliveryPrecondition, DeliveryRequest, EndpointDirectory, ProviderOperationAdmission,
    ProviderOperationStore, ProviderSessionRecord, SessionDeliveryRoute,
};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

struct NoLivePeer;

struct LivePeer;

impl LiveSessionOwnershipCheck for NoLivePeer {
    fn check<'a>(&'a self, _target: &'a SessionRef) -> DeliveryFuture<'a, LiveSessionOwnership> {
        Box::pin(async { Ok(LiveSessionOwnership::NotLive) })
    }
}

impl LiveSessionOwnershipCheck for LivePeer {
    fn check<'a>(&'a self, _target: &'a SessionRef) -> DeliveryFuture<'a, LiveSessionOwnership> {
        Box::pin(async { Ok(LiveSessionOwnership::LiveUnsupported) })
    }
}

struct RecordedEvidence(tokio::sync::Mutex<Vec<RouteEffectEvidence<SessionRef, CodexGeneration>>>);

impl AttemptEvidenceSink for RecordedEvidence {
    fn record(
        &self,
        evidence: RouteEffectEvidence<SessionRef, CodexGeneration>,
    ) -> DeliveryFuture<'_, ()> {
        Box::pin(async move {
            self.0.lock().await.push(evidence);
            Ok(())
        })
    }
}

fn target() -> SessionRef {
    SessionRef {
        endpoint: EndpointRef {
            service_id: UuidIdentity::try_from("0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89".to_owned())
                .expect("service ID"),
            endpoint_id: EndpointId::try_from("cursor-local".to_owned()).expect("endpoint"),
        },
        session_id: SessionId::try_from("fixture-session".to_owned()).expect("session"),
    }
}

fn provider_binding(target: &SessionRef) -> ProviderBindingIdentity {
    ProviderBindingIdentity {
        endpoint: target.endpoint.clone(),
        binding_id: ProviderBindingId::try_from("cursor-fixture-binding".to_owned())
            .expect("binding"),
        runtime: ProviderRuntimeIdentity {
            provider: ProviderKind::Cursor,
            runtime_name: NonEmptyText::try_from("cursor-fixture".to_owned()).expect("runtime"),
            runtime_version: None,
        },
        transport: ProviderTransport::StdioAcp,
        generation: CodexGeneration {
            service_epoch: UuidIdentity::try_from(
                "1ff962c5-7fa3-4c18-a5ca-1bbe8db09e80".to_owned(),
            )
            .expect("epoch"),
            generation: GenerationNumber::try_from(1).expect("generation"),
        },
        capabilities: ProviderCapabilities::try_from(vec![ProviderCapability {
            name: ProviderCapabilityName::Prompt,
            status: ProviderCapabilityStatus::Supported,
            evidence: ProviderCapabilityEvidence::Advertised,
        }])
        .expect("capabilities"),
    }
}

fn available_directory(
    target: &SessionRef,
    binding: &ProviderBindingIdentity,
) -> EndpointDirectory {
    let directory = EndpointDirectory::new(target.endpoint.service_id.clone());
    directory
        .publish(EndpointDescription {
            endpoint: target.endpoint.clone(),
            label: NonEmptyText::try_from("Cursor".to_owned()).expect("label"),
            availability: EndpointAvailability::Available {
                observed_at: ObservationTimestamp::try_from("2026-09-24T12:00:00Z".to_owned())
                    .expect("time"),
            },
            channels: vec![ChannelDescription::ExternalProvider {
                transport: ProviderTransport::StdioAcp,
                binding_id: binding.binding_id.clone(),
                binding_generation: binding.generation.generation,
                runtime: binding.runtime.clone(),
                capabilities: binding.capabilities.clone(),
            }],
        })
        .expect("endpoint");
    directory
}

fn cursor_prompt_fixture(event_socket: &Path) -> ExternalProviderLaunch {
    let script = format!(
        r#"
import json,socket,sys
request=json.loads(sys.stdin.readline())
print(json.dumps({{'jsonrpc':'2.0','id':request['id'],'result':{{'protocolVersion':1,'agentCapabilities':{{}},'agentInfo':{{'name':'cursor-fixture','version':'1'}}}}}})); sys.stdout.flush()
request=json.loads(sys.stdin.readline())
assert request['method']=='session/new'
print(json.dumps({{'jsonrpc':'2.0','id':request['id'],'result':{{'sessionId':'fixture-session'}}}})); sys.stdout.flush()
request=json.loads(sys.stdin.readline())
assert request['method']=='session/prompt'
with socket.socket(socket.AF_UNIX,socket.SOCK_STREAM) as event:
 event.connect({:?})
 event.sendall(b'first')
 event.recv(1)
print(json.dumps({{'jsonrpc':'2.0','id':request['id'],'result':{{'stopReason':'end_turn'}}}})); sys.stdout.flush()
request=json.loads(sys.stdin.readline())
assert request['method']=='session/prompt'
with socket.socket(socket.AF_UNIX,socket.SOCK_STREAM) as event:
 event.connect({:?})
 event.sendall(b'second')
print(json.dumps({{'jsonrpc':'2.0','id':request['id'],'result':{{'stopReason':'end_turn'}}}})); sys.stdout.flush()
sys.stdin.read()
"#,
        event_socket.display().to_string(),
        event_socket.display().to_string()
    );
    ExternalProviderLaunch {
        executable: PathBuf::from("/usr/bin/python3"),
        arguments: vec!["-c".to_owned(), script],
        environment: Vec::new(),
    }
}

fn refusing_load_fixture(load_marker: &Path) -> ExternalProviderLaunch {
    let script = format!(
        r#"
import json,sys
request=json.loads(sys.stdin.readline())
print(json.dumps({{'jsonrpc':'2.0','id':request['id'],'result':{{'protocolVersion':1,'agentCapabilities':{{'loadSession':True}},'agentInfo':{{'name':'load-fixture','version':'1'}}}}}})); sys.stdout.flush()
line=sys.stdin.readline()
if line:
 request=json.loads(line)
 assert request['method']=='session/load', request['method']
 assert request['params']['sessionId']=='fixture-session'
 open({:?},'w').write('session/load')
 print(json.dumps({{'jsonrpc':'2.0','id':request['id'],'error':{{'code':-32001,'message':'refused load'}}}})); sys.stdout.flush()
sys.stdin.read()
"#,
        load_marker.display().to_string()
    );
    ExternalProviderLaunch {
        executable: PathBuf::from("/usr/bin/python3"),
        arguments: vec!["-c".to_owned(), script],
        environment: Vec::new(),
    }
}

fn request(target: SessionRef, text: &str) -> DeliveryRequest {
    DeliveryRequest {
        target,
        message: MessageContent::HumanUser {
            text: MessageText::try_from(text.to_owned()).expect("message"),
        },
        mode: MessageDelivery::Auto,
        precondition: DeliveryPrecondition::Unpinned,
        correlation: DeliveryCorrelationId::generate(),
        attempt: AttemptId::generate(),
    }
}

#[tokio::test]
async fn cursor_steer_rejects_before_evidence_or_client_io() {
    let root = tempfile::tempdir().expect("provider root");
    let target = target();
    let store = Arc::new(tokio::sync::Mutex::new(
        ProviderOperationStore::open(&root.path().join("operations.sqlite"))
            .await
            .expect("store"),
    ));
    let supervisor = Arc::new(
        ExternalProviderSupervisor::new(Vec::new(), Arc::clone(&store)).expect("supervisor"),
    );
    let route = ProviderAcpDeliveryRoute::new(
        target.endpoint.service_id.clone(),
        EndpointDirectory::new(target.endpoint.service_id.clone()),
        supervisor,
        store,
        Arc::new(NoLivePeer),
    );
    let evidence = RecordedEvidence(tokio::sync::Mutex::new(Vec::new()));

    let receipt = route
        .deliver(
            DeliveryRequest {
                target,
                message: MessageContent::HumanUser {
                    text: MessageText::try_from("hello".to_owned()).expect("message"),
                },
                mode: MessageDelivery::Steer,
                precondition: DeliveryPrecondition::Unpinned,
                correlation: DeliveryCorrelationId::generate(),
                attempt: AttemptId::generate(),
            },
            &evidence,
        )
        .await
        .expect("delivery refusal");

    assert!(
        matches!(receipt.outcome, DeliveryOutcome::Rejected(rejection)
        if rejection.reason == DeliveryRejectionReason::SteerUnsupported)
    );
    assert!(evidence.0.lock().await.is_empty());
}

#[tokio::test]
async fn cursor_auto_queues_behind_a_running_prompt() {
    let root = tempfile::tempdir().expect("provider root");
    let event_socket = root.path().join("prompt-events.sock");
    let listener = tokio::net::UnixListener::bind(&event_socket).expect("fixture events");
    let target = target();
    let binding = provider_binding(&target);
    let runtime = ExternalProviderRuntime::initialize(cursor_prompt_fixture(&event_socket))
        .await
        .expect("fixture provider");
    runtime
        .create_session(PathBuf::from("/tmp"))
        .await
        .expect("session/new");
    let store = Arc::new(tokio::sync::Mutex::new(
        ProviderOperationStore::open(&root.path().join("operations.sqlite"))
            .await
            .expect("store"),
    ));
    store
        .lock()
        .await
        .record_session(&ProviderSessionRecord {
            target: target.clone(),
            working_directory: ProviderWorkingDirectory::try_from("/tmp".to_owned()).expect("cwd"),
            requested_policy: ProviderRequestedPolicy {
                access: RouterAccess::WriteRestricted,
            },
            created_by: target.clone(),
            approver: target.clone(),
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
    let directory = available_directory(&target, &binding);
    let route = ProviderAcpDeliveryRoute::new(
        target.endpoint.service_id.clone(),
        directory,
        Arc::clone(&supervisor),
        store,
        Arc::new(NoLivePeer),
    );
    let first_evidence = RecordedEvidence(tokio::sync::Mutex::new(Vec::new()));
    let second_evidence = RecordedEvidence(tokio::sync::Mutex::new(Vec::new()));
    let stale_evidence = RecordedEvidence(tokio::sync::Mutex::new(Vec::new()));
    let mut stale_request = request(target.clone(), "stale");
    stale_request.precondition = DeliveryPrecondition::EndpointGeneration {
        expected: CodexGeneration {
            service_epoch: binding.generation.service_epoch.clone(),
            generation: GenerationNumber::try_from(2).expect("stale generation"),
        },
    };

    let stale = route
        .deliver(stale_request, &stale_evidence)
        .await
        .expect("stale guard");
    assert!(matches!(stale.outcome, DeliveryOutcome::Rejected(rejection)
        if rejection.reason == DeliveryRejectionReason::StaleGeneration));
    assert!(stale_evidence.0.lock().await.is_empty());

    let first = route
        .deliver(request(target.clone(), "first"), &first_evidence)
        .await
        .expect("first delivery");
    let (mut first_event, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept())
        .await
        .expect("first event deadline")
        .expect("first event");
    let mut first_bytes = [0_u8; 5];
    first_event
        .read_exact(&mut first_bytes)
        .await
        .expect("first bytes");
    let second = route
        .deliver(request(target, "second"), &second_evidence)
        .await
        .expect("second delivery");

    assert!(matches!(first.outcome, DeliveryOutcome::Started));
    assert!(matches!(second.outcome, DeliveryOutcome::Queued));
    assert!(
        matches!(second_evidence.0.lock().await.as_slice(), [RouteEffectEvidence::ProviderAcp(effect)]
        if effect.submission == SubmissionEffect::RouterQueued)
    );
    first_event
        .write_all(b"x")
        .await
        .expect("release first prompt");
    let (mut second_event, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept())
        .await
        .expect("second event deadline")
        .expect("second event");
    let mut second_bytes = Vec::new();
    second_event
        .read_to_end(&mut second_bytes)
        .await
        .expect("second bytes");
    assert_eq!(first_bytes, *b"first");
    assert_eq!(second_bytes, b"second");
    route.shutdown_queue().await;
    supervisor.shutdown().await.expect("supervisor shutdown");
}

#[tokio::test]
async fn router_queued_reconciliation_uses_only_the_operation_store() {
    let root = tempfile::tempdir().expect("provider root");
    let target = target();
    let binding = provider_binding(&target);
    let store = Arc::new(tokio::sync::Mutex::new(
        ProviderOperationStore::open(&root.path().join("operations.sqlite"))
            .await
            .expect("store"),
    ));
    let supervisor = Arc::new(
        ExternalProviderSupervisor::new(Vec::new(), Arc::clone(&store)).expect("supervisor"),
    );
    let route = ProviderAcpDeliveryRoute::new(
        target.endpoint.service_id.clone(),
        EndpointDirectory::new(target.endpoint.service_id.clone()),
        supervisor,
        Arc::clone(&store),
        Arc::new(NoLivePeer),
    );
    let attempt = AttemptId::generate();
    let operation_id =
        agent_automation::OperationId::try_from(attempt.as_str().to_owned()).expect("operation ID");
    let evidence = RouteEffectEvidence::ProviderAcp(ProviderAcpEffectEvidence {
        target: target.clone(),
        generation: binding.generation.clone(),
        binding: ProviderBindingReference::try_from(String::from(binding.binding_id.clone()))
            .expect("binding reference"),
        attempt_id: attempt,
        submission: SubmissionEffect::RouterQueued,
        settlement: ProviderSettlementEffect::NotObserved,
    });
    let context = || AttemptReconciliationContext {
        target: target.clone(),
        message: MessageContent::HumanUser {
            text: MessageText::try_from("queued".to_owned()).expect("message"),
        },
        mode: MessageDelivery::Queue,
        recorded: evidence.clone(),
    };

    let absent = route
        .reconcile_attempt(context())
        .await
        .expect("absent reconciliation");
    assert!(matches!(absent, AttemptReconciliation::KnownNotSubmitted));
    store
        .lock()
        .await
        .admit(ProviderOperationAdmission {
            operation_id: operation_id.clone(),
            operation_kind: ProviderOperationKind::ConversationPrompt,
            binding: collaboration_protocol::ConversationBindingIdentity::ExternalProvider {
                binding,
            },
            admitted_at_ms: 1,
        })
        .await
        .expect("operation admission");
    store
        .lock()
        .await
        .record_target(&operation_id, &target, 2)
        .await
        .expect("target");
    store
        .lock()
        .await
        .mark_may_have_dispatched(&operation_id, 3)
        .await
        .expect("dispatch marker");
    store
        .lock()
        .await
        .record_terminal(
            &operation_id,
            ProviderOperationEffect::Applied,
            ProviderReconciliationState::Confirmed,
            4,
        )
        .await
        .expect("operation settlement");

    let present = route
        .reconcile_attempt(context())
        .await
        .expect("settled reconciliation");
    assert!(matches!(present, AttemptReconciliation::Accepted(receipt)
        if matches!(receipt.outcome, DeliveryOutcome::Queued)));
}

#[tokio::test]
async fn failed_load_returns_not_submitted_without_session_new() {
    let root = tempfile::tempdir().expect("provider root");
    let load_marker = root.path().join("load-method.txt");
    let target = target();
    let binding = provider_binding(&target);
    let runtime = ExternalProviderRuntime::initialize(refusing_load_fixture(&load_marker))
        .await
        .expect("fixture provider");
    let store = Arc::new(tokio::sync::Mutex::new(
        ProviderOperationStore::open(&root.path().join("operations.sqlite"))
            .await
            .expect("store"),
    ));
    store
        .lock()
        .await
        .record_session(&ProviderSessionRecord {
            target: target.clone(),
            working_directory: ProviderWorkingDirectory::try_from("/tmp".to_owned()).expect("cwd"),
            requested_policy: ProviderRequestedPolicy {
                access: RouterAccess::WriteRestricted,
            },
            created_by: target.clone(),
            approver: target.clone(),
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
    let route = ProviderAcpDeliveryRoute::new(
        target.endpoint.service_id.clone(),
        available_directory(&target, &binding),
        Arc::clone(&supervisor),
        store,
        Arc::new(NoLivePeer),
    );
    let evidence = RecordedEvidence(tokio::sync::Mutex::new(Vec::new()));

    let receipt = route
        .deliver(request(target, "hello"), &evidence)
        .await
        .expect("load refusal");

    assert!(matches!(
        receipt.outcome,
        DeliveryOutcome::NotSubmitted { .. }
    ));
    assert_eq!(
        std::fs::read_to_string(load_marker).expect("method"),
        "session/load"
    );
    assert!(matches!(evidence.0.lock().await.as_slice(),
        [RouteEffectEvidence::ProviderAcp(before), RouteEffectEvidence::ProviderAcp(after)]
        if before.submission == SubmissionEffect::Dispatching && after.submission == SubmissionEffect::NotDispatched));
    route.shutdown_queue().await;
    supervisor.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn live_peer_recheck_prevents_provider_load() {
    let root = tempfile::tempdir().expect("provider root");
    let load_marker = root.path().join("load-method.txt");
    let target = target();
    let binding = provider_binding(&target);
    let runtime = ExternalProviderRuntime::initialize(refusing_load_fixture(&load_marker))
        .await
        .expect("fixture provider");
    let store = Arc::new(tokio::sync::Mutex::new(
        ProviderOperationStore::open(&root.path().join("operations.sqlite"))
            .await
            .expect("store"),
    ));
    store
        .lock()
        .await
        .record_session(&ProviderSessionRecord {
            target: target.clone(),
            working_directory: ProviderWorkingDirectory::try_from("/tmp".to_owned()).expect("cwd"),
            requested_policy: ProviderRequestedPolicy {
                access: RouterAccess::WriteRestricted,
            },
            created_by: target.clone(),
            approver: target.clone(),
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
    let route = ProviderAcpDeliveryRoute::new(
        target.endpoint.service_id.clone(),
        available_directory(&target, &binding),
        Arc::clone(&supervisor),
        store,
        Arc::new(LivePeer),
    );
    let evidence = RecordedEvidence(tokio::sync::Mutex::new(Vec::new()));

    let receipt = route
        .deliver(request(target, "hello"), &evidence)
        .await
        .expect("live peer veto");

    assert!(matches!(
        receipt.outcome,
        DeliveryOutcome::NotSubmitted {
            retryable: true,
            ..
        }
    ));
    assert!(!load_marker.exists());
    assert!(matches!(evidence.0.lock().await.as_slice(),
        [RouteEffectEvidence::ProviderAcp(before), RouteEffectEvidence::ProviderAcp(after)]
        if before.submission == SubmissionEffect::Dispatching && after.submission == SubmissionEffect::NotDispatched));
    route.shutdown_queue().await;
    supervisor.shutdown().await.expect("shutdown");
}

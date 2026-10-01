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
    AttemptId, ChannelDescription, CodexGeneration, DeliveryClientReceipt, DeliveryCorrelationId,
    DeliveryOutcome, DeliveryRejectionReason, EndpointAvailability, EndpointDescription,
    EndpointId, EndpointRef, GenerationNumber, MessageContent, MessageDelivery, MessageText,
    NonEmptyText, ObservationTimestamp, OperationId, ProviderBindingId, ProviderBindingIdentity,
    ProviderCapabilities, ProviderCapability, ProviderCapabilityEvidence, ProviderCapabilityName,
    ProviderCapabilityStatus, ProviderKind, ProviderOperationEffect, ProviderOperationKind,
    ProviderReconciliationState, ProviderRequestedPolicy, ProviderRuntimeIdentity,
    ProviderTransport, ProviderWorkingDirectory, PushId, RouterAccess, SessionId, SessionRef,
    UuidIdentity,
};
use collaboration_service::{
    AttemptEvidenceSink, AttemptReconciliation, AttemptReconciliationContext, DeliveryFuture,
    DeliveryPrecondition, DeliveryRequest, EndpointDirectory, LoadPolicy, NOT_LOADED_REASON,
    ProviderOperationAdmission, ProviderOperationStore, ProviderSessionRecord, RouteClaim,
    RoutePresence, SessionDeliveryRoute, SessionDeliveryRouter, SessionMessageDelivery,
    layer_zero::{DeliveryRequest as PreparedDeliveryRequest, PreparedPush},
};
use serde_json::Value;
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

struct StaleRunningClaimRoute(Arc<ProviderAcpDeliveryRoute>);

impl SessionDeliveryRoute for StaleRunningClaimRoute {
    fn reachability(&self) -> collaboration_protocol::SessionReachability {
        collaboration_protocol::SessionReachability::ProviderAcp
    }

    fn claim(&self, _: &SessionRef) -> DeliveryFuture<'_, RouteClaim> {
        Box::pin(async { Ok(RouteClaim::Holds) })
    }

    fn presence(&self, target: &SessionRef) -> DeliveryFuture<'_, RoutePresence> {
        self.0.presence(target)
    }

    fn deliver<'a>(
        &'a self,
        request: DeliveryRequest,
        evidence: &'a dyn AttemptEvidenceSink,
    ) -> DeliveryFuture<'a, collaboration_protocol::DeliveryReceipt> {
        self.0.deliver(request, evidence)
    }

    fn deliver_prepared<'a>(
        &'a self,
        request: PreparedDeliveryRequest,
        evidence: &'a dyn AttemptEvidenceSink,
    ) -> DeliveryFuture<'a, collaboration_protocol::DeliveryReceipt> {
        self.0.deliver_prepared(request, evidence)
    }

    fn reconcile_attempt(
        &self,
        context: AttemptReconciliationContext,
    ) -> DeliveryFuture<'_, AttemptReconciliation> {
        self.0.reconcile_attempt(context)
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

fn cursor_prompt_fixture(
    event_socket: &Path,
    captured_prepared_prompt: &Path,
) -> ExternalProviderLaunch {
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
with open({:?},'w') as capture:
 json.dump(request,capture)
with socket.socket(socket.AF_UNIX,socket.SOCK_STREAM) as event:
 event.connect({:?})
 event.sendall(b'second')
print(json.dumps({{'jsonrpc':'2.0','id':request['id'],'result':{{'stopReason':'end_turn'}}}})); sys.stdout.flush()
sys.stdin.read()
"#,
        event_socket.display().to_string(),
        captured_prepared_prompt.display().to_string(),
        event_socket.display().to_string()
    );
    ExternalProviderLaunch {
        persistence_target: acp_client_runtime::ProviderPersistenceTarget::Unspecified,
        executable: PathBuf::from("/usr/bin/python3"),
        arguments: vec!["-c".to_owned(), script],
        environment: Vec::new(),
    }
}

fn refusing_load_fixture(load_marker: &Path, error_code: i32) -> ExternalProviderLaunch {
    let script = format!(
        r#"
import json,sys
request=json.loads(sys.stdin.readline())
print(json.dumps({{'jsonrpc':'2.0','id':request['id'],'result':{{'protocolVersion':1,'agentCapabilities':{{'loadSession':True}},'agentInfo':{{'name':'load-fixture','version':'1'}}}}}})); sys.stdout.flush()
for line in sys.stdin:
 request=json.loads(line)
 assert request['method']=='session/load', request['method']
 assert request['params']['sessionId']=='fixture-session'
 with open({:?},'a') as marker: marker.write('session/load\n')
 print(json.dumps({{'jsonrpc':'2.0','id':request['id'],'error':{{'code':{error_code},'message':'provider fixture refusal','data':{{'privateText':'must not escape'}}}}}})); sys.stdout.flush()
"#,
        load_marker.display().to_string(),
        error_code = error_code,
    );
    ExternalProviderLaunch {
        persistence_target: acp_client_runtime::ProviderPersistenceTarget::Unspecified,
        executable: PathBuf::from("/usr/bin/python3"),
        arguments: vec!["-c".to_owned(), script],
        environment: Vec::new(),
    }
}

fn no_load_fixture() -> ExternalProviderLaunch {
    let script = r#"
import json,sys
initialize=json.loads(sys.stdin.readline())
print(json.dumps({'jsonrpc':'2.0','id':initialize['id'],'result':{'protocolVersion':1,'agentCapabilities':{},'agentInfo':{'name':'no-load-fixture','version':'1'}}})); sys.stdout.flush()
for line in sys.stdin:
 request=json.loads(line)
 assert request['method']=='session/new', request['method']
 print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':{'sessionId':'fixture-session'}})); sys.stdout.flush()
"#;
    ExternalProviderLaunch {
        persistence_target: acp_client_runtime::ProviderPersistenceTarget::Unspecified,
        executable: PathBuf::from("/usr/bin/python3"),
        arguments: vec!["-c".to_owned(), script.to_owned()],
        environment: Vec::new(),
    }
}

fn exited_provider_fixture(exit_marker: &Path) -> ExternalProviderLaunch {
    let script = format!(
        r#"
import json,sys
request=json.loads(sys.stdin.readline())
print(json.dumps({{'jsonrpc':'2.0','id':request['id'],'result':{{'protocolVersion':1,'agentCapabilities':{{'loadSession':True}},'agentInfo':{{'name':'exited-provider-fixture','version':'1'}}}}}})); sys.stdout.flush()
with open({:?},'w') as marker: marker.write('provider exited')
sys.exit(0)
"#,
        exit_marker.display().to_string()
    );
    ExternalProviderLaunch {
        persistence_target: acp_client_runtime::ProviderPersistenceTarget::Unspecified,
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
        header_context: collaboration_protocol::MessageHeaderContext::default(),
        mode: MessageDelivery::Auto,
        load_policy: collaboration_service::LoadPolicy::MayLoad,
        precondition: DeliveryPrecondition::Unpinned,
        correlation: DeliveryCorrelationId::generate(),
        attempt: AttemptId::generate(),
    }
}

#[tokio::test]
async fn cursor_steer_rejects_before_evidence_or_client_io() {
    let root = tempfile::tempdir().expect("provider root");
    let target = target();
    let binding = provider_binding(&target);
    let runtime = ExternalProviderRuntime::initialize(no_load_fixture())
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
            created_by: (target.clone()).into(),
            approver: (target.clone()).into(),
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
        std::iter::once(target.endpoint.clone()).collect(),
        available_directory(&target, &binding),
        Arc::clone(&supervisor),
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
                header_context: collaboration_protocol::MessageHeaderContext::default(),
                mode: MessageDelivery::Steer,
                load_policy: collaboration_service::LoadPolicy::MayLoad,
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
    route.shutdown_queue().await;
    supervisor.shutdown().await.expect("supervisor shutdown");
}

#[tokio::test]
async fn cursor_auto_queues_a_prepared_push_with_exact_line_and_push_id() {
    let root = tempfile::tempdir().expect("provider root");
    let event_socket = root.path().join("prompt-events.sock");
    let captured_prepared_prompt = root.path().join("prepared-prompt.json");
    let listener = tokio::net::UnixListener::bind(&event_socket).expect("fixture events");
    let target = target();
    let binding = provider_binding(&target);
    let runtime = ExternalProviderRuntime::initialize(cursor_prompt_fixture(
        &event_socket,
        &captured_prepared_prompt,
    ))
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
            created_by: (target.clone()).into(),
            approver: (target.clone()).into(),
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
    let route = Arc::new(ProviderAcpDeliveryRoute::new(
        target.endpoint.service_id.clone(),
        std::iter::once(target.endpoint.clone()).collect(),
        directory,
        Arc::clone(&supervisor),
        store,
        Arc::new(NoLivePeer),
    ));
    let router = SessionDeliveryRouter::new(vec![route.clone()]);
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

    let stale = router
        .deliver(stale_request, &stale_evidence)
        .await
        .expect("stale guard");
    assert!(matches!(stale.outcome, DeliveryOutcome::Rejected(rejection)
        if rejection.reason == DeliveryRejectionReason::StaleGeneration));
    assert!(stale_evidence.0.lock().await.is_empty());

    let first = router
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
    let push_id = PushId::try_from("018f47d2-24d5-7a68-b9ec-6f759c394599".to_owned())
        .expect("UUIDv7 push id");
    let line = MessageText::try_from(format!(
        "✉️ Main · \"prepared while busy\" · router://00000000-0000-4000-8000-000000000001/push/{}",
        push_id.as_str()
    ))
    .expect("push line");
    let correlation =
        DeliveryCorrelationId::try_from(push_id.as_str().to_owned()).expect("push correlation");
    assert_eq!(correlation.as_str(), push_id.as_str());
    let attempt = AttemptId::generate();
    let second = router
        .deliver_prepared(
            PreparedDeliveryRequest {
                payload: PreparedPush {
                    push_id: push_id.clone(),
                    line: line.clone(),
                    load_policy: LoadPolicy::LoadedOnly,
                },
                target: target.clone(),
                mode: MessageDelivery::Auto,
                precondition: DeliveryPrecondition::Unpinned,
                correlation,
                attempt: attempt.clone(),
            },
            &second_evidence,
        )
        .await
        .expect("second delivery");

    assert!(matches!(first.outcome, DeliveryOutcome::Started));
    assert!(matches!(second.outcome, DeliveryOutcome::Queued));
    assert_eq!(
        second.reachability,
        Some(collaboration_protocol::SessionReachability::ProviderAcp)
    );
    assert!(matches!(
        second.client,
        Some(DeliveryClientReceipt::ProviderAcp { operation_id })
            if operation_id == OperationId::try_from(attempt.as_str().to_owned()).expect("attempt operation id")
    ));
    assert!(
        matches!(second_evidence.0.lock().await.as_slice(), [RouteEffectEvidence::ProviderAcp(effect)]
        if effect.submission == SubmissionEffect::RouterQueued)
    );
    let queued = route.queue_list(&target);
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].input_id.as_str(), push_id.as_str());
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
    let captured_prompt: Value = serde_json::from_slice(
        &std::fs::read(&captured_prepared_prompt).expect("captured prepared prompt"),
    )
    .expect("prepared prompt JSON");
    assert_eq!(
        captured_prompt["params"]["sessionId"].as_str(),
        Some("fixture-session")
    );
    let prompt_blocks = captured_prompt["params"]["prompt"]
        .as_array()
        .expect("ACP prompt content blocks");
    assert_eq!(prompt_blocks.len(), 1);
    assert_eq!(prompt_blocks[0]["text"].as_str(), Some(line.as_str()));
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
        std::iter::once(target.endpoint.clone()).collect(),
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
        prepared_push_id: None,
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
            None,
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
async fn provider_load_auth_rejection_is_typed_without_session_new() {
    let root = tempfile::tempdir().expect("provider root");
    let load_marker = root.path().join("load-method.txt");
    let target = target();
    let binding = provider_binding(&target);
    let runtime = ExternalProviderRuntime::initialize(refusing_load_fixture(&load_marker, -32000))
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
            created_by: (target.clone()).into(),
            approver: (target.clone()).into(),
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
    let route = Arc::new(ProviderAcpDeliveryRoute::new(
        target.endpoint.service_id.clone(),
        std::iter::once(target.endpoint.clone()).collect(),
        available_directory(&target, &binding),
        Arc::clone(&supervisor),
        store,
        Arc::new(NoLivePeer),
    ));
    assert_eq!(
        route.presence(&target).await.expect("provider presence"),
        RoutePresence::Wakeable
    );

    let stale_router =
        SessionDeliveryRouter::new(vec![Arc::new(StaleRunningClaimRoute(Arc::clone(&route)))]);
    let loaded_only_push_id = PushId::try_from("018f47d2-24d5-7a68-b9ec-6f759c394588".to_owned())
        .expect("UUIDv7 loaded-only push id");
    let loaded_only_line = MessageText::try_from(format!(
        "🧵 Main · \"held batch\" · router://00000000-0000-4000-8000-000000000001/push/{}",
        loaded_only_push_id.as_str()
    ))
    .expect("loaded-only push line");
    let loaded_only_correlation =
        DeliveryCorrelationId::try_from(loaded_only_push_id.as_str().to_owned())
            .expect("loaded-only push correlation");
    assert_eq!(
        loaded_only_correlation.as_str(),
        loaded_only_push_id.as_str()
    );
    let loaded_only_request = PreparedDeliveryRequest {
        payload: PreparedPush {
            push_id: loaded_only_push_id.clone(),
            line: loaded_only_line,
            load_policy: LoadPolicy::LoadedOnly,
        },
        target: target.clone(),
        mode: MessageDelivery::Auto,
        precondition: DeliveryPrecondition::Unpinned,
        correlation: loaded_only_correlation,
        attempt: AttemptId::generate(),
    };
    let loaded_only_evidence = RecordedEvidence(tokio::sync::Mutex::new(Vec::new()));
    let loaded_only_receipt = stale_router
        .deliver_prepared(loaded_only_request, &loaded_only_evidence)
        .await
        .expect("loaded-only stale-claim refusal");
    assert!(matches!(
        loaded_only_receipt.outcome,
        DeliveryOutcome::NotSubmitted {
            retryable: true,
            ref reason,
        } if reason == NOT_LOADED_REASON
    ));
    assert!(!load_marker.exists());
    assert!(matches!(
        loaded_only_evidence.0.lock().await.as_slice(),
        [RouteEffectEvidence::ProviderAcp(before), RouteEffectEvidence::ProviderAcp(after)]
            if before.submission == SubmissionEffect::Dispatching
                && after.submission == SubmissionEffect::NotDispatched
    ));

    let evidence = RecordedEvidence(tokio::sync::Mutex::new(Vec::new()));

    let receipt = route
        .deliver(request(target.clone(), "hello"), &evidence)
        .await
        .expect("load refusal");

    let outcome = serde_json::to_value(&receipt.outcome).expect("receipt encoding");
    assert_eq!(outcome["kind"], "rejected");
    assert_eq!(outcome["reason"], "providerRejected");
    assert_eq!(outcome["clientCode"], -32000);
    assert!(!outcome.to_string().contains("must not escape"));
    assert_eq!(
        std::fs::read_to_string(load_marker).expect("method").trim(),
        "session/load"
    );
    assert!(matches!(evidence.0.lock().await.as_slice(),
        [RouteEffectEvidence::ProviderAcp(before), RouteEffectEvidence::ProviderAcp(after)]
        if before.submission == SubmissionEffect::Dispatching && after.submission == SubmissionEffect::NotDispatched));
    route.shutdown_queue().await;
    supervisor.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn unadvertised_load_settles_not_submitted_without_sending_load() {
    // ACP v1 initialization.mdx:245 requires an advertisement for session/load.
    // R9 settles a cold delivery as unsupported: load without sending it.
    let root = tempfile::tempdir().expect("provider root");
    let target = target();
    let binding = provider_binding(&target);
    let runtime = ExternalProviderRuntime::initialize(no_load_fixture())
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
            created_by: (target.clone()).into(),
            approver: (target.clone()).into(),
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
        std::iter::once(target.endpoint.clone()).collect(),
        available_directory(&target, &binding),
        Arc::clone(&supervisor),
        store,
        Arc::new(NoLivePeer),
    );
    assert!(matches!(
        route.presence(&target).await.expect("provider presence"),
        RoutePresence::Unreachable { .. }
    ));
    let evidence = RecordedEvidence(tokio::sync::Mutex::new(Vec::new()));
    let receipt = tokio::time::timeout(
        Duration::from_secs(2),
        route.deliver(request(target, "hello"), &evidence),
    )
    .await
    .expect("delivery cannot wait on unsupported load")
    .expect("delivery receipt");
    assert!(
        matches!(
            receipt.outcome,
            DeliveryOutcome::NotSubmitted { retryable: false, ref reason }
                if reason == "unsupported: load"
        ),
        "{receipt:?}"
    );
    route.shutdown_queue().await;
    supervisor.shutdown().await.expect("supervisor shutdown");
}

#[tokio::test]
async fn coded_provider_load_errors_keep_correlations_and_do_not_expose_acp_text() {
    for (error_code, expected_kind, expected_reason, expected_explanation, code_label) in [
        (
            -32002,
            "rejected",
            Some("providerSessionNotFound"),
            "this session never started a turn and did not survive the provider restart; create a new conversation",
            "provider code",
        ),
        (
            -32600,
            "rejected",
            Some("providerRejected"),
            "provider rejected the ACP operation",
            "provider code",
        ),
        (
            -32000,
            "rejected",
            Some("providerRejected"),
            "provider rejected the ACP operation",
            "provider code",
        ),
        (
            -32601,
            "notSubmitted",
            None,
            "provider ACP method is unsupported",
            "ACP code",
        ),
        (
            -32602,
            "notSubmitted",
            None,
            "provider ACP parameters are invalid",
            "ACP code",
        ),
        (
            -32800,
            "notSubmitted",
            None,
            "provider ACP request was cancelled",
            "ACP code",
        ),
    ] {
        let root = tempfile::tempdir().expect("provider root");
        let load_marker = root.path().join("load-method.txt");
        let target = target();
        let binding = provider_binding(&target);
        let runtime =
            ExternalProviderRuntime::initialize(refusing_load_fixture(&load_marker, error_code))
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
                working_directory: ProviderWorkingDirectory::try_from("/tmp".to_owned())
                    .expect("cwd"),
                requested_policy: ProviderRequestedPolicy {
                    access: RouterAccess::WriteRestricted,
                },
                created_by: (target.clone()).into(),
                approver: (target.clone()).into(),
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
            std::iter::once(target.endpoint.clone()).collect(),
            available_directory(&target, &binding),
            Arc::clone(&supervisor),
            store,
            Arc::new(NoLivePeer),
        );
        let evidence = RecordedEvidence(tokio::sync::Mutex::new(Vec::new()));

        let receipt = route
            .deliver(request(target, "empty restart session"), &evidence)
            .await
            .expect("load refusal");
        let outcome = serde_json::to_value(&receipt.outcome).expect("receipt encoding");

        assert_eq!(outcome["kind"], expected_kind);
        if let Some(expected_reason) = expected_reason {
            assert_eq!(outcome["reason"], expected_reason);
            assert_eq!(outcome["clientCode"], error_code);
            let detail = outcome["detail"].as_str().expect("rejection detail");
            assert!(detail.starts_with(expected_explanation), "{detail}");
            assert!(
                !detail.contains("provider fixture refusal") && !detail.contains("must not escape"),
                "ACP error data escaped: {outcome}"
            );
            assert_uuid_v7_provider_reference(detail, code_label, error_code);
        } else {
            assert_eq!(outcome["retryable"], true);
            let reason = outcome["reason"].as_str().expect("not-submitted reason");
            assert!(reason.starts_with(expected_explanation), "{reason}");
            assert!(
                !reason.contains("provider fixture refusal") && !reason.contains("must not escape"),
                "ACP error data escaped: {outcome}"
            );
            assert_uuid_v7_provider_reference(reason, code_label, error_code);
        }
        assert_eq!(
            std::fs::read_to_string(&load_marker)
                .expect("load marker")
                .trim(),
            "session/load"
        );
        assert!(matches!(evidence.0.lock().await.as_slice(),
            [RouteEffectEvidence::ProviderAcp(before), RouteEffectEvidence::ProviderAcp(after)]
            if before.submission == SubmissionEffect::Dispatching && after.submission == SubmissionEffect::NotDispatched));
        route.shutdown_queue().await;
        supervisor.shutdown().await.expect("shutdown");
    }
}

fn assert_uuid_v7_provider_reference(detail: &str, code_label: &str, provider_code: i32) {
    assert!(
        detail.contains(&format!("{code_label} {provider_code}; reference ")),
        "provider code/reference missing: {detail}"
    );
    let correlation_reference = detail
        .split_once("reference ")
        .map(|(_, reference)| reference.trim_end_matches(')'))
        .expect("provider correlation reference");
    let correlation_uuid =
        uuid::Uuid::parse_str(correlation_reference).expect("UUIDv7 correlation reference");
    assert_eq!(correlation_uuid.get_version_num(), 7, "{detail}");
}

#[tokio::test]
async fn provider_process_transport_failure_remains_retryable() {
    let root = tempfile::tempdir().expect("provider root");
    let exit_marker = root.path().join("provider-exit.txt");
    let target = target();
    let binding = provider_binding(&target);
    let runtime = ExternalProviderRuntime::initialize(exited_provider_fixture(&exit_marker))
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
            created_by: (target.clone()).into(),
            approver: (target.clone()).into(),
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
    tokio::time::timeout(Duration::from_secs(2), async {
        while !exit_marker.exists() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("provider exit marker deadline");
    let route = ProviderAcpDeliveryRoute::new(
        target.endpoint.service_id.clone(),
        std::iter::once(target.endpoint.clone()).collect(),
        available_directory(&target, &binding),
        Arc::clone(&supervisor),
        store,
        Arc::new(NoLivePeer),
    );

    let receipt = route
        .deliver(
            request(target, "retry a transient provider disconnect"),
            &RecordedEvidence(tokio::sync::Mutex::new(Vec::new())),
        )
        .await
        .expect("transport failure receipt");

    assert!(
        matches!(
            &receipt.outcome,
            DeliveryOutcome::NotSubmitted {
                retryable: true,
                ..
            }
        ),
        "transport refusal was not retryable: {:?}",
        receipt.outcome
    );
    assert_eq!(
        std::fs::read_to_string(exit_marker)
            .expect("provider exit marker")
            .trim(),
        "provider exited"
    );
    route.shutdown_queue().await;
    supervisor.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn live_peer_recheck_prevents_provider_load() {
    let root = tempfile::tempdir().expect("provider root");
    let load_marker = root.path().join("load-method.txt");
    let target = target();
    let binding = provider_binding(&target);
    let runtime = ExternalProviderRuntime::initialize(refusing_load_fixture(&load_marker, -32002))
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
            created_by: (target.clone()).into(),
            approver: (target.clone()).into(),
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
        std::iter::once(target.endpoint.clone()).collect(),
        available_directory(&target, &binding),
        Arc::clone(&supervisor),
        store,
        Arc::new(LivePeer),
    );
    assert!(matches!(
        route.presence(&target).await.expect("provider presence"),
        RoutePresence::LiveElsewhere { .. }
    ));
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

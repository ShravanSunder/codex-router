#![allow(clippy::expect_used)]
//! A running Claude prompt identifies the operation receiving a steered message.
use agent_automation::{RouteEffectEvidence, SubmissionEffect};
use codex_router_host::{
    ExternalProviderBinding, ExternalProviderLaunch, ExternalProviderRuntime,
    ExternalProviderSupervisor, LiveSessionOwnership, LiveSessionOwnershipCheck,
    ProviderAcpDeliveryRoute,
};
use collaboration_protocol::{
    AttemptId, ChannelDescription, CodexGeneration, ConversationPromptRequest,
    DeliveryClientReceipt, DeliveryCorrelationId, DeliveryOutcome, EndpointAvailability,
    EndpointDescription, EndpointId, EndpointRef, GenerationNumber, MessageContent,
    MessageDelivery, MessageText, NonEmptyText, ObservationTimestamp, OperationId,
    ProviderBindingId, ProviderBindingIdentity, ProviderCapabilities, ProviderCapability,
    ProviderCapabilityEvidence, ProviderCapabilityName, ProviderCapabilityStatus, ProviderKind,
    ProviderRequestedPolicy, ProviderRuntimeIdentity, ProviderTransport, ProviderWorkingDirectory,
    RouterAccess, SessionId, SessionRef, UuidIdentity,
};
use collaboration_service::{
    AttemptEvidenceSink, AttemptReconciliation, AttemptReconciliationContext, DeliveryFuture,
    DeliveryPrecondition, DeliveryRequest, EndpointDirectory, ProviderConversationBackend,
    ProviderOperationStore, ProviderSessionRecord, SessionDeliveryRoute,
};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

struct NoLivePeer;

impl LiveSessionOwnershipCheck for NoLivePeer {
    fn check<'a>(&'a self, _target: &'a SessionRef) -> DeliveryFuture<'a, LiveSessionOwnership> {
        Box::pin(async { Ok(LiveSessionOwnership::NotLive) })
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
                .expect("service"),
            endpoint_id: EndpointId::try_from("claude-local".to_owned()).expect("endpoint"),
        },
        session_id: SessionId::try_from("fixture-session".to_owned()).expect("session"),
    }
}

fn binding(target: &SessionRef) -> ProviderBindingIdentity {
    ProviderBindingIdentity {
        endpoint: target.endpoint.clone(),
        binding_id: ProviderBindingId::try_from("claude-fixture-binding".to_owned())
            .expect("binding"),
        runtime: ProviderRuntimeIdentity {
            provider: ProviderKind::ClaudeCode,
            runtime_name: NonEmptyText::try_from("claude-fixture".to_owned()).expect("runtime"),
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

fn steering_fixture(event_socket: &Path) -> ExternalProviderLaunch {
    let script = format!(
        r#"
import json,socket,sys
request=json.loads(sys.stdin.readline())
print(json.dumps({{'jsonrpc':'2.0','id':request['id'],'result':{{'protocolVersion':1,'agentCapabilities':{{'loadSession':True}},'agentInfo':{{'name':'claude-fixture','version':'1'}},'_meta':{{'steering':{{'supported':True}}}}}}}})); sys.stdout.flush()
request=json.loads(sys.stdin.readline())
assert request['method']=='session/new'
print(json.dumps({{'jsonrpc':'2.0','id':request['id'],'result':{{'sessionId':'fixture-session'}}}})); sys.stdout.flush()
prompt=json.loads(sys.stdin.readline())
assert prompt['method']=='session/prompt'
with socket.socket(socket.AF_UNIX,socket.SOCK_STREAM) as event:
 event.connect({:?})
 event.sendall(b'active')
 for _ in range(2):
  steer=json.loads(sys.stdin.readline())
  assert steer['method']=='_session/steering'
  assert steer['params']['prompt'][0]['text'].endswith('follow-up')
  print(json.dumps({{'jsonrpc':'2.0','id':steer['id'],'result':{{'outcome':'injected'}}}})); sys.stdout.flush()
 event.recv(1)
print(json.dumps({{'jsonrpc':'2.0','id':prompt['id'],'result':{{'stopReason':'end_turn'}}}})); sys.stdout.flush()
sys.stdin.read()
"#,
        event_socket.display().to_string()
    );
    ExternalProviderLaunch {
        executable: PathBuf::from("/usr/bin/python3"),
        arguments: vec!["-c".to_owned(), script],
        environment: Vec::new(),
    }
}

async fn prepared_route(
    root: &Path,
    launch: ExternalProviderLaunch,
) -> (
    ProviderAcpDeliveryRoute,
    Arc<ExternalProviderSupervisor>,
    SessionRef,
    CodexGeneration,
) {
    let target = target();
    let binding = binding(&target);
    let generation = binding.generation.clone();
    let runtime = ExternalProviderRuntime::initialize(launch)
        .await
        .expect("fixture provider");
    runtime
        .create_session(PathBuf::from("/tmp"))
        .await
        .expect("session/new");
    let store = Arc::new(tokio::sync::Mutex::new(
        ProviderOperationStore::open(&root.join("operations.sqlite"))
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
    let directory = EndpointDirectory::new(target.endpoint.service_id.clone());
    directory
        .publish(EndpointDescription {
            endpoint: target.endpoint.clone(),
            label: NonEmptyText::try_from("Claude Code".to_owned()).expect("label"),
            availability: EndpointAvailability::Available {
                observed_at: ObservationTimestamp::try_from("2026-09-24T12:00:00Z".to_owned())
                    .expect("time"),
            },
            channels: vec![ChannelDescription::ExternalProvider {
                transport: ProviderTransport::StdioAcp,
                binding_id: binding.binding_id,
                binding_generation: binding.generation.generation,
                runtime: binding.runtime,
                capabilities: binding.capabilities,
            }],
        })
        .expect("endpoint");
    let route = ProviderAcpDeliveryRoute::new(
        target.endpoint.service_id.clone(),
        directory,
        Arc::clone(&supervisor),
        store,
        Arc::new(NoLivePeer),
    );
    (route, supervisor, target, generation)
}

#[derive(Clone, Copy)]
enum IdleSteeringScenario {
    SteerOnly,
    AutoStartsPrompt,
    DropDuringSteer,
}

fn idle_steering_fixture(scenario: IdleSteeringScenario) -> ExternalProviderLaunch {
    let tail = match scenario {
        IdleSteeringScenario::SteerOnly => "sys.stdin.read()",
        IdleSteeringScenario::AutoStartsPrompt => {
            r#"
prompt=json.loads(sys.stdin.readline())
assert prompt['method']=='session/prompt'
print(json.dumps({'jsonrpc':'2.0','id':prompt['id'],'result':{'stopReason':'end_turn'}})); sys.stdout.flush()
sys.stdin.read()
"#
        }
        IdleSteeringScenario::DropDuringSteer => "sys.exit(0)",
    };
    let steer_reply = match scenario {
        IdleSteeringScenario::DropDuringSteer => "",
        IdleSteeringScenario::SteerOnly | IdleSteeringScenario::AutoStartsPrompt => {
            "print(json.dumps({'jsonrpc':'2.0','id':steer['id'],'result':{'outcome':'promptRequired'}})); sys.stdout.flush()"
        }
    };
    let script = format!(
        r#"
import json,sys
request=json.loads(sys.stdin.readline())
print(json.dumps({{'jsonrpc':'2.0','id':request['id'],'result':{{'protocolVersion':1,'agentCapabilities':{{'loadSession':True}},'agentInfo':{{'name':'claude-fixture','version':'1'}},'_meta':{{'steering':{{'supported':True}}}}}}}})); sys.stdout.flush()
request=json.loads(sys.stdin.readline())
assert request['method']=='session/new'
print(json.dumps({{'jsonrpc':'2.0','id':request['id'],'result':{{'sessionId':'fixture-session'}}}})); sys.stdout.flush()
steer=json.loads(sys.stdin.readline())
assert steer['method']=='_session/steering'
{steer_reply}
{tail}
"#
    );
    ExternalProviderLaunch {
        executable: PathBuf::from("/usr/bin/python3"),
        arguments: vec!["-c".to_owned(), script],
        environment: Vec::new(),
    }
}

fn request(target: SessionRef, mode: MessageDelivery) -> DeliveryRequest {
    DeliveryRequest {
        target,
        message: MessageContent::HumanUser {
            text: MessageText::try_from("follow-up".to_owned()).expect("message"),
        },
        mode,
        precondition: DeliveryPrecondition::Unpinned,
        correlation: DeliveryCorrelationId::generate(),
        attempt: AttemptId::generate(),
    }
}

#[tokio::test]
async fn running_claude_auto_and_steer_name_the_running_operation() {
    let root = tempfile::tempdir().expect("fixture root");
    let event_socket = root.path().join("prompt-event.sock");
    let listener = tokio::net::UnixListener::bind(&event_socket).expect("event listener");
    let (route, supervisor, target, generation) =
        prepared_route(root.path(), steering_fixture(&event_socket)).await;
    let running_operation = OperationId::generate();
    supervisor
        .prompt(ConversationPromptRequest {
            operation_id: running_operation.clone(),
            target: target.clone(),
            generation,
            requested_by: target.clone(),
            approver: target.clone(),
            prompt: MessageContent::Router {
                text: MessageText::try_from("hold".to_owned()).expect("prompt"),
            },
        })
        .await
        .expect("running prompt submitted");
    let (mut active_event, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept())
        .await
        .expect("active event deadline")
        .expect("active event");
    let mut marker = [0_u8; 6];
    active_event
        .read_exact(&mut marker)
        .await
        .expect("active marker");
    for mode in [MessageDelivery::Auto, MessageDelivery::Steer] {
        let evidence = RecordedEvidence(tokio::sync::Mutex::new(Vec::new()));
        let receipt = route
            .deliver(
                DeliveryRequest {
                    target: target.clone(),
                    message: MessageContent::HumanUser {
                        text: MessageText::try_from("follow-up".to_owned()).expect("message"),
                    },
                    mode,
                    precondition: DeliveryPrecondition::Unpinned,
                    correlation: DeliveryCorrelationId::generate(),
                    attempt: AttemptId::generate(),
                },
                &evidence,
            )
            .await
            .expect("steer delivery");

        assert!(matches!(receipt.outcome, DeliveryOutcome::Steered));
        assert!(matches!(receipt.client,
            Some(DeliveryClientReceipt::ProviderAcp { operation_id }) if operation_id == running_operation));
        assert!(matches!(evidence.0.lock().await.as_slice(),
            [RouteEffectEvidence::ProviderAcp(before), RouteEffectEvidence::ProviderAcp(after)]
            if before.submission == SubmissionEffect::Dispatching && after.submission == SubmissionEffect::Accepted));
    }
    active_event.write_all(b"x").await.expect("release prompt");
    route.shutdown_queue().await;
    supervisor.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn idle_claude_steer_is_known_not_submitted() {
    let root = tempfile::tempdir().expect("fixture root");
    let (route, supervisor, target, _) = prepared_route(
        root.path(),
        idle_steering_fixture(IdleSteeringScenario::SteerOnly),
    )
    .await;
    let evidence = RecordedEvidence(tokio::sync::Mutex::new(Vec::new()));

    let sent = request(target, MessageDelivery::Steer);
    let receipt = route
        .deliver(sent.clone(), &evidence)
        .await
        .expect("idle steer");

    assert!(
        matches!(receipt.outcome, DeliveryOutcome::NotSubmitted { retryable: false, reason }
        if reason == "no running turn")
    );
    assert!(matches!(evidence.0.lock().await.as_slice(),
        [RouteEffectEvidence::ProviderAcp(before), RouteEffectEvidence::ProviderAcp(after)]
        if before.submission == SubmissionEffect::Dispatching && after.submission == SubmissionEffect::NotDispatched));
    route.shutdown_queue().await;
    supervisor.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn idle_claude_auto_starts_after_prompt_required() {
    let root = tempfile::tempdir().expect("fixture root");
    let (route, supervisor, target, _) = prepared_route(
        root.path(),
        idle_steering_fixture(IdleSteeringScenario::AutoStartsPrompt),
    )
    .await;
    let evidence = RecordedEvidence(tokio::sync::Mutex::new(Vec::new()));

    let receipt = route
        .deliver(request(target, MessageDelivery::Auto), &evidence)
        .await
        .expect("idle auto");

    assert!(matches!(receipt.outcome, DeliveryOutcome::Started));
    assert!(matches!(
        receipt.client,
        Some(DeliveryClientReceipt::ProviderAcp { .. })
    ));
    assert!(matches!(evidence.0.lock().await.as_slice(),
        [RouteEffectEvidence::ProviderAcp(before), RouteEffectEvidence::ProviderAcp(after)]
        if before.submission == SubmissionEffect::Dispatching && after.submission == SubmissionEffect::Accepted));
    route.shutdown_queue().await;
    supervisor.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn dropped_steering_reply_remains_unknown() {
    let root = tempfile::tempdir().expect("fixture root");
    let (route, supervisor, target, _) = prepared_route(
        root.path(),
        idle_steering_fixture(IdleSteeringScenario::DropDuringSteer),
    )
    .await;
    let evidence = RecordedEvidence(tokio::sync::Mutex::new(Vec::new()));

    let sent = request(target, MessageDelivery::Steer);
    let receipt = route
        .deliver(sent.clone(), &evidence)
        .await
        .expect("dropped steering reply");

    assert!(matches!(receipt.outcome, DeliveryOutcome::Unknown));
    assert!(matches!(evidence.0.lock().await.as_slice(),
        [RouteEffectEvidence::ProviderAcp(before), RouteEffectEvidence::ProviderAcp(after)]
        if before.submission == SubmissionEffect::Dispatching && after.submission == SubmissionEffect::Unknown));
    let recorded = evidence
        .0
        .lock()
        .await
        .last()
        .expect("unknown evidence")
        .clone();
    let reconciliation = route
        .reconcile_attempt(AttemptReconciliationContext {
            target: sent.target,
            message: sent.message,
            mode: sent.mode,
            recorded,
        })
        .await
        .expect("reconciliation");
    assert!(matches!(
        reconciliation,
        AttemptReconciliation::StillUnknown
    ));
    route.shutdown_queue().await;
    let _shutdown = supervisor.shutdown().await;
}

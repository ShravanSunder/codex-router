#![allow(clippy::expect_used)]
//! Explicit queue mode is shared by both ACP providers.
use agent_automation::{RouteEffectEvidence, SubmissionEffect};
use codex_router_host::{
    ExternalProviderBinding, ExternalProviderLaunch, ExternalProviderRuntime,
    ExternalProviderSupervisor, LiveSessionOwnership, LiveSessionOwnershipCheck,
    ProviderAcpDeliveryRoute,
};
use collaboration_protocol::{
    AttemptId, ChannelDescription, CodexGeneration, DeliveryClientReceipt, DeliveryCorrelationId,
    DeliveryOutcome, EndpointAvailability, EndpointDescription, EndpointId, EndpointRef,
    GenerationNumber, MessageContent, MessageDelivery, MessageText, NonEmptyText,
    ObservationTimestamp, ProviderBindingId, ProviderBindingIdentity, ProviderCapabilities,
    ProviderCapability, ProviderCapabilityEvidence, ProviderCapabilityName,
    ProviderCapabilityStatus, ProviderKind, ProviderRequestedPolicy, ProviderRuntimeIdentity,
    ProviderTransport, ProviderWorkingDirectory, RouterAccess, SessionId, SessionRef, UuidIdentity,
};
use collaboration_service::{
    AttemptEvidenceSink, DeliveryFuture, DeliveryPrecondition, DeliveryRequest, EndpointDirectory,
    ProviderOperationStore, ProviderSessionRecord, SessionDeliveryRoute,
};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::io::AsyncReadExt as _;

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

fn one_prompt_fixture(event_socket: &Path) -> ExternalProviderLaunch {
    let script = format!(
        r#"
import json,socket,sys
request=json.loads(sys.stdin.readline())
print(json.dumps({{'jsonrpc':'2.0','id':request['id'],'result':{{'protocolVersion':1,'agentCapabilities':{{'loadSession':True}},'agentInfo':{{'name':'queue-fixture','version':'1'}},'_meta':{{'steering':{{'supported':True}}}}}}}})); sys.stdout.flush()
request=json.loads(sys.stdin.readline())
assert request['method']=='session/new'
print(json.dumps({{'jsonrpc':'2.0','id':request['id'],'result':{{'sessionId':'fixture-session'}}}})); sys.stdout.flush()
request=json.loads(sys.stdin.readline())
assert request['method']=='session/prompt'
with socket.socket(socket.AF_UNIX,socket.SOCK_STREAM) as event:
 event.connect({:?})
 event.sendall(b'queued-prompt')
print(json.dumps({{'jsonrpc':'2.0','id':request['id'],'result':{{'stopReason':'end_turn'}}}})); sys.stdout.flush()
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

#[tokio::test]
async fn idle_explicit_queue_is_accepted_and_drains_for_both_providers() {
    for (endpoint_id, provider) in [
        ("claude-local", ProviderKind::ClaudeCode),
        ("cursor-local", ProviderKind::Cursor),
    ] {
        let root = tempfile::tempdir().expect("fixture root");
        let event_socket = root.path().join("queue-event.sock");
        let listener = tokio::net::UnixListener::bind(&event_socket).expect("event listener");
        let service_id = UuidIdentity::try_from("0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89".to_owned())
            .expect("service");
        let target = SessionRef {
            endpoint: EndpointRef {
                service_id: service_id.clone(),
                endpoint_id: EndpointId::try_from(endpoint_id.to_owned()).expect("endpoint"),
            },
            session_id: SessionId::try_from("fixture-session".to_owned()).expect("session"),
        };
        let generation = CodexGeneration {
            service_epoch: UuidIdentity::try_from(
                "1ff962c5-7fa3-4c18-a5ca-1bbe8db09e80".to_owned(),
            )
            .expect("epoch"),
            generation: GenerationNumber::try_from(1).expect("generation"),
        };
        let runtime_identity = ProviderRuntimeIdentity {
            provider,
            runtime_name: NonEmptyText::try_from("queue-fixture".to_owned()).expect("runtime"),
            runtime_version: None,
        };
        let capabilities = ProviderCapabilities::try_from(vec![ProviderCapability {
            name: ProviderCapabilityName::Prompt,
            status: ProviderCapabilityStatus::Supported,
            evidence: ProviderCapabilityEvidence::Advertised,
        }])
        .expect("capabilities");
        let binding_id =
            ProviderBindingId::try_from("queue-fixture-binding".to_owned()).expect("binding");
        let binding = ProviderBindingIdentity {
            endpoint: target.endpoint.clone(),
            binding_id: binding_id.clone(),
            runtime: runtime_identity.clone(),
            transport: ProviderTransport::StdioAcp,
            generation: generation.clone(),
            capabilities: capabilities.clone(),
        };
        let runtime = ExternalProviderRuntime::initialize(one_prompt_fixture(&event_socket))
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
                working_directory: ProviderWorkingDirectory::try_from("/tmp".to_owned())
                    .expect("cwd"),
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
                    identity: binding,
                    runtime,
                }],
                Arc::clone(&store),
            )
            .expect("supervisor"),
        );
        let directory = EndpointDirectory::new(service_id.clone());
        directory
            .publish(EndpointDescription {
                endpoint: target.endpoint.clone(),
                label: NonEmptyText::try_from("Provider".to_owned()).expect("label"),
                availability: EndpointAvailability::Available {
                    observed_at: ObservationTimestamp::try_from("2026-09-24T12:00:00Z".to_owned())
                        .expect("time"),
                },
                channels: vec![ChannelDescription::ExternalProvider {
                    transport: ProviderTransport::StdioAcp,
                    binding_id,
                    binding_generation: generation.generation,
                    runtime: runtime_identity,
                    capabilities,
                }],
            })
            .expect("endpoint");
        let route = ProviderAcpDeliveryRoute::new(
            service_id,
            directory,
            Arc::clone(&supervisor),
            store,
            Arc::new(NoLivePeer),
        );
        let evidence = RecordedEvidence(tokio::sync::Mutex::new(Vec::new()));
        let attempt = AttemptId::generate();

        let receipt = route
            .deliver(
                DeliveryRequest {
                    target,
                    message: MessageContent::HumanUser {
                        text: MessageText::try_from("queued work".to_owned()).expect("message"),
                    },
                    mode: MessageDelivery::Queue,
                    precondition: DeliveryPrecondition::Unpinned,
                    correlation: DeliveryCorrelationId::generate(),
                    attempt: attempt.clone(),
                },
                &evidence,
            )
            .await
            .expect("queue delivery");

        assert!(
            matches!(receipt.outcome, DeliveryOutcome::Queued),
            "{endpoint_id}"
        );
        assert!(
            matches!(receipt.client,
            Some(DeliveryClientReceipt::ProviderAcp { operation_id })
            if operation_id.as_str() == attempt.as_str()),
            "{endpoint_id}"
        );
        assert!(
            matches!(evidence.0.lock().await.as_slice(), [RouteEffectEvidence::ProviderAcp(effect)]
            if effect.submission == SubmissionEffect::RouterQueued),
            "{endpoint_id}"
        );
        let (mut stream, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept())
            .await
            .expect("queued prompt deadline")
            .expect("queued prompt event");
        let mut marker = Vec::new();
        stream.read_to_end(&mut marker).await.expect("event bytes");
        assert_eq!(marker, b"queued-prompt", "{endpoint_id}");
        route.shutdown_queue().await;
        supervisor.shutdown().await.expect("shutdown");
    }
}

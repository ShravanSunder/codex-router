use agent_automation::RouteEffectEvidence;
use codex_router_host::{ExternalProviderLaunch, LiveSessionOwnership, LiveSessionOwnershipCheck};
use collaboration_protocol::{
    AttemptId, ChannelDescription, CodexGeneration, DeliveryCorrelationId, EndpointAvailability,
    EndpointDescription, EndpointId, EndpointRef, GenerationNumber, MessageContent,
    MessageDelivery, MessageText, NonEmptyText, ObservationTimestamp, ProviderBindingId,
    ProviderBindingIdentity, ProviderCapabilities, ProviderCapability, ProviderCapabilityEvidence,
    ProviderCapabilityName, ProviderCapabilityStatus, ProviderKind, ProviderRuntimeIdentity,
    ProviderTransport, SessionId, SessionRef, UuidIdentity,
};
use collaboration_service::{
    AttemptEvidenceSink, DeliveryFuture, DeliveryPrecondition, DeliveryRequest, EndpointDirectory,
};
use std::path::{Path, PathBuf};

pub(super) struct NoLivePeer;

impl LiveSessionOwnershipCheck for NoLivePeer {
    fn check<'a>(&'a self, _target: &'a SessionRef) -> DeliveryFuture<'a, LiveSessionOwnership> {
        Box::pin(async { Ok(LiveSessionOwnership::NotLive) })
    }
}

pub(super) struct RecordedEvidence(
    pub(super) tokio::sync::Mutex<Vec<RouteEffectEvidence<SessionRef, CodexGeneration>>>,
);

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

pub(super) fn target() -> SessionRef {
    SessionRef {
        endpoint: EndpointRef {
            service_id: UuidIdentity::try_from("0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89".to_owned())
                .expect("service ID"),
            endpoint_id: EndpointId::try_from("cursor-local".to_owned()).expect("endpoint"),
        },
        session_id: SessionId::try_from("fixture-session".to_owned()).expect("session"),
    }
}

pub(super) fn provider_binding(target: &SessionRef) -> ProviderBindingIdentity {
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

pub(super) fn available_directory(
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

pub(super) fn cursor_prompt_fixture(event_socket: &Path) -> ExternalProviderLaunch {
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

pub(super) fn refusing_load_fixture(load_marker: &Path) -> ExternalProviderLaunch {
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
 print(json.dumps({{'jsonrpc':'2.0','id':request['id'],'error':{{'code':-32001,'message':'refused load'}}}})); sys.stdout.flush()
"#,
        load_marker.display().to_string()
    );
    ExternalProviderLaunch {
        executable: PathBuf::from("/usr/bin/python3"),
        arguments: vec!["-c".to_owned(), script],
        environment: Vec::new(),
    }
}

pub(super) fn request(target: SessionRef, text: &str) -> DeliveryRequest {
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

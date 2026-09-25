use super::*;
use crate::ExternalProviderLaunch;
use collaboration_protocol::{
    CodexGeneration, ConversationCreateRequest, EndpointId, GenerationNumber, ProviderBindingId,
    ProviderCapabilities, ProviderCapability, ProviderCapabilityEvidence, ProviderCapabilityName,
    ProviderCapabilityStatus, ProviderKind, ProviderRequestedPolicy, ProviderRuntimeIdentity,
    ProviderTransport, ProviderWorkingDirectory, RouterAccess, SessionId, UuidIdentity,
};

fn create_fixture() -> ExternalProviderLaunch {
    ExternalProviderLaunch {
        executable: PathBuf::from("/usr/bin/python3"),
        arguments: vec![
            "-c".to_owned(),
            r#"
import json,sys
request=json.loads(sys.stdin.readline())
print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':{'protocolVersion':1,'agentCapabilities':{},'agentInfo':{'name':'race-fixture','version':'1'}}})); sys.stdout.flush()
request=json.loads(sys.stdin.readline())
print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':{'sessionId':'fixture-session'}})); sys.stdout.flush()
sys.stdin.read()
"#
            .to_owned(),
        ],
        environment: Vec::new(),
    }
}

fn endpoint() -> EndpointRef {
    EndpointRef {
        service_id: UuidIdentity::try_from("0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89".to_owned())
            .expect("service ID"),
        endpoint_id: EndpointId::try_from("cursor-local".to_owned()).expect("endpoint ID"),
    }
}

fn generation() -> CodexGeneration {
    CodexGeneration {
        service_epoch: UuidIdentity::try_from("1ff962c5-7fa3-4c18-a5ca-1bbe8db09e80".to_owned())
            .expect("service epoch"),
        generation: GenerationNumber::try_from(1).expect("generation"),
    }
}

fn binding() -> ProviderBindingIdentity {
    ProviderBindingIdentity {
        endpoint: endpoint(),
        binding_id: ProviderBindingId::try_from("cursor-binding".to_owned()).expect("binding ID"),
        runtime: ProviderRuntimeIdentity {
            provider: ProviderKind::Cursor,
            runtime_name: NonEmptyText::try_from("cursor".to_owned()).expect("runtime name"),
            runtime_version: None,
        },
        transport: ProviderTransport::StdioAcp,
        generation: generation(),
        capabilities: ProviderCapabilities::try_from(vec![ProviderCapability {
            name: ProviderCapabilityName::Create,
            status: ProviderCapabilityStatus::Supported,
            evidence: ProviderCapabilityEvidence::Advertised,
        }])
        .expect("capabilities"),
    }
}

fn requester() -> SessionRef {
    SessionRef {
        endpoint: endpoint(),
        session_id: SessionId::try_from("requester".to_owned()).expect("session ID"),
    }
}

#[tokio::test]
async fn concurrent_admission_precedes_reconcile_live_state_check() {
    let root = tempfile::tempdir().expect("temporary root");
    let runtime = ExternalProviderRuntime::initialize(create_fixture())
        .await
        .expect("fixture initializes");
    let store = ProviderOperationStore::open(&root.path().join("operations.sqlite"))
        .await
        .expect("operation store opens");
    let backend = ExternalProviderSupervisor::new(
        vec![ExternalProviderBinding {
            identity: binding(),
            runtime,
        }],
        Arc::new(Mutex::new(store)),
    )
    .expect("supervisor initializes");
    let operation_id = OperationId::generate();

    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    *backend
        .inner
        .admission_test_pause
        .lock()
        .expect("admission test pause") = Some(AdmissionTestPause {
        entered: entered_tx,
        release: release_rx,
    });
    let create_backend = backend.clone();
    let create_operation_id = operation_id.clone();
    let create_task = tokio::spawn(async move {
        create_backend
            .create(ConversationCreateRequest {
                operation_id: create_operation_id,
                endpoint: endpoint(),
                generation: Some(generation()),
                working_directory: ProviderWorkingDirectory::try_from("/tmp".to_owned())
                    .expect("working directory"),
                created_by: requester(),
                approver: requester(),
                requested_policy: ProviderRequestedPolicy {
                    access: RouterAccess::WriteRestricted,
                },
            })
            .await
    });
    entered_rx.await.expect("admission holds the store lock");
    let reconcile_operation_id = operation_id.clone();
    let reconcile = backend.reconcile(ConversationOperationReconcileRequest {
        operation_id: reconcile_operation_id,
    });
    tokio::pin!(reconcile);
    assert!(futures_util::poll!(&mut reconcile).is_pending());
    release_tx.send(()).expect("release admission");

    let submitted = create_task
        .await
        .expect("create task joins")
        .expect("create admitted");
    assert_eq!(submitted.operation.operation_id, operation_id);
    let reconciled = reconcile.await.expect("live operation reconciles");
    assert_eq!(reconciled.stage, ProviderOperationStage::MayHaveDispatched);
    assert_eq!(
        reconciled.reconciliation,
        ProviderReconciliationState::Unresolved
    );

    backend.shutdown().await.expect("supervisor shuts down");
}

#[test]
fn typed_runtime_failure_mapping_never_classifies_provider_text() {
    let operation_id = OperationId::generate();
    for _old_magic_text in [
        "provider conversation is busy",
        "has not been created or loaded",
        "has no active prompt",
        "is no longer active",
    ] {
        let failure = runtime_failure(
            operation_id.clone(),
            None,
            ExternalProviderRuntimeError::ProviderFailure,
        );
        assert_eq!(
            failure.kind,
            ConversationOperationFailureKind::ProviderRejected
        );
        assert_eq!(failure.effect, ProviderOperationEffect::Unknown);
    }
    for error in [
        ExternalProviderRuntimeError::LocalBusy,
        ExternalProviderRuntimeError::LocalNotFound,
        ExternalProviderRuntimeError::LocalCancelTargetMismatch,
        ExternalProviderRuntimeError::AuthenticationRequired,
    ] {
        let failure = runtime_failure(operation_id.clone(), None, error);
        assert_eq!(failure.effect, ProviderOperationEffect::None);
    }
}

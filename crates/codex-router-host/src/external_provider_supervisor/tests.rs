use super::*;
use crate::ExternalProviderLaunch;
use collaboration_protocol::{
    CodexGeneration, ConversationCreateRequest, ConversationPromptRequest, EndpointId,
    GenerationNumber, MessageContent, MessageText, ProviderBindingId, ProviderCapabilities,
    ProviderCapability, ProviderCapabilityEvidence, ProviderCapabilityName,
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

fn pending_prompt_fixture() -> ExternalProviderLaunch {
    ExternalProviderLaunch {
        executable: PathBuf::from("/usr/bin/python3"),
        arguments: vec![
            "-c".to_owned(),
            r#"
import json,sys
request=json.loads(sys.stdin.readline())
print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':{'protocolVersion':1,'agentCapabilities':{},'agentInfo':{'name':'dispatch-fixture','version':'1'}}})); sys.stdout.flush()
request=json.loads(sys.stdin.readline())
assert request['method']=='session/new'
print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':{'sessionId':'fixture-session'}})); sys.stdout.flush()
request=json.loads(sys.stdin.readline())
assert request['method']=='session/prompt'
sys.stdin.read()
"#
            .to_owned(),
        ],
        environment: Vec::new(),
    }
}

fn ordered_prompt_fixture(socket_path: &std::path::Path) -> ExternalProviderLaunch {
    let script = format!(
        r#"
import json,socket,sys
request=json.loads(sys.stdin.readline())
print(json.dumps({{'jsonrpc':'2.0','id':request['id'],'result':{{'protocolVersion':1,'agentCapabilities':{{}},'agentInfo':{{'name':'fifo-fixture','version':'1'}}}}}})); sys.stdout.flush()
request=json.loads(sys.stdin.readline())
assert request['method']=='session/new'
print(json.dumps({{'jsonrpc':'2.0','id':request['id'],'result':{{'sessionId':'fixture-session'}}}})); sys.stdout.flush()
for expected in ('first','second'):
 request=json.loads(sys.stdin.readline())
 assert request['method']=='session/prompt'
 assert request['params']['prompt'][0]['text'].endswith(expected)
 with socket.socket(socket.AF_UNIX,socket.SOCK_STREAM) as event:
  event.connect({:?})
  event.sendall(expected.encode())
 print(json.dumps({{'jsonrpc':'2.0','id':request['id'],'result':{{'stopReason':'end_turn'}}}})); sys.stdout.flush()
sys.stdin.read()
"#,
        socket_path.display().to_string()
    );
    ExternalProviderLaunch {
        executable: PathBuf::from("/usr/bin/python3"),
        arguments: vec!["-c".to_owned(), script],
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

struct NoLivePeer;

impl crate::LiveSessionOwnershipCheck for NoLivePeer {
    fn check<'a>(
        &'a self,
        _target: &'a SessionRef,
    ) -> collaboration_service::DeliveryFuture<'a, crate::LiveSessionOwnership> {
        Box::pin(async { Ok(crate::LiveSessionOwnership::NotLive) })
    }
}

#[tokio::test]
async fn delivery_prompt_reports_submission_before_turn_settles() {
    let root = tempfile::tempdir().expect("temporary root");
    let runtime = ExternalProviderRuntime::initialize(pending_prompt_fixture())
        .await
        .expect("fixture initializes");
    runtime
        .create_session(PathBuf::from("/tmp"))
        .await
        .expect("session/new");
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
    let target = SessionRef {
        endpoint: endpoint(),
        session_id: SessionId::try_from("fixture-session".to_owned()).expect("session"),
    };

    let dispatch = backend
        .submit_delivery_prompt(ConversationPromptRequest {
            operation_id: operation_id.clone(),
            target: target.clone(),
            generation: generation(),
            requested_by: requester(),
            approver: requester(),
            prompt: MessageContent::Router {
                text: MessageText::try_from("start work".to_owned()).expect("message"),
            },
        })
        .await
        .expect("delivery prompt admission");

    assert_eq!(
        dispatch,
        provider_delivery_submission::ProviderPromptDispatch::Submitted
    );
    let operation = backend
        .show(ConversationOperationShowRequest { operation_id })
        .await
        .expect("operation");
    assert_eq!(operation.stage, ProviderOperationStage::MayHaveDispatched);
    backend.shutdown().await.expect("supervisor shuts down");
}

#[tokio::test]
async fn delivery_prompt_without_loaded_session_is_known_not_submitted() {
    let root = tempfile::tempdir().expect("temporary root");
    let runtime = ExternalProviderRuntime::initialize(create_fixture())
        .await
        .expect("fixture initializes");
    let store = ProviderOperationStore::open(&root.path().join("operations.sqlite"))
        .await
        .expect("operation store");
    let backend = ExternalProviderSupervisor::new(
        vec![ExternalProviderBinding {
            identity: binding(),
            runtime,
        }],
        Arc::new(Mutex::new(store)),
    )
    .expect("supervisor");

    let dispatch = backend
        .submit_delivery_prompt(ConversationPromptRequest {
            operation_id: OperationId::generate(),
            target: SessionRef {
                endpoint: endpoint(),
                session_id: SessionId::try_from("fixture-session".to_owned()).expect("session"),
            },
            generation: generation(),
            requested_by: requester(),
            approver: requester(),
            prompt: MessageContent::Router {
                text: MessageText::try_from("start work".to_owned()).expect("message"),
            },
        })
        .await
        .expect("delivery prompt admission");

    assert_eq!(
        dispatch,
        provider_delivery_submission::ProviderPromptDispatch::NotSubmitted
    );
    backend.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn router_queue_drains_provider_prompts_in_fifo_order() {
    use tokio::io::AsyncReadExt as _;
    let root = tempfile::tempdir().expect("temporary root");
    let socket_path = root.path().join("prompt-events.sock");
    let listener = tokio::net::UnixListener::bind(&socket_path).expect("fixture event listener");
    let runtime = ExternalProviderRuntime::initialize(ordered_prompt_fixture(&socket_path))
        .await
        .expect("fixture initializes");
    runtime
        .create_session(PathBuf::from("/tmp"))
        .await
        .expect("session/new");
    let store = ProviderOperationStore::open(&root.path().join("operations.sqlite"))
        .await
        .expect("operation store opens");
    let store = Arc::new(Mutex::new(store));
    let backend = Arc::new(
        ExternalProviderSupervisor::new(
            vec![ExternalProviderBinding {
                identity: binding(),
                runtime,
            }],
            Arc::clone(&store),
        )
        .expect("supervisor initializes"),
    );
    let queue = crate::provider_acp_message_fifo::ProviderAcpMessageFifo::new(
        Arc::clone(&backend),
        store,
        Arc::new(NoLivePeer),
    );
    let target = SessionRef {
        endpoint: endpoint(),
        session_id: SessionId::try_from("fixture-session".to_owned()).expect("session"),
    };
    for text in ["first", "second"] {
        let permit = queue.reserve(&target).expect("queue capacity");
        permit.send(ConversationPromptRequest {
            operation_id: OperationId::generate(),
            target: target.clone(),
            generation: generation(),
            requested_by: requester(),
            approver: requester(),
            prompt: MessageContent::Router {
                text: MessageText::try_from(text.to_owned()).expect("prompt text"),
            },
        });
    }

    for expected in ["first", "second"] {
        let (mut stream, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept())
            .await
            .expect("prompt event deadline")
            .expect("prompt event");
        let mut bytes = Vec::new();
        stream
            .read_to_end(&mut bytes)
            .await
            .expect("prompt event bytes");
        assert_eq!(bytes, expected.as_bytes());
    }
    queue.shutdown().await;
    backend.shutdown().await.expect("supervisor shuts down");
}

#[tokio::test]
async fn router_queue_shutdown_drops_an_unstarted_prompt() {
    let root = tempfile::tempdir().expect("temporary root");
    let runtime = ExternalProviderRuntime::initialize(pending_prompt_fixture())
        .await
        .expect("fixture initializes");
    runtime
        .create_session(PathBuf::from("/tmp"))
        .await
        .expect("session/new");
    let store = Arc::new(Mutex::new(
        ProviderOperationStore::open(&root.path().join("operations.sqlite"))
            .await
            .expect("operation store"),
    ));
    let backend = Arc::new(
        ExternalProviderSupervisor::new(
            vec![ExternalProviderBinding {
                identity: binding(),
                runtime,
            }],
            Arc::clone(&store),
        )
        .expect("supervisor"),
    );
    let target = SessionRef {
        endpoint: endpoint(),
        session_id: SessionId::try_from("fixture-session".to_owned()).expect("session"),
    };
    let active = backend
        .submit_delivery_prompt(ConversationPromptRequest {
            operation_id: OperationId::generate(),
            target: target.clone(),
            generation: generation(),
            requested_by: requester(),
            approver: requester(),
            prompt: MessageContent::Router {
                text: MessageText::try_from("active".to_owned()).expect("message"),
            },
        })
        .await
        .expect("active prompt");
    assert_eq!(
        active,
        provider_delivery_submission::ProviderPromptDispatch::Submitted
    );
    let queue = crate::provider_acp_message_fifo::ProviderAcpMessageFifo::new(
        Arc::clone(&backend),
        Arc::clone(&store),
        Arc::new(NoLivePeer),
    );
    let queued_id = OperationId::generate();
    queue
        .reserve(&target)
        .expect("queue capacity")
        .send(ConversationPromptRequest {
            operation_id: queued_id.clone(),
            target,
            generation: generation(),
            requested_by: requester(),
            approver: requester(),
            prompt: MessageContent::Router {
                text: MessageText::try_from("never started".to_owned()).expect("message"),
            },
        });

    queue.shutdown().await;

    assert!(
        store
            .lock()
            .await
            .inspect(&queued_id)
            .await
            .expect("queued lookup")
            .is_none()
    );
    backend.shutdown().await.expect("supervisor shutdown");
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
                generation: generation(),
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

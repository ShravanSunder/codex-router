use super::*;
use crate::ExternalProviderLaunch;
use collaboration_protocol::{
    CodexGeneration, ConversationCreateRequest, ConversationPromptRequest, EndpointId,
    GenerationNumber, MessageContent, MessageText, PositiveSeconds, ProviderBindingId,
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
            generation: Some(generation()),
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
            generation: Some(generation()),
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
            generation: Some(generation()),
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
            generation: Some(generation()),
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
            generation: Some(generation()),
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
async fn provider_retirement_settles_queued_input_without_resubmission() {
    // Specification R5: queued Inputs of a lost Session become notSubmitted
    // with providerRetired and are never sent to a successor connection.
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
    assert_eq!(
        backend
            .submit_delivery_prompt(ConversationPromptRequest {
                operation_id: OperationId::generate(),
                target: target.clone(),
                generation: Some(generation()),
                requested_by: requester(),
                approver: requester(),
                prompt: MessageContent::Router {
                    text: MessageText::try_from("active".to_owned()).expect("message"),
                },
            })
            .await
            .expect("active prompt"),
        provider_delivery_submission::ProviderPromptDispatch::Submitted
    );
    let queue = crate::provider_acp_message_fifo::ProviderAcpMessageFifo::new(
        Arc::clone(&backend),
        Arc::clone(&store),
        Arc::new(NoLivePeer),
    );
    let queued_id = OperationId::generate();
    let permit = queue.reserve(&target).expect("queue capacity");
    backend
        .queued_operation_registry()
        .record_queued(queued_id.clone(), target.clone(), binding());
    permit.send(ConversationPromptRequest {
        operation_id: queued_id.clone(),
        target,
        generation: Some(generation()),
        requested_by: requester(),
        approver: requester(),
        prompt: MessageContent::Router {
            text: MessageText::try_from("queued".to_owned()).expect("message"),
        },
    });
    backend
        .runtime_for(&endpoint())
        .expect("provider runtime")
        .shutdown()
        .await;
    let snapshot = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let snapshot = backend
                .queued_operation_registry()
                .snapshot(&queued_id)
                .expect("queued operation snapshot")
                .expect("queued operation exists");
            if matches!(
                snapshot.queue_state,
                Some(collaboration_protocol::ConversationOperationQueueState::NotSubmitted { .. })
            ) {
                break snapshot;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("queued operation settles");
    assert!(matches!(
        snapshot.queue_state,
        Some(collaboration_protocol::ConversationOperationQueueState::NotSubmitted { reason })
            if reason == "providerRetired"
    ));
    queue.shutdown().await;
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
            ExternalProviderRuntimeError::ProviderRejected { code: -32603 },
        );
        assert_eq!(
            failure.kind,
            ConversationOperationFailureKind::ProviderRejected
        );
        assert_eq!(failure.effect, ProviderOperationEffect::Unknown);
        assert_eq!(failure.provider_code, Some(-32603));
    }
    for error in [
        ExternalProviderRuntimeError::LocalBusy,
        ExternalProviderRuntimeError::LocalNotFound,
        ExternalProviderRuntimeError::LocalCancelTargetMismatch,
        ExternalProviderRuntimeError::AuthenticationRequired { code: -32000 },
    ] {
        let failure = runtime_failure(operation_id.clone(), None, error);
        assert_eq!(failure.effect, ProviderOperationEffect::None);
    }
}

#[test]
fn acp_error_codes_project_to_existing_public_failure_kinds() {
    // ACP v1 error-codes.mdx and Specification R7 classify by code. PR 1
    // preserves the existing public failure-kind enum and providerCode.
    for (error, expected_kind, code) in [
        (
            ExternalProviderRuntimeError::AuthenticationRequired { code: -32000 },
            ConversationOperationFailureKind::AuthenticationRequired,
            -32000,
        ),
        (
            ExternalProviderRuntimeError::ProviderSessionNotFound { code: -32002 },
            ConversationOperationFailureKind::ProviderSessionNotFound,
            -32002,
        ),
        (
            ExternalProviderRuntimeError::ResourceNotFound { code: -32002 },
            ConversationOperationFailureKind::NotFound,
            -32002,
        ),
        (
            ExternalProviderRuntimeError::UnsupportedMethod { code: -32601 },
            ConversationOperationFailureKind::UnsupportedCapability,
            -32601,
        ),
        (
            ExternalProviderRuntimeError::InvalidParams { code: -32602 },
            ConversationOperationFailureKind::InvalidRequest,
            -32602,
        ),
        (
            ExternalProviderRuntimeError::RequestCancelled { code: -32800 },
            ConversationOperationFailureKind::ProviderRejected,
            -32800,
        ),
        (
            ExternalProviderRuntimeError::ProviderRejected { code: -32603 },
            ConversationOperationFailureKind::ProviderRejected,
            -32603,
        ),
    ] {
        let failure = runtime_failure(OperationId::generate(), None, error);
        assert_eq!(failure.kind, expected_kind, "code {code}");
        assert_eq!(failure.provider_code, Some(code), "code {code}");
    }
}

#[tokio::test]
async fn provider_error_code_fixtures_project_without_agent_text() {
    // ACP v1 error-codes.mdx and Specification R7: each code is classified
    // from a real ACP response, with the agent's message/data kept private.
    for (code, expected_kind) in [
        (
            -32000,
            ConversationOperationFailureKind::AuthenticationRequired,
        ),
        (-32002, ConversationOperationFailureKind::NotFound),
        (
            -32601,
            ConversationOperationFailureKind::UnsupportedCapability,
        ),
        (-32602, ConversationOperationFailureKind::InvalidRequest),
        (-32800, ConversationOperationFailureKind::ProviderRejected),
        (-32603, ConversationOperationFailureKind::ProviderRejected),
    ] {
        let fixture = crate::external_provider_runtime::acp_scripted_fixture::AcpFixtureScript::new()
            .expect_request("initialize", "initialize", serde_json::json!({"protocolVersion": 1}))
            .respond("initialize", serde_json::json!({"protocolVersion": 1, "agentCapabilities": {}, "agentInfo": {"name": "error-fixture", "version": "1"}}))
            .expect_request("create", "session/new", serde_json::json!({}))
            .respond("create", serde_json::json!({"sessionId": "fixture-session"}))
            .expect_request("prompt", "session/prompt", serde_json::json!({"sessionId": "fixture-session"}))
            .respond_error("prompt", code)
            .launch();
        let runtime = ExternalProviderRuntime::initialize(fixture)
            .await
            .expect("fixture initializes");
        runtime
            .create_session(PathBuf::from("/tmp"))
            .await
            .expect("session created");
        let error = runtime
            .prompt("fixture-session".to_owned(), "continue".to_owned())
            .await
            .expect_err("agent rejected prompt");
        let failure = runtime_failure(OperationId::generate(), None, error);
        assert_eq!(failure.kind, expected_kind, "code {code}");
        assert_eq!(failure.provider_code, Some(i64::from(code)), "code {code}");
        let diagnostic = String::from(failure.message);
        assert!(!diagnostic.contains("private provider text"), "code {code}");
        assert!(!diagnostic.contains("secret sentinel"), "code {code}");
        runtime.shutdown().await;
    }
}

#[test]
fn lost_provider_prompt_has_terminal_unknown_effect_and_sanitized_reason() {
    // A prompt dispatched before connection loss cannot be retried safely.
    // Specification E4 and R5 require a terminal lost projection in PR 1.
    let failure = prompt_runtime_failure(
        OperationId::generate(),
        None,
        ExternalProviderRuntimeError::TransportFailure,
    );
    assert_eq!(
        failure.kind,
        ConversationOperationFailureKind::OutcomeUnknown
    );
    assert_eq!(failure.effect, ProviderOperationEffect::Unknown);
    assert_eq!(
        String::from(failure.message),
        "provider connection lost before the agent ended the turn (providerRetired)"
    );
}

#[tokio::test]
async fn unknown_agent_stop_reason_projects_applied_unknown_settlement() {
    // ACP v1 prompt-turn.mdx:369-390 defines the recognized stop reasons.
    // R4 preserves an unknown value until E4 has a typed unknown reason.
    let fixture = crate::external_provider_runtime::acp_scripted_fixture::AcpFixtureScript::new()
        .expect_request("initialize", "initialize", serde_json::json!({"protocolVersion": 1}))
        .respond("initialize", serde_json::json!({"protocolVersion": 1, "agentCapabilities": {}, "agentInfo": {"name": "unknown-stop-fixture", "version": "1"}}))
        .expect_request("create", "session/new", serde_json::json!({}))
        .respond("create", serde_json::json!({"sessionId": "fixture-session"}))
        .expect_request("prompt", "session/prompt", serde_json::json!({"sessionId": "fixture-session"}))
        .respond("prompt", serde_json::json!({"stopReason": "future_reason"}))
        .launch();
    let root = tempfile::tempdir().expect("temporary root");
    let runtime = ExternalProviderRuntime::initialize(fixture)
        .await
        .expect("fixture initializes");
    runtime
        .create_session(root.path().to_owned())
        .await
        .expect("session created");
    let store = Arc::new(Mutex::new(
        ProviderOperationStore::open(&root.path().join("operations.sqlite"))
            .await
            .expect("operation store"),
    ));
    let backend = ExternalProviderSupervisor::new(
        vec![ExternalProviderBinding {
            identity: binding(),
            runtime,
        }],
        store,
    )
    .expect("supervisor");
    let operation_id = OperationId::generate();
    assert_eq!(
        backend
            .submit_delivery_prompt(ConversationPromptRequest {
                operation_id: operation_id.clone(),
                target: SessionRef {
                    endpoint: endpoint(),
                    session_id: SessionId::try_from("fixture-session".to_owned()).expect("session"),
                },
                generation: Some(generation()),
                requested_by: requester(),
                approver: requester(),
                prompt: MessageContent::Router {
                    text: MessageText::try_from("continue".to_owned()).expect("message"),
                },
            })
            .await
            .expect("prompt submitted"),
        provider_delivery_submission::ProviderPromptDispatch::Submitted
    );
    let failure = backend
        .wait(ConversationOperationWaitRequest {
            operation_id: operation_id.clone(),
            timeout_seconds: PositiveSeconds::try_from(5).expect("timeout"),
        })
        .await
        .expect_err("unknown stop reason is a terminal failure projection");
    assert_eq!(
        failure.kind,
        ConversationOperationFailureKind::OutcomeUnknown
    );
    assert_eq!(failure.effect, ProviderOperationEffect::Applied);
    assert_eq!(
        String::from(failure.message),
        "agent ended the turn with an unrecognized stop reason (future_reason)"
    );
    let operation = backend
        .show(ConversationOperationShowRequest { operation_id })
        .await
        .expect("terminal operation");
    assert_eq!(operation.stage, ProviderOperationStage::Terminal);
    backend.shutdown().await.expect("supervisor shutdown");
}

#[test]
fn provider_session_not_found_failure_has_typed_guidance_and_code() {
    let failure = runtime_failure(
        OperationId::generate(),
        None,
        ExternalProviderRuntimeError::ProviderSessionNotFound { code: -32002 },
    );

    assert_eq!(
        failure.kind,
        ConversationOperationFailureKind::ProviderSessionNotFound
    );
    assert_eq!(failure.provider_code, Some(-32002));
    assert_eq!(
        String::from(failure.message),
        "this session never started a turn and did not survive the provider restart; create a new conversation"
    );
}

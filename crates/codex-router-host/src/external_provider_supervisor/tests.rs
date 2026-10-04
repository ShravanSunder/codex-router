use super::*;
use crate::ExternalProviderLaunch;
use collaboration_protocol::{
    CodexGeneration, ConversationCreateRequest, EndpointId, GenerationNumber, MessageText,
    PositiveSeconds, ProviderBindingId, ProviderCapabilities, ProviderCapability,
    ProviderCapabilityEvidence, ProviderCapabilityName, ProviderCapabilityStatus, ProviderKind,
    ProviderPromptStopReason, ProviderRequestedPolicy, ProviderRuntimeIdentity, ProviderTransport,
    ProviderWorkingDirectory, PushId, RouterAccess, SessionId, UuidIdentity,
};
use std::io::{self, Write};
use std::sync::{Arc, Mutex as StdMutex};

struct PreparedPromptFixture {
    operation_id: OperationId,
    input_id: session_event_model::InputId,
    target: SessionRef,
    preview: String,
}

fn prepared_prompt_request(fixture: PreparedPromptFixture) -> ProviderPromptContentsRequest {
    let push_id = PushId::try_from(agent_automation::AttemptId::generate().as_str().to_owned())
        .expect("UUIDv7 push id");
    let line = MessageText::try_from(
        collaboration_protocol::render_push_line(&collaboration_protocol::PushLineInput {
            link: collaboration_protocol::RouterLink::new(
                collaboration_protocol::MachineId::from(fixture.target.endpoint.service_id.clone()),
                push_id,
            ),
            machine_label: collaboration_protocol::MachineLabel::try_from(
                "fixture-host".to_owned(),
            )
            .expect("machine label"),
            origin: collaboration_protocol::PushOrigin::Session(fixture.target.clone()),
            header_facts: collaboration_protocol::PushHeaderFacts::DirectMessage {
                sender_display_name: None,
            },
            body: Some(fixture.preview),
        })
        .expect("prepared push line"),
    )
    .expect("valid push text");
    ProviderPromptContentsRequest::from_prepared_push(
        fixture.operation_id,
        fixture.input_id,
        fixture.target.clone(),
        fixture.target.clone().into(),
        fixture.target.into(),
        &line,
    )
    .expect("provider prompt contents")
}

#[derive(Clone, Default)]
struct CapturedProviderTrace(Arc<StdMutex<Vec<u8>>>);

impl CapturedProviderTrace {
    fn rendered(&self) -> String {
        self.0
            .lock()
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .unwrap_or_default()
    }
}

impl<'writer> tracing_subscriber::fmt::MakeWriter<'writer> for CapturedProviderTrace {
    type Writer = CapturedProviderTraceBuffer;

    fn make_writer(&'writer self) -> Self::Writer {
        CapturedProviderTraceBuffer(Arc::clone(&self.0))
    }
}

struct CapturedProviderTraceBuffer(Arc<StdMutex<Vec<u8>>>);

impl Write for CapturedProviderTraceBuffer {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let mut bytes = self
            .0
            .lock()
            .map_err(|_| io::Error::other("provider trace capture lock poisoned"))?;
        bytes.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn create_fixture() -> ExternalProviderLaunch {
    ExternalProviderLaunch {
        persistence_target: acp_client_runtime::ProviderPersistenceTarget::Unspecified,
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
        persistence_target: acp_client_runtime::ProviderPersistenceTarget::Unspecified,
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
 text=request['params']['prompt'][0]['text']
 assert text.startswith(f'✉️ sender · "{{expected}}" · router://')
 machine_and_push_id=text.split('router://',1)[1]
 machine_id,separator,push_id=machine_and_push_id.partition('/push/')
 assert separator and machine_id and push_id
 with socket.socket(socket.AF_UNIX,socket.SOCK_STREAM) as event:
  event.connect({:?})
  event.sendall(text.encode())
 print(json.dumps({{'jsonrpc':'2.0','id':request['id'],'result':{{'stopReason':'end_turn'}}}})); sys.stdout.flush()
sys.stdin.read()
"#,
        socket_path.display().to_string()
    );
    ExternalProviderLaunch {
        persistence_target: acp_client_runtime::ProviderPersistenceTarget::Unspecified,
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
        .submit_delivery_prompt_contents(prepared_prompt_request(PreparedPromptFixture {
            operation_id: operation_id.clone(),
            input_id: session_event_model::InputId::generate(),
            target: target.clone(),
            preview: "start work".to_owned(),
        }))
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
        .submit_delivery_prompt_contents(prepared_prompt_request(PreparedPromptFixture {
            operation_id: OperationId::generate(),
            input_id: session_event_model::InputId::generate(),
            target: SessionRef {
                endpoint: endpoint(),
                session_id: SessionId::try_from("fixture-session".to_owned()).expect("session"),
            },
            preview: "start work".to_owned(),
        }))
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
    let store = ProviderOperationStore::open(&root.path().join("operations.sqlite"))
        .await
        .expect("operation store opens");
    let store = Arc::new(Mutex::new(store));
    let hub = Arc::new(collaboration_service::ProviderSessionEventHub::new(
        Arc::clone(&store),
    ));
    let hub_endpoint: message_board::SessionEndpointRef =
        serde_json::from_value(serde_json::to_value(endpoint()).expect("endpoint JSON"))
            .expect("hub endpoint");
    let runtime = ExternalProviderRuntime::initialize_with_mcp_http_and_hub(
        ordered_prompt_fixture(&socket_path),
        "fixture",
        "http://127.0.0.1:1/mcp",
        Arc::clone(&hub),
        hub_endpoint,
    )
    .await
    .expect("fixture initializes");
    runtime
        .create_session(PathBuf::from("/tmp"))
        .await
        .expect("session/new");
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
    let mut queued_input_ids = Vec::new();
    let mut expected_lines = Vec::new();
    for text in ["first", "second"] {
        let permit = queue.reserve(&target).expect("queue capacity");
        let operation_id = OperationId::generate();
        let input_id = session_event_model::InputId::generate();
        queued_input_ids.push(input_id.clone());
        let push_id = PushId::try_from(agent_automation::AttemptId::generate().as_str().to_owned())
            .expect("UUIDv7 push id");
        let prompt = MessageText::try_from(format!(
            "✉️ sender · \"{text}\" · router://{}/push/{}",
            String::from(target.endpoint.service_id.clone()),
            push_id.as_str()
        ))
        .expect("prepared push line");
        let contents =
            crate::external_provider_supervisor::ProviderPromptContentsRequest::from_prepared_push(
                operation_id.clone(),
                input_id.clone(),
                target.clone(),
                (requester()).into(),
                (requester()).into(),
                &prompt,
            )
            .expect("queued prepared contents");
        expected_lines.push(prompt.as_str().to_owned());
        backend.queued_operation_registry().record_queued_contents(
            operation_id.clone(),
            target.clone(),
            binding(),
            input_id.clone(),
            &contents.contents,
        );
        permit.send(
            crate::provider_acp_message_fifo::ProviderQueuedPrompt::Contents {
                request: contents,
                load_policy: collaboration_service::LoadPolicy::MayLoad,
            },
        );
    }

    for expected in expected_lines {
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
    let hub_target: message_board::SessionRef =
        serde_json::from_value(serde_json::to_value(target).expect("target JSON"))
            .expect("hub target");
    let attached = collaboration_service::SessionEventHub::attach(hub.as_ref(), hub_target)
        .await
        .expect("hub history");
    let started = attached
        .snapshot
        .into_iter()
        .filter_map(|event| match event.event {
            session_event_model::SessionEvent::TurnStarted { input_id, .. } => Some(input_id),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(started, queued_input_ids);
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
        .submit_delivery_prompt_contents(prepared_prompt_request(PreparedPromptFixture {
            operation_id: OperationId::generate(),
            input_id: session_event_model::InputId::generate(),
            target: target.clone(),
            preview: "active".to_owned(),
        }))
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
    let queued_input = session_event_model::InputId::generate();
    let queued_id_push =
        PushId::try_from(agent_automation::AttemptId::generate().as_str().to_owned())
            .expect("UUIDv7 push id");
    let queued_prompt = MessageText::try_from(format!(
        "✉️ sender · \"never started\" · router://{}/push/{}",
        String::from(target.endpoint.service_id.clone()),
        queued_id_push.as_str()
    ))
    .expect("prepared push line");
    let queued_contents =
        crate::external_provider_supervisor::ProviderPromptContentsRequest::from_prepared_push(
            queued_id.clone(),
            queued_input.clone(),
            target.clone(),
            (requester()).into(),
            (requester()).into(),
            &queued_prompt,
        )
        .expect("queued prepared contents");
    backend.queued_operation_registry().record_queued_contents(
        queued_id.clone(),
        target.clone(),
        binding(),
        queued_input.clone(),
        &queued_contents.contents,
    );
    queue.reserve(&target).expect("queue capacity").send(
        crate::provider_acp_message_fifo::ProviderQueuedPrompt::Contents {
            request: queued_contents,
            load_policy: collaboration_service::LoadPolicy::MayLoad,
        },
    );

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
            .submit_delivery_prompt_contents(prepared_prompt_request(PreparedPromptFixture {
                operation_id: OperationId::generate(),
                input_id: session_event_model::InputId::generate(),
                target: target.clone(),
                preview: "active".to_owned(),
            }))
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
    let queued_input_id = session_event_model::InputId::generate();
    let permit = queue.reserve(&target).expect("queue capacity");
    let push_id = PushId::try_from(agent_automation::AttemptId::generate().as_str().to_owned())
        .expect("UUIDv7 push id");
    let queued_line = MessageText::try_from(format!(
        "✉️ sender · \"queued work\" · router://{}/push/{}",
        String::from(target.endpoint.service_id.clone()),
        push_id.as_str()
    ))
    .expect("prepared push line");
    let queued_contents =
        crate::external_provider_supervisor::ProviderPromptContentsRequest::from_prepared_push(
            queued_id.clone(),
            queued_input_id.clone(),
            target.clone(),
            (requester()).into(),
            (requester()).into(),
            &queued_line,
        )
        .expect("queued prepared contents");
    backend.queued_operation_registry().record_queued_contents(
        queued_id.clone(),
        target.clone(),
        binding(),
        queued_input_id.clone(),
        &queued_contents.contents,
    );
    permit.send(
        crate::provider_acp_message_fifo::ProviderQueuedPrompt::Contents {
            request: queued_contents,
            load_policy: collaboration_service::LoadPolicy::MayLoad,
        },
    );
    backend
        .runtime_for(&endpoint())
        .expect("provider runtime")
        .shutdown()
        .await;
    tokio::time::timeout(Duration::from_secs(5), queue.wait_for_workers())
        .await
        .expect("queued operation worker settles after provider retirement");
    let snapshot = backend
        .queued_operation_registry()
        .snapshot(&queued_id)
        .expect("queued operation snapshot")
        .expect("queued operation exists");
    assert_eq!(snapshot.input_id, Some(queued_input_id));
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
                settings: None,
                operation_id: create_operation_id,
                endpoint: endpoint(),
                generation: Some(generation()),
                working_directory: ProviderWorkingDirectory::try_from("/tmp".to_owned())
                    .expect("working directory"),
                created_by: (requester()).into(),
                approver: (requester()).into(),
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
            ExternalProviderRuntimeError::ProviderRejected {
                code: -32603,
                correlation_id: acp_client_runtime::ProviderErrorCorrelationId::generate(),
            },
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
        ExternalProviderRuntimeError::AuthenticationRequired {
            code: -32000,
            correlation_id: acp_client_runtime::ProviderErrorCorrelationId::generate(),
        },
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
            ExternalProviderRuntimeError::AuthenticationRequired {
                code: -32000,
                correlation_id: acp_client_runtime::ProviderErrorCorrelationId::generate(),
            },
            ConversationOperationFailureKind::AuthenticationRequired,
            -32000,
        ),
        (
            ExternalProviderRuntimeError::ProviderSessionNotFound {
                code: -32002,
                correlation_id: acp_client_runtime::ProviderErrorCorrelationId::generate(),
            },
            ConversationOperationFailureKind::ProviderSessionNotFound,
            -32002,
        ),
        (
            ExternalProviderRuntimeError::ResourceNotFound {
                code: -32002,
                correlation_id: acp_client_runtime::ProviderErrorCorrelationId::generate(),
            },
            ConversationOperationFailureKind::NotFound,
            -32002,
        ),
        (
            ExternalProviderRuntimeError::UnsupportedMethod {
                code: -32601,
                correlation_id: acp_client_runtime::ProviderErrorCorrelationId::generate(),
            },
            ConversationOperationFailureKind::UnsupportedCapability,
            -32601,
        ),
        (
            ExternalProviderRuntimeError::InvalidParams {
                code: -32602,
                correlation_id: acp_client_runtime::ProviderErrorCorrelationId::generate(),
            },
            ConversationOperationFailureKind::InvalidRequest,
            -32602,
        ),
        (
            ExternalProviderRuntimeError::RequestCancelled {
                code: -32800,
                correlation_id: acp_client_runtime::ProviderErrorCorrelationId::generate(),
            },
            ConversationOperationFailureKind::ProviderRejected,
            -32800,
        ),
        (
            ExternalProviderRuntimeError::ProviderRejected {
                code: -32603,
                correlation_id: acp_client_runtime::ProviderErrorCorrelationId::generate(),
            },
            ConversationOperationFailureKind::ProviderRejected,
            -32603,
        ),
    ] {
        let correlation_reference = match &error {
            ExternalProviderRuntimeError::AuthenticationRequired { correlation_id, .. }
            | ExternalProviderRuntimeError::ProviderSessionNotFound { correlation_id, .. }
            | ExternalProviderRuntimeError::ResourceNotFound { correlation_id, .. }
            | ExternalProviderRuntimeError::UnsupportedMethod { correlation_id, .. }
            | ExternalProviderRuntimeError::InvalidParams { correlation_id, .. }
            | ExternalProviderRuntimeError::RequestCancelled { correlation_id, .. }
            | ExternalProviderRuntimeError::ProviderRejected { correlation_id, .. } => {
                correlation_id.to_string()
            }
            _ => panic!("expected a coded ACP error, got {error:?}"),
        };
        let failure = runtime_failure(OperationId::generate(), None, error);
        assert_eq!(failure.kind, expected_kind, "code {code}");
        assert_eq!(failure.provider_code, Some(code), "code {code}");
        assert!(
            String::from(failure.message).contains(&correlation_reference),
            "code {code} lost correlation reference {correlation_reference}"
        );
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
fn coded_provider_error_trace_reference_matches_safe_host_failure() {
    let raw_provider_text = "private provider rejection detail";
    let provider_error_codes: [i32; 6] = [-32000, -32002, -32601, -32602, -32800, -32603];
    for provider_code in provider_error_codes {
        let mut acp_error = if provider_code == -32000 {
            agent_client_protocol::Error::auth_required()
        } else {
            agent_client_protocol::Error::new(provider_code, raw_provider_text)
        };
        acp_error.message = raw_provider_text.to_owned();
        acp_error.data = Some(serde_json::json!({"privateData":"provider data sentinel"}));

        let captured_trace = CapturedProviderTrace::default();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .without_time()
            .with_max_level(tracing::Level::TRACE)
            .with_writer(captured_trace.clone())
            .finish();
        let provider_error = tracing::subscriber::with_default(subscriber, || {
            acp_client_runtime::acp_operation_error_for_test(acp_error)
        });
        let trace_output = captured_trace.rendered();
        let correlation_reference = match &provider_error {
            ExternalProviderRuntimeError::AuthenticationRequired {
                code,
                correlation_id,
            }
            | ExternalProviderRuntimeError::ProviderSessionNotFound {
                code,
                correlation_id,
            }
            | ExternalProviderRuntimeError::ResourceNotFound {
                code,
                correlation_id,
            }
            | ExternalProviderRuntimeError::UnsupportedMethod {
                code,
                correlation_id,
            }
            | ExternalProviderRuntimeError::InvalidParams {
                code,
                correlation_id,
            }
            | ExternalProviderRuntimeError::RequestCancelled {
                code,
                correlation_id,
            }
            | ExternalProviderRuntimeError::ProviderRejected {
                code,
                correlation_id,
            } => {
                assert_eq!(*code, i64::from(provider_code));
                correlation_id.to_string()
            }
            other => panic!("expected a typed ACP error, got {other:?}"),
        };
        let correlation_uuid =
            uuid::Uuid::parse_str(&correlation_reference).expect("UUIDv7 correlation reference");
        assert_eq!(correlation_uuid.get_version_num(), 7);
        let provider_diagnostic = provider_error.to_string();
        let failure = runtime_failure(OperationId::generate(), None, provider_error);
        let safe_host_failure = String::from(failure.message);

        assert_eq!(failure.provider_code, Some(i64::from(provider_code)));
        assert!(
            trace_output.contains(&format!("provider_code={provider_code}")),
            "{trace_output}"
        );
        assert!(
            trace_output.contains(&format!("correlation_id={correlation_reference}")),
            "{trace_output}"
        );
        assert!(
            trace_output.contains(&format!("raw_provider_text=\"{raw_provider_text}\"")),
            "{trace_output}"
        );
        assert!(
            safe_host_failure.contains(&format!("provider code {provider_code}")),
            "{safe_host_failure}"
        );
        assert!(safe_host_failure.contains(&correlation_reference));
        assert!(!provider_diagnostic.contains(raw_provider_text));
        assert!(!provider_diagnostic.contains("provider data sentinel"));
        assert!(!safe_host_failure.contains(raw_provider_text));
        assert!(!safe_host_failure.contains("provider data sentinel"));
    }
}

#[path = "failure_projection_tests.rs"]
mod failure_projection_tests;

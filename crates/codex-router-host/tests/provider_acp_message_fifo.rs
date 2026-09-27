#![allow(clippy::expect_used)]
//! Queue draining and inspectable provider queue operation behavior.

use codex_router_host::{
    ExternalProviderBinding, ExternalProviderRuntime, ExternalProviderSupervisor,
    ProviderAcpDeliveryRoute,
};
use collaboration_protocol::{
    DeliveryOutcome, MessageContent, MessageDelivery, MessageText, ProviderOperationKind,
    ProviderRequestedPolicy, ProviderWorkingDirectory, RouterAccess,
};
use collaboration_service::{ProviderOperationStore, ProviderSessionRecord, SessionDeliveryRoute};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

#[path = "support/provider_acp_message_fifo_support.rs"]
mod provider_acp_message_fifo_support;
use provider_acp_message_fifo_support::{
    NoLivePeer, RecordedEvidence, available_directory, cursor_prompt_fixture,
    first_prompt_refusal_fixture, provider_binding, refusing_load_fixture, request, target,
};

#[tokio::test]
async fn cursor_queue_drains_after_control_prompt_settles() {
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
    let route = ProviderAcpDeliveryRoute::new(
        target.endpoint.service_id.clone(),
        std::iter::once(target.endpoint.clone()).collect(),
        available_directory(&target, &binding),
        Arc::clone(&supervisor),
        Arc::clone(&store),
        Arc::new(NoLivePeer),
    );

    let first_operation_id = collaboration_protocol::OperationId::generate();
    let first = collaboration_service::ProviderConversationBackend::prompt(
        supervisor.as_ref(),
        collaboration_protocol::ConversationPromptRequest {
            operation_id: first_operation_id,
            target: target.clone(),
            generation: Some(binding.generation.clone()),
            requested_by: target.clone(),
            approver: target.clone(),
            prompt: MessageContent::Router {
                text: MessageText::try_from("first".to_owned()).expect("prompt"),
            },
        },
    )
    .await
    .expect("Control conversation/prompt admission");
    assert_eq!(
        first.operation.operation,
        ProviderOperationKind::ConversationPrompt
    );

    let (mut first_event, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept())
        .await
        .expect("first event deadline")
        .expect("first event");
    let mut first_bytes = [0_u8; 5];
    first_event
        .read_exact(&mut first_bytes)
        .await
        .expect("first bytes");
    let evidence = RecordedEvidence(tokio::sync::Mutex::new(Vec::new()));
    let queued_request = request(target.clone(), "second");
    let queued_operation_id =
        collaboration_protocol::OperationId::try_from(queued_request.attempt.as_str().to_owned())
            .expect("queue operation ID");
    let queued = route
        .deliver(queued_request, &evidence)
        .await
        .expect("queued delivery");
    assert!(matches!(queued.outcome, DeliveryOutcome::Queued));
    let queued_snapshot = collaboration_service::ProviderConversationBackend::show(
        supervisor.as_ref(),
        collaboration_protocol::ConversationOperationShowRequest {
            operation_id: queued_operation_id.clone(),
        },
    )
    .await
    .expect("queued operation is inspectable");
    assert_eq!(
        queued_snapshot.queue_state,
        Some(collaboration_protocol::ConversationOperationQueueState::RouterQueued)
    );

    let blocker_id = collaboration_protocol::OperationId::generate();
    {
        let mut provider_store = store.lock().await;
        provider_store
            .admit(collaboration_service::ProviderOperationAdmission {
                operation_id: blocker_id.clone(),
                operation_kind: ProviderOperationKind::ConversationPrompt,
                binding: collaboration_protocol::ConversationBindingIdentity::ExternalProvider {
                    binding: binding.clone(),
                },
                admitted_at_ms: 2,
            })
            .await
            .expect("admit transient busy blocker");
        provider_store
            .mark_may_have_dispatched(&blocker_id, 3)
            .await
            .expect("mark transient busy blocker");
    }

    first_event
        .write_all(b"x")
        .await
        .expect("release first prompt");
    let first_settlement = collaboration_service::ProviderConversationBackend::wait(
        supervisor.as_ref(),
        collaboration_protocol::ConversationOperationWaitRequest {
            operation_id: first.operation.operation_id,
            timeout_seconds: collaboration_protocol::PositiveSeconds::try_from(2)
                .expect("wait seconds"),
        },
    )
    .await
    .expect("first prompt settles");
    assert!(matches!(
        first_settlement.output,
        collaboration_protocol::ConversationOperationWaitOutput::Available { .. }
    ));
    tokio::time::sleep(Duration::from_millis(100)).await;
    store
        .lock()
        .await
        .record_terminal(
            &blocker_id,
            collaboration_protocol::ProviderOperationEffect::None,
            collaboration_protocol::ProviderReconciliationState::Confirmed,
            None,
            4,
        )
        .await
        .expect("release transient busy blocker");
    let (mut second_event, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept())
        .await
        .expect("queued prompt must drain after Control prompt settles")
        .expect("second event");
    let mut second_bytes = Vec::new();
    second_event
        .read_to_end(&mut second_bytes)
        .await
        .expect("second bytes");
    assert_eq!(first_bytes, *b"first");
    assert_eq!(second_bytes, b"second");
    let submitted_snapshot = collaboration_service::ProviderConversationBackend::show(
        supervisor.as_ref(),
        collaboration_protocol::ConversationOperationShowRequest {
            operation_id: queued_operation_id,
        },
    )
    .await
    .expect("submitted operation uses provider operation record");
    assert_eq!(submitted_snapshot.queue_state, None);
    assert_eq!(
        submitted_snapshot.stage,
        collaboration_protocol::ProviderOperationStage::MayHaveDispatched
    );

    route.shutdown_queue().await;
    supervisor.shutdown().await.expect("supervisor shutdown");
}

#[tokio::test]
async fn permanent_queued_load_failure_is_inspectable_and_does_not_stop_fifo() {
    let root = tempfile::tempdir().expect("provider root");
    let load_marker = root.path().join("load-marker.txt");
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
        std::iter::once(target.endpoint.clone()).collect(),
        available_directory(&target, &binding),
        Arc::clone(&supervisor),
        store,
        Arc::new(NoLivePeer),
    );
    let evidence = RecordedEvidence(tokio::sync::Mutex::new(Vec::new()));
    let mut first_request = request(target.clone(), "first load failure");
    first_request.mode = MessageDelivery::Queue;
    let first_id =
        collaboration_protocol::OperationId::try_from(first_request.attempt.as_str().to_owned())
            .expect("first queue operation ID");
    let mut second_request = request(target, "second load failure");
    second_request.mode = MessageDelivery::Queue;
    let second_id =
        collaboration_protocol::OperationId::try_from(second_request.attempt.as_str().to_owned())
            .expect("second queue operation ID");
    assert!(matches!(
        route
            .deliver(first_request, &evidence)
            .await
            .expect("first queue")
            .outcome,
        DeliveryOutcome::Queued
    ));
    assert!(matches!(
        route
            .deliver(second_request, &evidence)
            .await
            .expect("second queue")
            .outcome,
        DeliveryOutcome::Queued
    ));

    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let first = collaboration_service::ProviderConversationBackend::show(
                supervisor.as_ref(),
                collaboration_protocol::ConversationOperationShowRequest {
                    operation_id: first_id.clone(),
                },
            )
            .await
            .expect("first queued state");
            let second = collaboration_service::ProviderConversationBackend::show(
                supervisor.as_ref(),
                collaboration_protocol::ConversationOperationShowRequest {
                    operation_id: second_id.clone(),
                },
            )
            .await
            .expect("second queued state");
            if matches!(
                (&first.queue_state, &second.queue_state),
                (
                    Some(
                        collaboration_protocol::ConversationOperationQueueState::NotSubmitted { .. }
                    ),
                    Some(
                        collaboration_protocol::ConversationOperationQueueState::NotSubmitted { .. }
                    )
                )
            ) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("both permanent item failures are recorded");
    assert_eq!(
        std::fs::read_to_string(load_marker)
            .expect("load marker")
            .lines()
            .count(),
        2
    );

    route.shutdown_queue().await;
    supervisor.shutdown().await.expect("supervisor shutdown");
}

#[tokio::test]
async fn failed_queued_prompt_advances_to_the_next_accepted_item() {
    let root = tempfile::tempdir().expect("provider root");
    let event_socket = root.path().join("prompt-events.sock");
    let listener = tokio::net::UnixListener::bind(&event_socket).expect("fixture events");
    let target = target();
    let binding = provider_binding(&target);
    let runtime = ExternalProviderRuntime::initialize(first_prompt_refusal_fixture(&event_socket))
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
    let active_prompt = collaboration_service::ProviderConversationBackend::prompt(
        supervisor.as_ref(),
        collaboration_protocol::ConversationPromptRequest {
            operation_id: collaboration_protocol::OperationId::generate(),
            target: target.clone(),
            generation: Some(binding.generation.clone()),
            requested_by: target.clone(),
            approver: target.clone(),
            prompt: MessageContent::Router {
                text: MessageText::try_from("active prompt".to_owned()).expect("prompt"),
            },
        },
    )
    .await
    .expect("active prompt admission");
    let (mut active_event, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept())
        .await
        .expect("active prompt event deadline")
        .expect("active prompt event");
    let mut active_bytes = [0_u8; 6];
    active_event
        .read_exact(&mut active_bytes)
        .await
        .expect("active event bytes");
    assert_eq!(&active_bytes, b"active");
    let route = ProviderAcpDeliveryRoute::new(
        target.endpoint.service_id.clone(),
        std::iter::once(target.endpoint.clone()).collect(),
        available_directory(&target, &binding),
        Arc::clone(&supervisor),
        Arc::clone(&store),
        Arc::new(NoLivePeer),
    );
    let evidence = RecordedEvidence(tokio::sync::Mutex::new(Vec::new()));
    let mut first_request = request(target.clone(), "first queued prompt");
    first_request.mode = MessageDelivery::Queue;
    let first_id =
        collaboration_protocol::OperationId::try_from(first_request.attempt.as_str().to_owned())
            .expect("first queue operation ID");
    let mut second_request = request(target.clone(), "second queued prompt");
    second_request.mode = MessageDelivery::Queue;
    let second_id =
        collaboration_protocol::OperationId::try_from(second_request.attempt.as_str().to_owned())
            .expect("second queue operation ID");
    for queued_request in [first_request, second_request] {
        assert!(matches!(
            route
                .deliver(queued_request, &evidence)
                .await
                .expect("queued delivery")
                .outcome,
            DeliveryOutcome::Queued
        ));
    }
    active_event
        .write_all(b"x")
        .await
        .expect("release active prompt");
    let event_result = tokio::time::timeout(Duration::from_secs(3), listener.accept()).await;
    if event_result.is_err() {
        let first_state = collaboration_service::ProviderConversationBackend::show(
            supervisor.as_ref(),
            collaboration_protocol::ConversationOperationShowRequest {
                operation_id: first_id.clone(),
            },
        )
        .await
        .expect("first operation diagnostic");
        let second_state = collaboration_service::ProviderConversationBackend::show(
            supervisor.as_ref(),
            collaboration_protocol::ConversationOperationShowRequest {
                operation_id: second_id.clone(),
            },
        )
        .await
        .expect("second operation diagnostic");
        panic!(
            "second queued prompt event timed out; first={first_state:?}; second={second_state:?}"
        );
    }
    let (mut event, _) = event_result
        .expect("checked event deadline")
        .expect("second queued prompt event");
    let mut received = [0_u8; 6];
    event.read_exact(&mut received).await.expect("event bytes");
    assert_eq!(&received, b"second");
    let second_settlement = collaboration_service::ProviderConversationBackend::wait(
        supervisor.as_ref(),
        collaboration_protocol::ConversationOperationWaitRequest {
            operation_id: second_id.clone(),
            timeout_seconds: collaboration_protocol::PositiveSeconds::try_from(2)
                .expect("wait seconds"),
        },
    )
    .await
    .expect("second prompt settlement");
    assert!(matches!(
        second_settlement.output,
        collaboration_protocol::ConversationOperationWaitOutput::Available { .. }
    ));
    let first = collaboration_service::ProviderConversationBackend::show(
        supervisor.as_ref(),
        collaboration_protocol::ConversationOperationShowRequest {
            operation_id: first_id,
        },
    )
    .await
    .expect("first operation remains inspectable");
    assert_eq!(
        first.stage,
        collaboration_protocol::ProviderOperationStage::Terminal
    );
    assert_eq!(
        first.effect,
        collaboration_protocol::ProviderOperationEffect::None
    );
    let second = collaboration_service::ProviderConversationBackend::show(
        supervisor.as_ref(),
        collaboration_protocol::ConversationOperationShowRequest {
            operation_id: second_id,
        },
    )
    .await
    .expect("second operation remains inspectable");
    assert_eq!(
        second.stage,
        collaboration_protocol::ProviderOperationStage::Terminal
    );
    assert_eq!(
        second.effect,
        collaboration_protocol::ProviderOperationEffect::Applied
    );
    let active_settlement = collaboration_service::ProviderConversationBackend::wait(
        supervisor.as_ref(),
        collaboration_protocol::ConversationOperationWaitRequest {
            operation_id: active_prompt.operation.operation_id,
            timeout_seconds: collaboration_protocol::PositiveSeconds::try_from(2)
                .expect("wait seconds"),
        },
    )
    .await
    .expect("active prompt settlement");
    assert!(matches!(
        active_settlement.output,
        collaboration_protocol::ConversationOperationWaitOutput::Available { .. }
    ));

    route.shutdown_queue().await;
    supervisor.shutdown().await.expect("supervisor shutdown");
}

#[tokio::test]
async fn provider_retirement_marks_queued_items_not_submitted() {
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
    let route = ProviderAcpDeliveryRoute::new(
        target.endpoint.service_id.clone(),
        std::iter::once(target.endpoint.clone()).collect(),
        available_directory(&target, &binding),
        Arc::clone(&supervisor),
        store,
        Arc::new(NoLivePeer),
    );
    let first = collaboration_service::ProviderConversationBackend::prompt(
        supervisor.as_ref(),
        collaboration_protocol::ConversationPromptRequest {
            operation_id: collaboration_protocol::OperationId::generate(),
            target: target.clone(),
            generation: Some(binding.generation.clone()),
            requested_by: target.clone(),
            approver: target.clone(),
            prompt: MessageContent::Router {
                text: MessageText::try_from("active".to_owned()).expect("prompt"),
            },
        },
    )
    .await
    .expect("Control conversation/prompt admission");
    let (_first_event, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept())
        .await
        .expect("first event deadline")
        .expect("first event");
    let evidence = RecordedEvidence(tokio::sync::Mutex::new(Vec::new()));
    let queued_request = request(target.clone(), "drop on retirement");
    let queued_id =
        collaboration_protocol::OperationId::try_from(queued_request.attempt.as_str().to_owned())
            .expect("queued operation ID");
    assert!(matches!(
        route
            .deliver(queued_request, &evidence)
            .await
            .expect("queued delivery")
            .outcome,
        DeliveryOutcome::Queued
    ));
    let second_queued_request = request(target, "also drop on retirement");
    let second_queued_id = collaboration_protocol::OperationId::try_from(
        second_queued_request.attempt.as_str().to_owned(),
    )
    .expect("second queued operation ID");
    assert!(matches!(
        route
            .deliver(second_queued_request, &evidence)
            .await
            .expect("second queued delivery")
            .outcome,
        DeliveryOutcome::Queued
    ));

    supervisor.shutdown().await.expect("provider retires");
    for operation_id in [queued_id, second_queued_id] {
        let snapshot = collaboration_service::ProviderConversationBackend::show(
            supervisor.as_ref(),
            collaboration_protocol::ConversationOperationShowRequest { operation_id },
        )
        .await
        .expect("retired queue item remains inspectable");
        assert!(matches!(
            snapshot.queue_state,
            Some(collaboration_protocol::ConversationOperationQueueState::NotSubmitted { reason })
                if reason == "providerRetired"
        ));
    }
    let _ = first;
    route.shutdown_queue().await;
}

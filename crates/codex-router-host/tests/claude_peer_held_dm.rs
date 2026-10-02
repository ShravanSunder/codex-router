#![allow(clippy::expect_used, clippy::unwrap_used)]
//! Closed Claude peers hold Auto DMs until their owner publishes a writer.

use automation_storage::AutomationStore;
use claude_code_peer_messaging::{ClaudeCodePeerSocket, ClaudeCodeSessionRegistry};
use codex_router_host::ClaudeCodePeerDeliveryRoute;
use collaboration_protocol::{
    CodexGeneration, DeliveryOutcome, DeliveryRejectionReason, EndpointId, EndpointRef,
    MessageDelivery, PushDeliveryState, PushHeaderFacts, PushId, PushKind, PushOrigin,
    PushRecordDraft, SessionId, SessionRef, UuidIdentity,
};
use collaboration_service::{
    BoardAvailability, MachineIdentity, SessionDeliveryRouter, SessionMessageDelivery,
    SubscriptionDeliveryService, SubscriptionDeliveryServiceProps, SystemSubscriptionClock,
    TargetPresenceProbe,
};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use std::{os::unix::fs::PermissionsExt as _, sync::Arc, time::Duration};
use tokio::io::{AsyncBufReadExt as _, BufReader};
use tokio::sync::Mutex;

fn target() -> SessionRef {
    SessionRef {
        endpoint: EndpointRef {
            service_id: UuidIdentity::try_from("0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89".to_owned())
                .expect("service id"),
            endpoint_id: EndpointId::try_from("claude-local".to_owned()).expect("endpoint id"),
        },
        session_id: SessionId::try_from("closed-peer-session".to_owned()).expect("session id"),
    }
}

fn draft(
    target: &SessionRef,
    mode: MessageDelivery,
    guard: Option<CodexGeneration>,
) -> PushRecordDraft {
    PushRecordDraft {
        push_id: PushId::try_from(uuid::Uuid::now_v7().to_string()).expect("push id"),
        kind: PushKind::DirectMessage,
        origin: PushOrigin::OwnerUnverified,
        origin_router_ref: None,
        target: target.clone(),
        reply_to_push_id: None,
        header_facts: PushHeaderFacts::DirectMessage {
            sender_display_name: None,
        },
        body: Some(format!("resume {mode:?} delivery")),
        activity: None,
        mode: Some(mode),
        guard,
        created_at: chrono::Utc::now(),
    }
}

fn generation_guard() -> CodexGeneration {
    serde_json::from_value(json!({
        "serviceEpoch":"00000000-0000-4000-8000-000000000001",
        "generation":1
    }))
    .expect("Codex generation guard")
}

fn publish_peer(registry: &std::path::Path, socket_path: &std::path::Path) {
    let process_id = std::process::id();
    std::fs::write(
        registry.join(format!("{process_id}.json")),
        json!({
            "pid": process_id,
            "sessionId": "closed-peer-session",
            "status": "idle",
            "peerProtocol": 1,
            "messagingSocketPath": socket_path,
        })
        .to_string(),
    )
    .expect("peer registry record");
    let digest = Sha256::digest(socket_path.to_str().expect("socket path").as_bytes());
    let key_path = registry.join(format!("{process_id}.{digest:x}.key"));
    std::fs::write(
        &key_path,
        json!({"peerToken":"0123456789abcdef0123456789abcdef"}).to_string(),
    )
    .expect("peer authentication record");
    std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600))
        .expect("private peer authentication record");
}

async fn start_peer_delivery_service(
    root: &std::path::Path,
) -> (
    std::path::PathBuf,
    Arc<Mutex<AutomationStore>>,
    SubscriptionDeliveryService,
) {
    let registry = root.join("claude-sessions");
    std::fs::create_dir(&registry).expect("peer registry directory");
    let automation = Arc::new(Mutex::new(
        AutomationStore::open(&root.join("automation.sqlite"))
            .await
            .expect("push storage"),
    ));
    let route = Arc::new(ClaudeCodePeerDeliveryRoute::new(
        target().endpoint,
        Arc::new(ClaudeCodeSessionRegistry::new(registry.clone())),
        Arc::new(ClaudeCodePeerSocket::new(registry.clone())),
    ));
    let router = Arc::new(SessionDeliveryRouter::new(vec![route]));
    let delivery: Arc<dyn SessionMessageDelivery> = router.clone();
    let presence: Arc<dyn TargetPresenceProbe> = router;
    let service = SubscriptionDeliveryService::new(SubscriptionDeliveryServiceProps {
        board_availability: BoardAvailability::Unavailable,
        push_store: Arc::clone(&automation),
        delivery,
        presence,
        machine_identity: MachineIdentity::new(
            target().endpoint.service_id,
            Some("fixture-machine"),
        )
        .expect("machine identity"),
        clock: Arc::new(SystemSubscriptionClock),
    });
    service.start().await.expect("reader actor startup");
    (registry, automation, service)
}

#[tokio::test]
async fn closed_peer_holds_auto_dm_then_delivers_once_when_writable() {
    let directory = tempfile::tempdir().expect("isolated test directory");
    let (registry, automation, service) = start_peer_delivery_service(directory.path()).await;
    let target = target();
    let push_draft = draft(&target, MessageDelivery::Auto, None);
    let push_id = push_draft.push_id.clone();
    automation
        .lock()
        .await
        .insert_push_record(push_draft)
        .await
        .expect("store the DM before delivery");

    let held = tokio::time::timeout(
        Duration::from_secs(5),
        service.deliver_direct_message(target.clone(), push_id.clone()),
    )
    .await
    .expect("closed peer DM handling is bounded")
    .expect("closed peer DM receipt");
    assert_eq!(held.delivery_state, PushDeliveryState::Held);
    assert!(matches!(
        held.last_outcome.unwrap().outcome,
        DeliveryOutcome::NotSubmitted {
            retryable: true,
            ..
        }
    ));
    assert_eq!(
        automation
            .lock()
            .await
            .list_unsent_direct_messages(&target)
            .await
            .expect("list held DMs")
            .len(),
        1,
        "the same stored DM remains eligible for retry"
    );

    let socket_path = registry.join("peer.sock");
    let listener = tokio::net::UnixListener::bind(&socket_path).expect("peer socket");
    publish_peer(&registry, &socket_path);
    let received = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("peer accepted one write");
        let mut lines = BufReader::new(stream).lines();
        let auth: Value = serde_json::from_str(
            &lines
                .next_line()
                .await
                .expect("auth line")
                .expect("auth frame"),
        )
        .expect("auth JSON");
        let user: Value = serde_json::from_str(
            &lines
                .next_line()
                .await
                .expect("user line")
                .expect("user frame"),
        )
        .expect("user JSON");
        (auth, user)
    });

    let delivered = tokio::time::timeout(
        Duration::from_secs(5),
        service.deliver_direct_message(target.clone(), push_id.clone()),
    )
    .await
    .expect("resumed peer DM handling is bounded")
    .expect("resumed peer DM receipt");
    assert_eq!(delivered.push_id, push_id);
    assert_eq!(delivered.mode, Some(MessageDelivery::Auto));
    assert_eq!(delivered.delivery_state, PushDeliveryState::Delivered);
    assert_eq!(
        delivered.last_outcome.unwrap().outcome,
        DeliveryOutcome::PeerMessageWritten
    );
    let (_auth, user) = tokio::time::timeout(Duration::from_secs(5), received)
        .await
        .expect("peer frame receive is bounded")
        .expect("peer receiver task");
    assert_eq!(user["type"], "user");
    assert!(
        user["message"]["content"]
            .as_str()
            .expect("rendered push line")
            .contains(push_id.as_str())
    );
    assert!(
        automation
            .lock()
            .await
            .list_unsent_direct_messages(&target)
            .await
            .expect("list remaining DMs")
            .is_empty()
    );
    service.shutdown().await;
}

#[tokio::test]
async fn closed_peer_rejects_queue_steer_and_guarded_dms() {
    let directory = tempfile::tempdir().expect("isolated test directory");
    let (_registry, automation, service) = start_peer_delivery_service(directory.path()).await;
    let target = target();
    let cases = [
        (
            MessageDelivery::Queue,
            None,
            DeliveryRejectionReason::QueueUnsupported,
        ),
        (
            MessageDelivery::Steer,
            None,
            DeliveryRejectionReason::NoRoute,
        ),
        (
            MessageDelivery::Auto,
            Some(generation_guard()),
            DeliveryRejectionReason::NoRoute,
        ),
    ];

    for (mode, guard, expected_reason) in cases {
        let push_draft = draft(&target, mode, guard);
        let push_id = push_draft.push_id.clone();
        automation
            .lock()
            .await
            .insert_push_record(push_draft)
            .await
            .expect("store the DM before delivery");
        let rejected = tokio::time::timeout(
            Duration::from_secs(5),
            service.deliver_direct_message(target.clone(), push_id),
        )
        .await
        .expect("restricted peer DM handling is bounded")
        .expect("restricted peer DM receipt");
        assert_eq!(rejected.delivery_state, PushDeliveryState::Rejected);
        let DeliveryOutcome::Rejected(rejection) = rejected.last_outcome.unwrap().outcome else {
            panic!("mode {mode:?} must be rejected");
        };
        assert_eq!(rejection.reason, expected_reason, "mode={mode:?}");
        if mode == MessageDelivery::Queue {
            assert_eq!(
                rejection.detail.as_deref(),
                Some("Queue delivery isn't supported for Claude Code terminals")
            );
        }
    }
    assert!(
        automation
            .lock()
            .await
            .list_unsent_direct_messages(&target)
            .await
            .expect("rejected DMs are not queued")
            .is_empty()
    );
    service.shutdown().await;
}

#[tokio::test]
async fn queue_to_same_endpoint_label_on_another_service_keeps_queue_hold_behavior() {
    let directory = tempfile::tempdir().expect("isolated test directory");
    let (_registry, automation, service) = start_peer_delivery_service(directory.path()).await;
    let mut target = target();
    target.endpoint.service_id =
        UuidIdentity::try_from("00000000-0000-4000-8000-000000000099".to_owned())
            .expect("foreign router service id");
    let push_draft = draft(&target, MessageDelivery::Queue, None);
    let push_id = push_draft.push_id.clone();
    automation
        .lock()
        .await
        .insert_push_record(push_draft)
        .await
        .expect("store the DM before delivery");

    let held = tokio::time::timeout(
        Duration::from_secs(5),
        service.deliver_direct_message(target.clone(), push_id),
    )
    .await
    .expect("foreign endpoint handling is bounded")
    .expect("foreign endpoint receipt");
    assert_eq!(held.mode, Some(MessageDelivery::Queue));
    assert_eq!(held.delivery_state, PushDeliveryState::Held);
    assert!(matches!(
        held.last_outcome.unwrap().outcome,
        DeliveryOutcome::NotSubmitted {
            retryable: true,
            ..
        }
    ));
    service.shutdown().await;
}

//! Provider observers read one ordered Session hub through bounded observation on the API.

use std::sync::Arc;

use collaboration_client::CollaborationClient;
use collaboration_protocol::{
    BoundedObservationRequest, EndpointDescription, ObservationEndReason, ProviderRequestedPolicy,
    ProviderWorkingDirectory, RouterAccess, SessionRef,
};
use collaboration_service::{
    ProviderOperationStore, ProviderSessionEventHub, ProviderSessionRecord, ServiceIdentity,
};
use serde_json::{Value, json};
use session_event_model::{SessionEvent, SessionItem, SessionItemKind};
use tokio::sync::Mutex;
#[path = "support/served_api.rs"]
mod served_api;

#[tokio::test]
async fn provider_observers_read_snapshot_and_live_order() {
    let root = tempfile::tempdir().expect("service root");
    let service_id = "00000000-0000-4000-8000-000000000001";
    let epoch = "00000000-0000-4000-8000-000000000002";
    let target: SessionRef = serde_json::from_value(json!({
        "endpoint":{"serviceId":service_id,"endpointId":"claude-local"},
        "sessionId":"provider-session"
    }))
    .expect("provider target");
    let owner: SessionRef = serde_json::from_value(json!({
        "endpoint":{"serviceId":service_id,"endpointId":"codex-local"},
        "sessionId":"owner"
    }))
    .expect("owner");
    let store = Arc::new(Mutex::new(
        ProviderOperationStore::open(&root.path().join("operations.sqlite"))
            .await
            .expect("store"),
    ));
    store
        .lock()
        .await
        .record_session(&ProviderSessionRecord {
            target: target.clone(),
            working_directory: ProviderWorkingDirectory::try_from(
                root.path().display().to_string(),
            )
            .expect("cwd"),
            requested_policy: ProviderRequestedPolicy {
                access: RouterAccess::WriteRestricted,
            },
            created_by: owner.clone().into(),
            approver: owner.into(),
            updated_at_ms: 1,
        })
        .await
        .expect("session row");
    let hub = Arc::new(ProviderSessionEventHub::with_capacity(store, 2));
    let board_target: message_board::SessionRef =
        serde_json::from_value(serde_json::to_value(&target).expect("target JSON"))
            .expect("board target");
    hub.publish(board_target.clone(), item("item-1", "first"))
        .await
        .expect("first item");
    let description: EndpointDescription = serde_json::from_value(json!({
        "endpoint":target.endpoint,"label":"Claude fixture",
        "availability":{"state":"available","observedAt":"2026-09-26T00:00:00Z"},
        "channels":[{"kind":"externalProvider","transport":"stdioAcp",
            "bindingId":"fixture-binding","bindingGeneration":7,
            "runtime":{"provider":"claudeCode","runtimeName":"fixture"},
            "capabilities":[{"name":"create","status":"supported","evidence":"advertised"}]}]
    }))
    .expect("endpoint");
    let identity = ServiceIdentity::new(service_id, epoch)
        .expect("identity")
        .with_provider_session_hub(hub.clone());
    identity
        .endpoint_directory()
        .publish(description)
        .expect("endpoint publication");

    let served = served_api::ServedApi::start(identity)
        .await
        .expect("serve the API");
    let bounded = served
        .client("provider-observer")
        .await
        .expect("bounded client");
    let (first_snapshot, epoch) = single_event(&bounded, &target, None, None)
        .await
        .expect("first snapshot");
    assert_eq!(first_snapshot["sequence"], 1);

    hub.publish(board_target, item("item-2", "second"))
        .await
        .expect("live item");
    let (first_live, _) = single_event(&bounded, &target, Some(1), epoch)
        .await
        .expect("first live event");
    assert_eq!(first_live["sequence"], 2);

    let observed = bounded
        .observe_provider_session(BoundedObservationRequest {
            target,
            timeout_seconds: 1,
            max_events: 2,
            max_bytes: 262_144,
            after_sequence: None,
            epoch: None,
        })
        .await
        .expect("bounded observation");
    assert_eq!(observed.events, vec![first_snapshot, first_live.clone()]);
    assert_eq!(
        observed.end_reason,
        ObservationEndReason::ResultLimitReached
    );
    assert!(observed.continuation_gap);
    let resumed = bounded
        .observe_provider_session(BoundedObservationRequest {
            target: observed.target.clone(),
            timeout_seconds: 1,
            max_events: 1,
            max_bytes: 262_144,
            after_sequence: Some(1),
            epoch: observed.epoch,
        })
        .await
        .expect("paged observation");
    assert_eq!(resumed.events, vec![first_live.clone()]);
    assert_eq!(resumed.epoch, observed.epoch);
    let stale = bounded
        .observe_provider_session(BoundedObservationRequest {
            target: observed.target.clone(),
            timeout_seconds: 1,
            max_events: 1,
            max_bytes: 262_144,
            after_sequence: Some(1),
            epoch: Some(observed.epoch.expect("provider epoch") + 1),
        })
        .await
        .expect("stale epoch response");
    assert_eq!(stale.end_reason, ObservationEndReason::ResyncRequired);
    assert_eq!(stale.events, vec![json!({"kind":"resyncRequired"})]);
    for index in 3..=5 {
        hub.publish(
            serde_json::from_value(serde_json::to_value(&observed.target).expect("target JSON"))
                .expect("board target"),
            item(&format!("item-{index}"), "overflow subscriber buffer"),
        )
        .await
        .expect("overflow event");
    }
    let large_text = format!("{}{}", "a".repeat(900_000), "\"".repeat(50_000));
    hub.publish(
        serde_json::from_value(serde_json::to_value(&observed.target).expect("target JSON"))
            .expect("board target"),
        item("oversized", &large_text),
    )
    .await
    .expect("large item");
    let oversized_page = bounded
        .observe_provider_session(BoundedObservationRequest {
            target: observed.target.clone(),
            timeout_seconds: 1,
            max_events: 10,
            max_bytes: 1_048_576,
            after_sequence: None,
            epoch: None,
        })
        .await
        .expect("oversized observation");
    assert!(oversized_page.events.iter().any(|event| {
        event == &json!({"kind":"eventTooLarge","sequence":6,"itemId":"oversized"})
    }));
    let replay = bounded
        .observe_provider_session(BoundedObservationRequest {
            target: observed.target.clone(),
            timeout_seconds: 1,
            max_events: 10,
            max_bytes: 1_048_576,
            after_sequence: Some(5),
            epoch: observed.epoch,
        })
        .await
        .expect("replay after the large item");
    assert!(replay.events.iter().any(|event| {
        event == &json!({"kind":"eventTooLarge","sequence":6,"itemId":"oversized"})
    }));
    hub.publish(
        serde_json::from_value(serde_json::to_value(&observed.target).expect("target JSON"))
            .expect("board target"),
        item("after-large", "still delivered"),
    )
    .await
    .expect("later item");
    assert_eq!(
        single_event(&bounded, &observed.target, Some(6), observed.epoch)
            .await
            .expect("later delivery")
            .0["sequence"],
        7
    );
    drop(bounded);
    served.stop().await.expect("API stops");
}

fn item(item_id: &str, text: &str) -> SessionEvent {
    SessionEvent::ItemStarted {
        item: SessionItem {
            item_id: item_id.into(),
            kind: SessionItemKind::AgentMessage,
            text: Some(text.into()),
        },
    }
}

/// The next event after `after_sequence` in `epoch`, waiting up to two seconds for it, with the
/// hub's epoch.
async fn single_event(
    client: &CollaborationClient,
    target: &SessionRef,
    after_sequence: Option<u64>,
    epoch: Option<u64>,
) -> Result<(Value, Option<u64>), Box<dyn std::error::Error>> {
    let observed = client
        .observe_provider_session(BoundedObservationRequest {
            target: target.clone(),
            timeout_seconds: 2,
            max_events: 1,
            max_bytes: 262_144,
            after_sequence,
            epoch,
        })
        .await?;
    let event = observed
        .events
        .into_iter()
        .next()
        .ok_or("no event arrived")?;
    Ok((event, observed.epoch))
}

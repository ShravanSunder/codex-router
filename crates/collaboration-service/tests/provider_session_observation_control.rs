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

/// A Router whose provider endpoint's Session hub has one retained event.
struct ProviderFixture {
    _root: tempfile::TempDir,
    target: SessionRef,
    board_target: message_board::SessionRef,
    hub: Arc<ProviderSessionEventHub>,
    identity: ServiceIdentity,
}

async fn provider_fixture() -> Result<ProviderFixture, Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let service_id = "00000000-0000-4000-8000-000000000001";
    let epoch = "00000000-0000-4000-8000-000000000002";
    let target: SessionRef = serde_json::from_value(json!({
        "endpoint":{"serviceId":service_id,"endpointId":"claude-local"},
        "sessionId":"provider-session"
    }))?;
    let owner: SessionRef = serde_json::from_value(json!({
        "endpoint":{"serviceId":service_id,"endpointId":"codex-local"},
        "sessionId":"owner"
    }))?;
    let store = Arc::new(Mutex::new(
        ProviderOperationStore::open(&root.path().join("operations.sqlite")).await?,
    ));
    store
        .lock()
        .await
        .record_session(&ProviderSessionRecord {
            target: target.clone(),
            working_directory: ProviderWorkingDirectory::try_from(
                root.path().display().to_string(),
            )?,
            requested_policy: ProviderRequestedPolicy {
                access: RouterAccess::WriteRestricted,
            },
            created_by: owner.clone().into(),
            approver: owner.into(),
            updated_at_ms: 1,
        })
        .await?;
    let hub = Arc::new(ProviderSessionEventHub::with_capacity(store, 2));
    let board_target: message_board::SessionRef =
        serde_json::from_value(serde_json::to_value(&target)?)?;
    hub.publish(board_target.clone(), item("item-1", "first"))
        .await?;
    let description: EndpointDescription = serde_json::from_value(json!({
        "endpoint":target.endpoint,"label":"Claude fixture",
        "availability":{"state":"available","observedAt":"2026-09-26T00:00:00Z"},
        "channels":[{"kind":"externalProvider","transport":"stdioAcp",
            "bindingId":"fixture-binding","bindingGeneration":7,
            "runtime":{"provider":"claudeCode","runtimeName":"fixture"},
            "capabilities":[{"name":"create","status":"supported","evidence":"advertised"}]}]
    }))?;
    let identity = ServiceIdentity::new(service_id, epoch)?.with_provider_session_hub(hub.clone());
    identity.endpoint_directory().publish(description)?;
    Ok(ProviderFixture {
        _root: root,
        target,
        board_target,
        hub,
        identity,
    })
}

#[tokio::test]
async fn the_client_hands_on_each_streamed_event_before_the_call_returns() {
    // Arrange: one retained event; the call is bounded at two events and thirty seconds.
    let fixture = provider_fixture().await.expect("provider fixture");
    let served = served_api::ServedApi::start(fixture.identity)
        .await
        .expect("serve the API");
    let client = served
        .client("streaming-observer")
        .await
        .expect("observing client");
    let (observed, mut streamed) = tokio::sync::mpsc::unbounded_channel();
    let request = BoundedObservationRequest {
        target: fixture.target.clone(),
        timeout_seconds: 30,
        max_events: 2,
        max_bytes: 262_144,
        after_sequence: None,
        epoch: None,
    };

    // Act
    let call = tokio::spawn(async move {
        client
            .observe_provider_session_streaming(request, Some(observed))
            .await
    });
    let retained = tokio::time::timeout(std::time::Duration::from_secs(5), streamed.recv())
        .await
        .expect("the retained event streamed while the call was open")
        .expect("streamed event");
    let still_open = !call.is_finished();
    fixture
        .hub
        .publish(fixture.board_target.clone(), item("item-2", "second"))
        .await
        .expect("live item");
    let result = call
        .await
        .expect("call joins")
        .expect("bounded observation");
    let live = streamed.recv().await.expect("live event streamed");

    // Assert
    assert!(
        still_open,
        "the call ended before its first event was handed on"
    );
    assert_eq!(retained.event["sequence"], 1);
    let epoch = result.epoch.expect("provider epoch");
    assert_eq!(
        retained.cursor,
        Some(collaboration_protocol::ObservationCursor {
            epoch,
            after_sequence: 1
        })
    );
    assert_eq!(live.event["sequence"], 2);
    assert_eq!(result.events, vec![retained.event, live.event]);
    assert_eq!(result.end_reason, ObservationEndReason::ResultLimitReached);
    served.stop().await.expect("API stops");
}

#[tokio::test]
async fn a_stale_epoch_streams_its_resync_marker_without_a_cursor() {
    // Arrange: the hub's current epoch, then a call that names an older one.
    let fixture = provider_fixture().await.expect("provider fixture");
    let served = served_api::ServedApi::start(fixture.identity)
        .await
        .expect("serve the API");
    let client = served
        .client("stale-observer")
        .await
        .expect("observing client");
    let (_, epoch) = single_event(&client, &fixture.target, None, None)
        .await
        .expect("current epoch");
    let (observed, mut streamed) = tokio::sync::mpsc::unbounded_channel();

    // Act
    let result = client
        .observe_provider_session_streaming(
            BoundedObservationRequest {
                target: fixture.target.clone(),
                timeout_seconds: 1,
                max_events: 1,
                max_bytes: 262_144,
                after_sequence: Some(1),
                epoch: Some(epoch.expect("provider epoch") + 1),
            },
            Some(observed),
        )
        .await
        .expect("stale epoch response");

    // Assert: the marker was streamed like every other event, with no cursor to continue
    // from; the result is unchanged.
    let marker = streamed.try_recv().expect("the resync marker was streamed");
    assert_eq!(
        marker,
        collaboration_protocol::ObservationEventNotification {
            event: json!({"kind":"resyncRequired"}),
            cursor: None,
        }
    );
    assert_eq!(result.events, vec![json!({"kind":"resyncRequired"})]);
    assert_eq!(result.end_reason, ObservationEndReason::ResyncRequired);
    served.stop().await.expect("API stops");
}

#[tokio::test]
async fn a_stale_epoch_answers_its_resync_marker_even_when_the_budget_cannot_hold_it() {
    // Arrange: a call naming an older epoch, with the smallest valid event and byte budget.
    let fixture = provider_fixture().await.expect("provider fixture");
    let served = served_api::ServedApi::start(fixture.identity)
        .await
        .expect("serve the API");
    let client = served
        .client("small-budget-observer")
        .await
        .expect("observing client");
    let (_, epoch) = single_event(&client, &fixture.target, None, None)
        .await
        .expect("current epoch");
    let (observed, mut streamed) = tokio::sync::mpsc::unbounded_channel();

    // Act
    let result = client
        .observe_provider_session_streaming(
            BoundedObservationRequest {
                target: fixture.target.clone(),
                timeout_seconds: 1,
                max_events: 1,
                max_bytes: 1,
                after_sequence: Some(1),
                epoch: Some(epoch.expect("provider epoch") + 1),
            },
            Some(observed),
        )
        .await
        .expect("stale epoch response");

    // Assert: the marker is the answer itself, so the budget does not apply to it.
    assert_eq!(result.events, vec![json!({"kind":"resyncRequired"})]);
    assert_eq!(result.end_reason, ObservationEndReason::ResyncRequired);
    let marker = streamed.try_recv().expect("the resync marker was streamed");
    assert_eq!(
        marker,
        collaboration_protocol::ObservationEventNotification {
            event: json!({"kind":"resyncRequired"}),
            cursor: None,
        }
    );
    assert!(
        streamed.try_recv().is_err(),
        "one notification carries the marker"
    );
    served.stop().await.expect("API stops");
}

#[tokio::test]
async fn a_reset_during_an_open_call_streams_its_resync_marker_without_a_cursor() {
    // Arrange: an open call that has streamed the retained event.
    let fixture = provider_fixture().await.expect("provider fixture");
    let served = served_api::ServedApi::start(fixture.identity)
        .await
        .expect("serve the API");
    let client = served
        .client("reset-observer")
        .await
        .expect("observing client");
    let (observed, mut streamed) = tokio::sync::mpsc::unbounded_channel();
    let request = BoundedObservationRequest {
        target: fixture.target.clone(),
        timeout_seconds: 30,
        max_events: 10,
        max_bytes: 262_144,
        after_sequence: None,
        epoch: None,
    };
    let call = tokio::spawn(async move {
        client
            .observe_provider_session_streaming(request, Some(observed))
            .await
    });
    let retained = tokio::time::timeout(std::time::Duration::from_secs(5), streamed.recv())
        .await
        .expect("the retained event streamed while the call was open")
        .expect("streamed event");

    // Act: the Session's history is invalidated while the call is open.
    fixture
        .hub
        .begin_history_unavailable(fixture.board_target.clone())
        .await
        .expect("history reset");
    let result = call
        .await
        .expect("call joins")
        .expect("bounded observation");
    let marker = streamed.recv().await.expect("resync marker streamed");

    // Assert: the hub numbered the marker, but nothing continues after it, so it has no
    // cursor; the result still ends with it and reports the resync.
    assert_eq!(retained.event["sequence"], 1);
    assert_eq!(
        marker.event["event"]["kind"], "resyncRequired",
        "{marker:?}"
    );
    assert_eq!(marker.cursor, None, "{marker:?}");
    assert_eq!(result.events.last(), Some(&marker.event));
    assert_eq!(result.end_reason, ObservationEndReason::ResyncRequired);
    served.stop().await.expect("API stops");
}

#[tokio::test]
async fn provider_observers_read_snapshot_and_live_order() {
    let ProviderFixture {
        _root,
        target,
        board_target,
        hub,
        identity,
    } = provider_fixture().await.expect("provider fixture");

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

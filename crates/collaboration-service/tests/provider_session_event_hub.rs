use std::sync::Arc;

use collaboration_protocol::{ProviderRequestedPolicy, ProviderWorkingDirectory, RouterAccess};
use collaboration_service::{
    HubReceiveError, ProviderOperationStore, ProviderSessionEventHub, ProviderSessionRecord,
    SessionEventHub, receive_hub_event,
};
use message_board::SessionRef;
use session_event_model::{
    PendingInteraction, SessionEvent, SessionItem, SessionItemKind, SessionState, StopReason,
    TurnOutcome,
};
use tokio::sync::Mutex;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

macro_rules! ensure {
    ($condition:expr) => {
        if !$condition {
            return Err(format!("assertion failed: {}", stringify!($condition)).into());
        }
    };
}

macro_rules! ensure_eq {
    ($left:expr, $right:expr) => {{
        let left = &$left;
        let right = &$right;
        if left != right {
            return Err(format!(
                "assertion failed: {} == {}: left={left:?}, right={right:?}",
                stringify!($left),
                stringify!($right)
            )
            .into());
        }
    }};
}

fn session() -> TestResult<SessionRef> {
    Ok(serde_json::from_value(serde_json::json!({
        "endpoint":{"serviceId":"0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89","endpointId":"claude-local"},
        "sessionId":"provider-session-1"
    }))?)
}

async fn hub(capacity: usize) -> TestResult<(tempfile::TempDir, ProviderSessionEventHub)> {
    let root = tempfile::tempdir()?;
    let store = ProviderOperationStore::open(&root.path().join("operations.sqlite")).await?;
    Ok((
        root,
        ProviderSessionEventHub::with_capacity(Arc::new(Mutex::new(store)), capacity),
    ))
}

fn user_item(item_id: &str) -> SessionEvent {
    SessionEvent::ItemStarted {
        item: SessionItem {
            item_id: item_id.to_owned(),
            kind: SessionItemKind::UserMessage,
            text: Some(item_id.to_owned()),
        },
    }
}

// R24-R25: two subscribers receive the same ordered live events.
#[tokio::test]
async fn subscribers_receive_identical_sequences() -> TestResult {
    let (_root, hub) = hub(8).await?;
    let target = session()?;
    hub.publish(target.clone(), user_item("one")).await?;
    let mut first = hub.attach(target.clone()).await?;
    let mut second = hub.attach(target.clone()).await?;
    ensure_eq!(first.snapshot, second.snapshot);
    hub.publish(target, user_item("two")).await?;
    ensure_eq!(
        receive_hub_event(&mut first.receiver).await?,
        receive_hub_event(&mut second.receiver).await?
    );
    Ok(())
}

// R25: the snapshot/subscription handoff cannot lose or duplicate an event.
#[tokio::test]
async fn attach_mid_turn_has_no_gap_or_duplicate() -> TestResult {
    let (_root, hub) = hub(8).await?;
    let target = session()?;
    hub.publish(target.clone(), user_item("first")).await?;
    let (attached, published) = tokio::join!(
        hub.attach(target.clone()),
        hub.publish(target, user_item("second"))
    );
    let attached = attached?;
    let second = published?;
    let mut sequence = attached
        .snapshot
        .iter()
        .map(|event| event.sequence)
        .collect::<Vec<_>>();
    let mut receiver = attached.receiver;
    if !attached.snapshot.contains(&second) {
        sequence.push(receive_hub_event(&mut receiver).await?.sequence);
    }
    ensure_eq!(sequence, vec![1, 2]);
    Ok(())
}

// R24: a lagging subscriber must reattach from a snapshot.
#[tokio::test]
async fn lagging_subscriber_gets_resync_required() -> TestResult {
    let (_root, hub) = hub(2).await?;
    let target = session()?;
    hub.publish(target.clone(), user_item("initial")).await?;
    let mut attached = hub.attach(target.clone()).await?;
    for item_id in ["one", "two", "three"] {
        hub.publish(target.clone(), user_item(item_id)).await?;
    }
    ensure_eq!(
        receive_hub_event(&mut attached.receiver).await,
        Err(HubReceiveError::ResyncRequired)
    );
    Ok(())
}

// R26: events accepted from distinct front doors keep one arrival sequence.
#[tokio::test]
async fn front_door_inputs_keep_arrival_order() -> TestResult {
    let (_root, hub) = hub(8).await?;
    let target = session()?;
    let first = hub.publish(target.clone(), user_item("cli-input")).await?;
    let second = hub.publish(target.clone(), user_item("mcp-input")).await?;
    ensure!(first.sequence < second.sequence);
    let attached = hub.attach(target).await?;
    ensure_eq!(attached.snapshot, vec![first, second]);
    Ok(())
}

// R25: pending approval and question requests remain visible on later attach.
#[tokio::test]
async fn late_attach_replays_pending_approval_and_question() -> TestResult {
    let (_root, hub) = hub(8).await?;
    let target = session()?;
    let interactions: [PendingInteraction; 2] = [
        serde_json::from_value(serde_json::json!({
            "kind":"approval", "approver":{"kind":"human","humanId":"owner"}, "request":{
                "requestId":"approval-1", "title":"Run command", "options":[
                    {"optionId":"allow-once","label":"Allow once","choice":{"effect":"allow","scope":"once"}}
                ]
            }
        }))?,
        serde_json::from_value(serde_json::json!({
            "kind":"question", "approver":{"kind":"human","humanId":"owner"}, "request":{
                "requestId":"question-1", "prompt":"Choose a count", "fields":[
                    {"kind":"number","fieldId":"count","label":"Count","required":true}
                ]
            }
        }))?,
    ];
    for interaction in interactions {
        hub.publish(
            target.clone(),
            SessionEvent::InteractionRequested { interaction },
        )
        .await?;
    }
    let attached = hub.attach(target.clone()).await?;
    ensure_eq!(attached.snapshot.len(), 2);
    let approval = serde_json::to_value(&attached.snapshot[0].event)?;
    let question = serde_json::to_value(&attached.snapshot[1].event)?;
    ensure_eq!(approval["interaction"]["request"]["title"], "Run command");
    ensure_eq!(
        approval["interaction"]["request"]["options"][0]["optionId"],
        "allow-once"
    );
    ensure_eq!(
        question["interaction"]["request"]["prompt"],
        "Choose a count"
    );
    ensure_eq!(
        question["interaction"]["request"]["fields"][0]["label"],
        "Count"
    );
    ensure!(matches!(
        hub.state(target).await?,
        SessionState::RequiresAction { pending } if pending.iter().count() == 2
    ));
    Ok(())
}

#[tokio::test]
async fn sessions_use_durable_inventory_with_live_state_overlay() -> TestResult {
    let root = tempfile::tempdir()?;
    let mut store = ProviderOperationStore::open(&root.path().join("operations.sqlite")).await?;
    let target = session()?;
    let stored_target: collaboration_protocol::SessionRef =
        serde_json::from_value(serde_json::to_value(&target)?)?;
    store
        .record_session(&ProviderSessionRecord {
            target: stored_target.clone(),
            working_directory: ProviderWorkingDirectory::try_from(
                root.path().display().to_string(),
            )?,
            requested_policy: ProviderRequestedPolicy {
                access: RouterAccess::WriteRestricted,
            },
            created_by: stored_target.clone(),
            approver: stored_target,
            updated_at_ms: 3_000,
        })
        .await?;
    let hub = ProviderSessionEventHub::new(Arc::new(Mutex::new(store)));
    let cold = hub.sessions(target.endpoint.clone()).await?;
    ensure_eq!(cold.len(), 1);
    ensure_eq!(cold[0].updated_at_seconds, 3);
    ensure_eq!(cold[0].state, SessionState::Unloaded);
    ensure_eq!(cold[0].preview, "");
    ensure!(cold[0].name.is_none() && cold[0].model.is_none());
    let mut cold_attachment = hub.attach(target.clone()).await?;
    ensure!(cold_attachment.snapshot.is_empty());
    ensure_eq!(hub.state(target.clone()).await?, SessionState::Unloaded);

    hub.publish(
        target.clone(),
        SessionEvent::TurnStarted {
            turn_id: "turn-1".into(),
            input_id: "input-1".into(),
        },
    )
    .await?;
    ensure_eq!(
        receive_hub_event(&mut cold_attachment.receiver)
            .await?
            .sequence,
        1
    );
    let live = hub.sessions(target.endpoint).await?;
    ensure_eq!(live[0].state, SessionState::Running);
    Ok(())
}

#[tokio::test]
async fn replay_reset_replaces_lost_history_and_resyncs_old_subscribers() -> TestResult {
    let (_root, hub) = hub(8).await?;
    let target = session()?;
    hub.publish(
        target.clone(),
        SessionEvent::TurnStarted {
            turn_id: "old-turn".into(),
            input_id: "old-input".into(),
        },
    )
    .await?;
    let approval: PendingInteraction = serde_json::from_value(serde_json::json!({
        "kind":"approval", "approver":{"kind":"human","humanId":"owner"}, "request":{
            "requestId":"old-approval", "title":"Old approval", "options":[
                {"optionId":"allow-once","label":"Allow once","choice":{"effect":"allow","scope":"once"}}
            ]
        }
    }))?;
    hub.publish(
        target.clone(),
        SessionEvent::InteractionRequested {
            interaction: approval,
        },
    )
    .await?;
    hub.publish(
        target.clone(),
        SessionEvent::InteractionResolved {
            request_id: "old-approval".into(),
        },
    )
    .await?;
    hub.publish(
        target.clone(),
        SessionEvent::TurnEnded {
            turn_id: "old-turn".into(),
            outcome: TurnOutcome::Lost {
                reason: "providerRetired".into(),
            },
        },
    )
    .await?;
    let mut stale = hub.attach(target.clone()).await?;

    ensure_eq!(hub.begin_history_replay(target.clone()).await?, 1);
    ensure_eq!(
        receive_hub_event(&mut stale.receiver).await,
        Err(HubReceiveError::ResyncRequired)
    );
    hub.publish(
        target.clone(),
        SessionEvent::TurnStarted {
            turn_id: "historical-turn".into(),
            input_id: "replayed-input".into(),
        },
    )
    .await?;
    hub.publish(target.clone(), user_item("replayed-message"))
        .await?;
    hub.publish(
        target.clone(),
        SessionEvent::TurnEnded {
            turn_id: "historical-turn".into(),
            outcome: TurnOutcome::Ended {
                stop_reason: StopReason::Unknown("replayed".into()),
                local_cause: None,
            },
        },
    )
    .await?;
    let attached = hub.attach(target).await?;
    ensure_eq!(attached.snapshot.len(), 3);
    ensure!(
        attached
            .snapshot
            .iter()
            .all(|item| { !matches!(&item.event, SessionEvent::InteractionRequested { .. }) })
    );
    Ok(())
}

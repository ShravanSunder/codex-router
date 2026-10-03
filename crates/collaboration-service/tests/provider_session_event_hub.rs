use std::sync::Arc;

use collaboration_protocol::{ProviderRequestedPolicy, ProviderWorkingDirectory, RouterAccess};
use collaboration_service::{
    HubReceiveError, ProviderOperationStore, ProviderSessionEventHub, ProviderSessionRecord,
    SessionEventHub, receive_hub_event,
};
use message_board::SessionRef;
use session_event_model::{
    ConfigValue, ConfigValueState, PendingInteraction, SessionEvent, SessionItem, SessionItemKind,
    SessionSettings, SessionState, StopReason, TurnLostReason, TurnOutcome,
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

fn updated_item(item_id: &str, text: String) -> SessionEvent {
    SessionEvent::ItemUpdated {
        item: SessionItem {
            item_id: item_id.to_owned(),
            kind: SessionItemKind::AgentMessage,
            text: Some(text),
        },
    }
}

#[tokio::test]
async fn cumulative_updates_retain_one_latest_item_and_order_for_two_observers() -> TestResult {
    let (_root, hub) = hub(1_024).await?;
    let target = session()?;
    hub.publish(target.clone(), updated_item("message", String::new()))
        .await?;
    let mut first = hub.attach(target.clone()).await?;
    let mut second = hub.attach(target.clone()).await?;
    let mut text = String::new();
    for _ in 0..1_000 {
        text.push('x');
        hub.publish(target.clone(), updated_item("message", text.clone()))
            .await?;
    }
    for _ in 0..1_000 {
        ensure_eq!(
            receive_hub_event(&mut first.receiver).await?,
            receive_hub_event(&mut second.receiver).await?
        );
    }
    let first_late = hub.attach(target.clone()).await?;
    let second_late = hub.attach(target).await?;
    ensure_eq!(first_late.snapshot, second_late.snapshot);
    ensure_eq!(first_late.snapshot.len(), 1);
    ensure_eq!(first_late.snapshot[0].sequence, 1_001);
    let retained_bytes = first_late
        .snapshot
        .iter()
        .map(|event| serde_json::to_vec(&event.event).map(|encoded| encoded.len()))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .sum::<usize>();
    ensure!(retained_bytes < 2 * text.len());
    hub.publish(
        session()?,
        SessionEvent::ItemCompleted {
            item_id: "message".into(),
        },
    )
    .await?;
    let completed = hub.attach(session()?).await?;
    ensure_eq!(completed.snapshot.len(), 2);
    ensure_eq!(completed.snapshot[0].sequence, 1_001);
    ensure_eq!(completed.snapshot[1].sequence, 1_002);
    ensure!(matches!(
        &completed.snapshot[1].event,
        SessionEvent::ItemCompleted { item_id } if item_id == "message"
    ));
    Ok(())
}

#[tokio::test]
async fn unloaded_session_evacuates_accumulated_history() -> TestResult {
    let (_root, hub) = hub(8).await?;
    let target = session()?;
    hub.publish(target.clone(), user_item("before-close"))
        .await?;
    hub.publish(
        target.clone(),
        SessionEvent::StateChanged {
            state: SessionState::Unloaded,
        },
    )
    .await?;
    ensure!(hub.attach(target).await?.snapshot.is_empty());
    Ok(())
}

#[tokio::test]
async fn closed_session_evacuates_history_and_remains_closed() -> TestResult {
    let (_root, hub) = hub(8).await?;
    let target = session()?;
    hub.publish(target.clone(), user_item("before-close"))
        .await?;
    let mut attached = hub.attach(target.clone()).await?;
    hub.publish(
        target.clone(),
        SessionEvent::StateChanged {
            state: SessionState::Closed,
        },
    )
    .await?;
    ensure_eq!(hub.state(target.clone()).await?, SessionState::Closed);
    ensure!(hub.attach(target).await?.snapshot.is_empty());
    ensure_eq!(
        receive_hub_event(&mut attached.receiver).await?.event,
        SessionEvent::StateChanged {
            state: SessionState::Closed
        }
    );
    ensure_eq!(
        receive_hub_event(&mut attached.receiver).await,
        Err(HubReceiveError::ResyncRequired)
    );
    Ok(())
}

#[tokio::test]
async fn projection_rejection_resyncs_only_that_session() -> TestResult {
    let (_root, hub) = hub(8).await?;
    let affected = session()?;
    let other: SessionRef = serde_json::from_value(serde_json::json!({
        "endpoint":{"serviceId":"0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89","endpointId":"claude-local"},
        "sessionId":"provider-session-2"
    }))?;
    let approval: PendingInteraction = serde_json::from_value(serde_json::json!({
        "kind":"approval", "approver":{"kind":"human","humanId":"owner"}, "request":{
            "requestId":"approval-1", "title":"Run command", "options":[
                {"optionId":"allow-once","label":"Allow once","choice":{"effect":"allow","scope":"once"}}
            ]
        }
    }))?;
    hub.publish(
        affected.clone(),
        SessionEvent::InteractionRequested {
            interaction: approval,
        },
    )
    .await?;
    let mut stale = hub.attach(affected.clone()).await?;
    let invalid = hub
        .publish(
            affected.clone(),
            SessionEvent::TurnEnded {
                turn_id: "ended-with-pending".into(),
                outcome: TurnOutcome::Lost {
                    reason: TurnLostReason::ProviderRetired,
                },
            },
        )
        .await?;
    ensure!(matches!(invalid.event, SessionEvent::ResyncRequired { .. }));
    ensure_eq!(
        receive_hub_event(&mut stale.receiver).await,
        Err(HubReceiveError::ResyncRequired)
    );
    ensure_eq!(hub.state(affected.clone()).await?, SessionState::Unloaded);
    ensure!(hub.attach(affected).await?.snapshot.is_empty());
    let published = hub
        .publish(other.clone(), user_item("other-session"))
        .await?;
    ensure_eq!(hub.attach(other).await?.snapshot, vec![published]);
    Ok(())
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

#[tokio::test]
async fn unobservable_steer_end_returns_the_live_session_to_idle() -> TestResult {
    let (_root, hub) = hub(8).await?;
    let target = session()?;
    hub.publish(
        target.clone(),
        SessionEvent::TurnStarted {
            turn_id: "steer-turn".into(),
            input_id: session_event_model::InputId::new("steer-input")?,
        },
    )
    .await?;
    hub.publish(
        target.clone(),
        SessionEvent::TurnEnded {
            turn_id: "steer-turn".into(),
            outcome: TurnOutcome::Lost {
                reason: TurnLostReason::EndNotObservable,
            },
        },
    )
    .await?;
    ensure_eq!(hub.state(target).await?, SessionState::Idle);
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
            created_by: stored_target.clone().into(),
            approver: stored_target.into(),
            updated_at_ms: 3_000,
        })
        .await?;
    let hub = ProviderSessionEventHub::new(Arc::new(Mutex::new(store)));
    let cold = hub.sessions(target.endpoint.clone()).await?;
    ensure_eq!(cold.len(), 1);
    ensure_eq!(cold[0].updated_at_seconds, 3);
    ensure_eq!(
        cold[0].approver,
        message_board::Identity::Session {
            session: target.clone()
        }
    );
    ensure_eq!(cold[0].state, SessionState::Unloaded);
    ensure_eq!(cold[0].preview, "");
    ensure!(cold[0].name.is_none() && cold[0].model.is_none() && cold[0].mode.is_none());
    let mut cold_attachment = hub.attach(target.clone()).await?;
    ensure!(cold_attachment.snapshot.is_empty());
    ensure_eq!(hub.state(target.clone()).await?, SessionState::Unloaded);

    hub.publish(
        target.clone(),
        SessionEvent::SettingsChanged {
            settings: SessionSettings {
                mode: Some("default".into()),
                model: Some("model-a".into()),
                effort: None,
                config: vec![ConfigValue {
                    id: "thinking".into(),
                    value: ConfigValueState::Boolean(false),
                }],
            },
        },
    )
    .await?;
    let created = hub.sessions(target.endpoint.clone()).await?;
    ensure_eq!(created[0].model.as_deref(), Some("model-a"));
    ensure_eq!(created[0].mode.as_deref(), Some("default"));
    hub.publish(
        target.clone(),
        SessionEvent::SettingsChanged {
            settings: SessionSettings {
                mode: Some("ask".into()),
                model: Some("model-b".into()),
                effort: Some("high".into()),
                config: vec![ConfigValue {
                    id: "thinking".into(),
                    value: ConfigValueState::Boolean(true),
                }],
            },
        },
    )
    .await?;
    let updated = hub.sessions(target.endpoint.clone()).await?;
    ensure_eq!(updated[0].model.as_deref(), Some("model-b"));
    ensure_eq!(updated[0].mode.as_deref(), Some("ask"));

    hub.publish(
        target.clone(),
        SessionEvent::TurnStarted {
            turn_id: "turn-1".into(),
            input_id: session_event_model::InputId::new("input-1").expect("input ID"),
        },
    )
    .await?;
    let mut observed = Vec::new();
    for _ in 0..3 {
        observed.push(
            receive_hub_event(&mut cold_attachment.receiver)
                .await?
                .sequence,
        );
    }
    ensure_eq!(observed, vec![1, 2, 3]);
    let live = hub.sessions(target.endpoint.clone()).await?;
    ensure_eq!(live[0].state, SessionState::Running);
    ensure_eq!(
        live[0].approver,
        message_board::Identity::Session {
            session: target.clone()
        }
    );
    hub.begin_history_unavailable(target.clone()).await?;
    let resumed = hub.sessions(target.endpoint.clone()).await?;
    ensure_eq!(resumed[0].model.as_deref(), Some("model-b"));
    ensure_eq!(resumed[0].mode.as_deref(), Some("ask"));
    drop(hub);
    let restarted_store =
        ProviderOperationStore::open(&root.path().join("operations.sqlite")).await?;
    let restarted_hub = ProviderSessionEventHub::new(Arc::new(Mutex::new(restarted_store)));
    let restarted = restarted_hub
        .sessions(live[0].session.endpoint.clone())
        .await?;
    ensure_eq!(restarted.len(), 1);
    ensure_eq!(restarted[0].state, SessionState::Unloaded);
    ensure!(restarted[0].model.is_none() && restarted[0].mode.is_none());
    ensure_eq!(restarted[0].approver, live[0].approver);
    Ok(())
}

#[tokio::test]
async fn sessions_inventory_preserves_human_approver_after_restart() -> TestResult {
    let root = tempfile::tempdir()?;
    let path = root.path().join("operations.sqlite");
    let mut store = ProviderOperationStore::open(&path).await?;
    let target = session()?;
    let stored_target: collaboration_protocol::SessionRef =
        serde_json::from_value(serde_json::to_value(&target)?)?;
    let human: collaboration_protocol::ProviderIdentity =
        serde_json::from_value(serde_json::json!({"humanId":"owner"}))?;
    store
        .record_session(&ProviderSessionRecord {
            target: stored_target,
            working_directory: ProviderWorkingDirectory::try_from(
                root.path().display().to_string(),
            )?,
            requested_policy: ProviderRequestedPolicy {
                access: RouterAccess::WriteRestricted,
            },
            created_by: human.clone(),
            approver: human,
            updated_at_ms: 3_000,
        })
        .await?;
    store.close().await?;
    let reopened = ProviderOperationStore::open(&path).await?;
    let hub = ProviderSessionEventHub::new(Arc::new(Mutex::new(reopened)));
    let rows = hub.sessions(target.endpoint).await?;
    ensure_eq!(rows.len(), 1);
    ensure!(matches!(&rows[0].approver,
        message_board::Identity::Human { human_id } if human_id.as_str() == "owner"));
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
            input_id: session_event_model::InputId::new("old-input").expect("input ID"),
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
                reason: TurnLostReason::ProviderRetired,
            },
        },
    )
    .await?;
    let mut stale = hub.attach(target.clone()).await?;

    ensure_eq!(hub.begin_history_replay(target.clone()).await?, 2);
    ensure_eq!(
        receive_hub_event(&mut stale.receiver).await,
        Err(HubReceiveError::ResyncRequired)
    );
    hub.publish(
        target.clone(),
        SessionEvent::TurnStarted {
            turn_id: "historical-turn".into(),
            input_id: session_event_model::InputId::new("replayed-input").expect("input ID"),
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

#[tokio::test]
async fn resume_without_replay_invalidates_old_history_and_subscribers() -> TestResult {
    let (_root, hub) = hub(8).await?;
    let target = session()?;
    hub.publish(target.clone(), user_item("old-message"))
        .await?;
    let mut stale = hub.attach(target.clone()).await?;

    ensure_eq!(hub.begin_history_unavailable(target.clone()).await?, 1);
    ensure_eq!(
        receive_hub_event(&mut stale.receiver).await,
        Err(HubReceiveError::ResyncRequired)
    );
    let current = hub.attach(target.clone()).await?;
    ensure!(current.snapshot.is_empty());
    ensure_eq!(hub.state(target).await?, SessionState::Idle);
    Ok(())
}

//! Real Codex app-server proofs for held DM and subscription push cells.
use super::{
    OBSERVATION_TIMEOUT, ProofContext, ProofResult, SUBSCRIPTION_NOTICE_LABEL, dm_notice_and_show,
    matrix_marker,
};
use codex_native_integration::NativeOperation;
use collaboration_client::board::{Identity, MessageId, SubscriptionScope};
use collaboration_client::protocol::{
    RouterAccess, SessionRef, ThreadSubscriptionPresence, ThreadSubscriptionsRequest,
};
use collaboration_client::{
    ConversationClient, ConversationLoadInput, ConversationOperationResult,
};
use collaboration_protocol::{PushDeliveryState, PushKind};
use serde_json::{Value, json};
use std::time::Duration;

pub(super) struct UnloadedMatrixTargets {
    pub(super) held_dm: SessionRef,
    pub(super) held_subscription: SessionRef,
    pub(super) wake: SessionRef,
}

pub(super) async fn prepare_unloaded_targets(
    proof: &mut ProofContext,
) -> ProofResult<UnloadedMatrixTargets> {
    let targets = UnloadedMatrixTargets {
        held_dm: prepare_materialized_target(proof, "S3 held DM recipient").await?,
        held_subscription: prepare_materialized_target(proof, "S5 held batch recipient").await?,
        wake: prepare_materialized_target(proof, "S6 unloaded wake recipient").await?,
    };
    super::debug_backend_restart::restart(proof).await?;
    for target in [&targets.held_dm, &targets.held_subscription, &targets.wake] {
        require_unloaded_target(proof, target).await?;
    }
    proof.record(
        "matrixUnloadedTargetsPrepared",
        json!({
            "heldDm":targets.held_dm,"heldSubscription":targets.held_subscription,
            "wake":targets.wake,"generation":proof.generation,
        }),
    )?;
    Ok(targets)
}

async fn prepare_materialized_target(
    proof: &mut ProofContext,
    role: &str,
) -> ProofResult<SessionRef> {
    let target = proof.start_thread(role).await?;
    let marker = matrix_marker("unloaded-preparation");
    let task = format!("Output exactly {marker}. Do not call tools or spawn agents.");
    let response = proof
        .native
        .request_validated(
            &proof.schemas,
            NativeOperation::StartTurn,
            json!({"threadId":String::from(target.session_id.clone()),
            "input":[{"type":"text","text":task,"text_elements":[]}]}),
        )
        .await?;
    let turn_id = response
        .pointer("/turn/id")
        .and_then(Value::as_str)
        .ok_or("Unloaded preparation did not accept a native turn")?
        .to_owned();
    let deadline = tokio::time::Instant::now() + OBSERVATION_TIMEOUT;
    let mut observation = tokio::time::interval(Duration::from_millis(250));
    loop {
        observation.tick().await;
        let turns = proof.turns(&target).await?;
        if let Some(turn) = turns
            .iter()
            .find(|turn| turn.get("id").and_then(Value::as_str) == Some(turn_id.as_str()))
        {
            match turn.get("status").and_then(Value::as_str) {
                Some("completed") => {
                    let input_count = turn
                        .get("items")
                        .and_then(Value::as_array)
                        .ok_or("Completed preparation turn omitted its history")?
                        .iter()
                        .filter(|item| {
                            super::user_message_text(item).is_ok_and(|text| text == task)
                        })
                        .count();
                    if input_count != 1 || !super::proof_context::agent_text(turn).contains(&marker)
                    {
                        return Err("Preparation completed without the exact native input and requested output".into());
                    }
                    proof.record("matrixTargetMaterialized", json!({"role":role,"target":target,"turnId":turn_id,"input":task,"status":"completed"}))?;
                    return Ok(target);
                }
                Some("failed" | "interrupted") => {
                    proof.record(
                        "matrixTargetPreparationFailed",
                        json!({"role":role,"target":target,"turn":turn}),
                    )?;
                    return Err(format!("{role} preparation did not complete; model/auth/runtime failure is a failed cell").into());
                }
                _ => {}
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(format!(
                "{role} preparation did not complete within its deadline; cell is unverified"
            )
            .into());
        }
    }
}

async fn require_unloaded_target(proof: &mut ProofContext, target: &SessionRef) -> ProofResult<()> {
    let response = proof
        .native
        .request_validated(
            &proof.schemas,
            NativeOperation::ListLoadedThreads,
            json!({}),
        )
        .await?;
    if response
        .get("nextCursor")
        .is_some_and(|cursor| !cursor.is_null())
    {
        return Err("Unexpected pagination in private loaded-thread inventory".into());
    }
    let loaded = response
        .get("data")
        .and_then(Value::as_array)
        .ok_or("Private loaded-thread inventory omitted its records")?;
    let target_id = String::from(target.session_id.clone());
    if loaded
        .iter()
        .any(|id| id.as_str() == Some(target_id.as_str()))
    {
        return Err(
            format!("Saved target {target_id} is still loaded; held cell not attempted").into(),
        );
    }
    Ok(())
}

pub(super) async fn cover_held_dm_cell(
    proof: &mut ProofContext,
    sender: &SessionRef,
    target: &SessionRef,
) -> ProofResult<()> {
    require_unloaded_target(proof, target).await?;
    let marker = matrix_marker("held-dm");
    let body = format!("{marker} delivered after conversation load");
    let receipt = super::cli_send(proof, Some(sender), target, &body, false).await?;
    let push_id = super::receipt_push_id(&receipt)?;
    if receipt.get("deliveryState") != Some(&json!("held")) {
        return Err(format!("S3 send was not held: {receipt}").into());
    }

    let (held, held_line) = dm_notice_and_show(proof, target, &push_id).await?;
    if held.record.delivery_state != PushDeliveryState::Held {
        return Err("S3 show did not preserve the held DM state".into());
    }

    load_codex_target(proof, sender, target).await?;
    let item = super::wait_for_codex_input(proof, target, &marker, OBSERVATION_TIMEOUT).await?;
    let observed_line = super::user_message_text(&item)?;
    super::assert_exact_notice_line(&observed_line, &held_line)?;
    let (delivered, delivered_line) =
        super::dm_history_notice_and_show(proof, sender, target, &push_id).await?;
    if delivered.record.delivery_state != PushDeliveryState::Delivered
        || delivered.record.push_id.as_str() != push_id
        || delivered_line != held_line
    {
        return Err("S3 load did not deliver the original stored DM line".into());
    }
    proof.record(
        "pushDeliveryMatrixS3",
        json!({"target":target,"pushId":push_id,"link":delivered.link,"deliveryState":"delivered","line":observed_line}),
    )?;
    Ok(())
}

pub(super) async fn cover_held_subscription_batch_cell(
    proof: &mut ProofContext,
    sender: &SessionRef,
    target: &SessionRef,
) -> ProofResult<()> {
    require_unloaded_target(proof, target).await?;
    let first_marker = matrix_marker("held-subscription-first-root");
    let second_marker = matrix_marker("held-subscription-second-root");
    let roots = super::held_subscription_producer::board_two_thread_subscription_push(
        proof,
        sender,
        target,
        [&first_marker, &second_marker],
    )
    .await?;

    wait_for_held_subscription_roots(proof, target, &roots).await?;
    require_unloaded_target(proof, target).await?;
    load_codex_target(proof, sender, target).await?;
    let item = super::wait_for_codex_input(
        proof,
        target,
        SUBSCRIPTION_NOTICE_LABEL,
        OBSERVATION_TIMEOUT,
    )
    .await?;
    let observed = super::user_message_text(&item)?;
    let delivered_show =
        super::subscription::verify_subscription_notice(proof, target, &observed, &first_marker)
            .await?;
    let expected_line = super::expected_push_line(&delivered_show, &super::machine_label(proof)?)?;
    super::assert_exact_notice_line(&observed, &expected_line)?;
    assert_held_two_root_batch(
        &delivered_show,
        &observed,
        &expected_line,
        &first_marker,
        &second_marker,
    )?;
    require_one_subscription_input(proof, target, &observed).await?;
    proof.record(
        "pushDeliveryMatrixS5",
        json!({"target":target,"pushId":delivered_show.record.push_id,"link":delivered_show.link,"deliveryState":"delivered","rangeCount":delivered_show.activity_ranges.len(),"line":observed}),
    )?;
    Ok(())
}

async fn load_codex_target(
    proof: &ProofContext,
    requested_by: &SessionRef,
    target: &SessionRef,
) -> ProofResult<()> {
    let conversation =
        ConversationClient::connect(&proof.service_directory, &target.endpoint).await?;
    let result = conversation
        .load(
            ConversationLoadInput {
                operation_id: None,
                target: target.clone(),
                working_directory: proof.workspace.clone(),
                requested_by: requested_by.clone(),
                approver: Some(requested_by.clone()),
                access: RouterAccess::WorkspaceWrite,
                generation: None,
            },
            Duration::from_secs(90),
        )
        .await?;
    match result {
        ConversationOperationResult::Completed { target: loaded, .. } if loaded == *target => {
            Ok(())
        }
        other => Err(format!("Codex load returned an unexpected result: {other:?}").into()),
    }
}

async fn wait_for_held_subscription_roots(
    proof: &mut ProofContext,
    target: &SessionRef,
    roots: &[MessageId],
) -> ProofResult<()> {
    if roots.len() != 2 || roots.first() == roots.last() {
        return Err("S5 requires two distinct subscription roots".into());
    }
    let reader: Identity = serde_json::from_value(json!({"kind":"session","session":target}))?;
    let deadline = tokio::time::Instant::now() + OBSERVATION_TIMEOUT;
    let mut observation = tokio::time::interval(Duration::from_millis(250));
    loop {
        observation.tick().await;
        let page = proof
            .client
            .board_thread_subscriptions(ThreadSubscriptionsRequest {
                actor: reader.clone(),
            })
            .await?;
        let held = roots.iter().all(|root| {
            page.subscriptions.iter().any(|record| {
                record.scope == SubscriptionScope::thread(root.clone())
                    && matches!(record.presence, ThreadSubscriptionPresence::Wakeable {})
                    && record.pending_count > 0
                    && record.held_since.is_some()
            })
        });
        if held {
            proof.record(
                "matrixS5RootsHeld",
                json!({"target":target,"roots":roots,"subscriptions":page.subscriptions}),
            )?;
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(
                "S5 did not observe both exact roots Wakeable, pending and held before load".into(),
            );
        }
    }
}

async fn require_one_subscription_input(
    proof: &mut ProofContext,
    target: &SessionRef,
    expected_line: &str,
) -> ProofResult<()> {
    let turns = proof.turns(target).await?;
    let notices = turns
        .iter()
        .filter_map(|turn| turn.get("items").and_then(Value::as_array))
        .flatten()
        .filter_map(|item| super::user_message_text(item).ok())
        .filter(|text| text.starts_with(SUBSCRIPTION_NOTICE_LABEL))
        .collect::<Vec<_>>();
    if notices.len() != 1 || notices.first().is_none_or(|line| line != expected_line) {
        return Err(format!(
            "S5 native history must contain exactly one subscription notice: {notices:?}"
        )
        .into());
    }
    Ok(())
}

fn assert_held_two_root_batch(
    show: &collaboration_protocol::PushRecordShowResult,
    observed_line: &str,
    expected_line: &str,
    first_marker: &str,
    second_marker: &str,
) -> ProofResult<()> {
    let collaboration_protocol::PushHeaderFacts::SubscriptionActivity {
        root_count,
        held_since,
        ..
    } = &show.record.header_facts
    else {
        return Err("S5 held push did not have subscription header facts".into());
    };
    let activity = show
        .record
        .activity
        .as_ref()
        .ok_or("S5 held push omitted its activity snapshot")?;
    let has_marker = |marker: &str| {
        show.activity_ranges.iter().any(|range| {
            range
                .messages
                .iter()
                .any(|message| message.text.as_str() == marker)
        })
    };
    if show.record.kind != PushKind::SubscriptionActivity
        || show.record.delivery_state != PushDeliveryState::Delivered
        || *root_count != 2
        || held_since.is_none()
        || !activity.held
        || activity.ranges.len() != 2
        || show.activity_ranges.len() != 2
        || !has_marker(first_marker)
        || !has_marker(second_marker)
        || observed_line != expected_line
        || !observed_line.contains(" · 2 threads · ")
        || !observed_line.contains(" · held since ")
    {
        return Err(format!(
            "S5 was not one held two-root batch: state={:?}, roots={}, held_since={:?}, ranges={}, line={observed_line:?}",
            show.record.delivery_state,
            root_count,
            held_since,
            show.activity_ranges.len(),
        )
        .into());
    }
    Ok(())
}

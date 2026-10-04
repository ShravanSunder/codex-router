#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic_in_result_fn
)]
//! Recipient-observed push-format matrix on an isolated debug Host.
#[path = "automation_live_support/debug_backend_restart.rs"]
mod debug_backend_restart;
#[path = "delivery_matrix/support.rs"]
#[allow(dead_code)]
mod delivery_matrix_support;
#[path = "delivery_matrix/held_cells.rs"]
mod held_cells;
#[path = "delivery_matrix/held_subscription_producer.rs"]
mod held_subscription_producer;
#[path = "automation_live_support/proof_context.rs"]
#[allow(dead_code)]
mod proof_context;
#[path = "delivery_matrix/subscription_notice_observer.rs"]
mod subscription;

use automation_storage::AutomationStore;
use chrono::{Duration as ChronoDuration, Utc};
use collaboration_protocol::{
    FireReceipt, PushHeaderFacts, PushId, PushKind, PushOrigin, PushRecordDraft,
    PushRecordListParams, PushRecordShowParams, RouterOriginRef, SessionRef,
};
#[path = "delivery_matrix/push_matrix_driver.rs"]
mod push_matrix_driver;
use proof_context::{ProofContext, ProofResult};
use push_matrix_driver::*;
use serde_json::json;
use std::time::Duration;

const OBSERVATION_TIMEOUT: Duration = Duration::from_secs(120);
const ACP_TARGET_EXPECTED_PROMPTS: usize = 9;
const SUBSCRIPTION_NOTICE_LABEL: &str = "🧵 Router: new thread activity";

fn expected_long_preview_literal(body: &str) -> ProofResult<String> {
    let total_scalars = body.chars().count();
    let source_prefix = body.chars().take(100).collect::<String>();
    let omitted_scalars = total_scalars.saturating_sub(source_prefix.chars().count());
    if omitted_scalars == 0 {
        return Err("long-preview proof requires an omitted source scalar".into());
    }
    Ok(format!(
        "\"{source_prefix}…\" (+{omitted_scalars} more chars)"
    ))
}

fn assert_long_preview_literal(observed: &str, body: &str) -> ProofResult<()> {
    let expected_preview = expected_long_preview_literal(body)?;
    if !observed.contains(&expected_preview) {
        return Err(format!(
            "observed output omitted the source-derived preview literal {expected_preview:?}: {observed:?}"
        )
        .into());
    }
    Ok(())
}

async fn assert_inbox_notice_has_literal_long_preview(
    proof: &mut ProofContext,
    recipient: &SessionRef,
    push_id: &str,
    body: &str,
) -> ProofResult<()> {
    let inbox = proof
        .client
        .message_inbox(PushRecordListParams {
            caller: recipient.clone(),
            limit: 100,
        })
        .await?;
    let notice = inbox
        .records
        .iter()
        .find(|notice| notice.push_id.as_str() == push_id)
        .ok_or_else(|| format!("push {push_id} missing from recipient inbox"))?;
    assert_long_preview_literal(&notice.line, body)?;
    if !notice.line.ends_with(&notice.link) {
        return Err("recipient inbox notice did not retain its complete link".into());
    }
    Ok(())
}

#[test]
#[ignore = "creates an isolated debug Host root and scripted ACP provider fixture"]
fn prepare_delivery_matrix_provider_fixture() -> ProofResult<()> {
    delivery_matrix_support::prepare_provider_fixture()
}

#[test]
#[ignore = "creates a fresh isolated Host root with two scripted ACP provider runtimes"]
fn prepare_delivery_matrix_acp_target_fixture() -> ProofResult<()> {
    delivery_matrix_support::prepare_acp_target_fixture()
}

#[tokio::test]
#[ignore = "requires an isolated debug Host with the delivery-matrix provider fixture"]
async fn push_delivery_matrix_covers_codex_and_claude_peer() -> ProofResult<()> {
    let config_guard = delivery_matrix_support::ConfigHashGuard::capture()?;
    let result = exercise_push_delivery_matrix(&config_guard).await;
    config_guard.verify()?;
    result
}

async fn exercise_push_delivery_matrix(
    config_guard: &delivery_matrix_support::ConfigHashGuard,
) -> ProofResult<()> {
    let mut proof = ProofContext::connect().await?;
    proof.record(
        "pushDeliveryMatrixRun",
        json!({"runId":uuid::Uuid::now_v7().to_string(),
        "cells":["S1","S2","S3","S4","S5","S6","S7","S8","S9","S10"]}),
    )?;
    let mut peer = delivery_matrix_support::PeerFixture::start(&proof).await?;
    let mut automation = AutomationStore::open(&proof.root.join("automation.sqlite")).await?;
    let sender = proof.start_thread("Push matrix sender").await?;
    let mut covered_cells = Vec::new();

    // S1: an agent DM reaches a real Codex app-server and matches its inbox line.
    let codex_recipient = proof.start_thread("S1 Codex recipient").await?;
    let s1_prefix = matrix_marker("native-long");
    let s1_body = format!(
        "{s1_prefix} {}",
        "N".repeat(1_339 - s1_prefix.chars().count())
    );
    assert_eq!(s1_body.chars().count(), 1_340);
    let s1_receipt = cli_send(&proof, Some(&sender), &codex_recipient, &s1_body, false).await?;
    let s1_push_id = receipt_push_id(&s1_receipt)?;
    let (s1_show, s1_expected) =
        dm_notice_and_show(&mut proof, &codex_recipient, &s1_push_id).await?;
    let s1_item = wait_for_codex_input(
        &mut proof,
        &codex_recipient,
        &s1_prefix,
        OBSERVATION_TIMEOUT,
    )
    .await?;
    let s1_observed = user_message_text(&s1_item)?;
    assert_exact_notice_line(&s1_observed, &s1_expected)?;
    assert_long_preview_literal(&s1_observed, &s1_body)?;
    assert_inbox_notice_has_literal_long_preview(
        &mut proof,
        &codex_recipient,
        &s1_push_id,
        &s1_body,
    )
    .await?;
    let s1_fetched = cli_show(&proof, &codex_recipient, &s1_show.link).await?;
    assert_eq!(
        s1_fetched.pointer("/result/record/body"),
        Some(&json!(s1_body))
    );
    assert!(s1_fetched.pointer("/result/record/preview").is_none());
    record_passed_matrix_cell(&proof, config_guard, "S1", &[s1_observed])?;
    covered_cells.push("S1 Codex app-server");

    // S2: a long receipt is shortened to 100 Unicode scalars, linked, and fetchable in full.
    let s2_prefix = matrix_marker("long-receipt");
    let s2_body = format!(
        "{s2_prefix} {}",
        "R".repeat(1_339 - s2_prefix.chars().count())
    );
    assert_eq!(s2_body.chars().count(), 1_340);
    let s2_receipt = cli_send(&proof, Some(&sender), &peer.target, &s2_body, false).await?;
    let s2_push_id = receipt_push_id(&s2_receipt)?;
    let (s2_show, s2_expected) = dm_notice_and_show(&mut proof, &peer.target, &s2_push_id).await?;
    let s2_observed = peer
        .expect_text_with_timeout(&s2_prefix, OBSERVATION_TIMEOUT)
        .await?;
    assert_exact_notice_line(&s2_observed, &s2_expected)?;
    assert_long_preview_literal(&s2_observed, &s2_body)?;
    assert_inbox_notice_has_literal_long_preview(&mut proof, &peer.target, &s2_push_id, &s2_body)
        .await?;
    let s2_fetched = cli_show(&proof, &peer.target, &s2_show.link).await?;
    assert_eq!(
        s2_fetched.pointer("/result/record/body"),
        Some(&json!(s2_body))
    );
    assert!(s2_fetched.pointer("/result/record/preview").is_none());
    record_passed_matrix_cell(&proof, config_guard, "S2", &[s2_observed])?;
    covered_cells.push("S2 Claude peer");

    // S4: reply by the first push id reaches A even after a later DM from B.
    let sender_a = proof.start_thread("S4 sender A").await?;
    let sender_b = proof.start_thread("S4 sender B").await?;
    let s4_a_receipt = cli_send(
        &proof,
        Some(&sender_a),
        &peer.target,
        "Message from A",
        false,
    )
    .await?;
    let s4_a_push_id = receipt_push_id(&s4_a_receipt)?;
    let (s4_a_show, s4_a_expected) =
        dm_notice_and_show(&mut proof, &peer.target, &s4_a_push_id).await?;
    let s4_a_observed = peer
        .expect_text_with_timeout("Message from A", OBSERVATION_TIMEOUT)
        .await?;
    assert_exact_notice_line(&s4_a_observed, &s4_a_expected)?;

    let s4_b_receipt = cli_send(
        &proof,
        Some(&sender_b),
        &peer.target,
        "Message from B",
        false,
    )
    .await?;
    let s4_b_push_id = receipt_push_id(&s4_b_receipt)?;
    let (_, s4_b_expected) = dm_notice_and_show(&mut proof, &peer.target, &s4_b_push_id).await?;
    let s4_b_observed = peer
        .expect_text_with_timeout("Message from B", OBSERVATION_TIMEOUT)
        .await?;
    assert_exact_notice_line(&s4_b_observed, &s4_b_expected)?;

    let reply_receipt = cli_reply(&proof, &peer.target, &s4_a_push_id, "done").await?;
    let reply_push_id = receipt_push_id(&reply_receipt)?;
    let (reply_show, reply_expected) =
        dm_notice_and_show(&mut proof, &sender_a, &reply_push_id).await?;
    assert_eq!(
        reply_show
            .record
            .reply_to_push_id
            .as_ref()
            .map(PushId::as_str),
        Some(s4_a_push_id.as_str())
    );
    let reply_item =
        wait_for_codex_input(&mut proof, &sender_a, "done", OBSERVATION_TIMEOUT).await?;
    assert_exact_notice_line(&user_message_text(&reply_item)?, &reply_expected)?;
    assert_message_reply_without_reference_is_rejected(&proof).await?;
    record_passed_matrix_cell(
        &proof,
        config_guard,
        "S4",
        &[
            s4_a_observed,
            s4_b_observed,
            user_message_text(&reply_item)?,
        ],
    )?;
    covered_cells.push("S4 reply by id and no-id rejection");

    // S8: a third session cannot fetch A-to-B's DM by its push id or link.
    let third_party = proof.start_thread("S8 third party").await?;
    let forbidden_show = cli_show(&proof, &third_party, &s4_a_show.link).await?;
    assert_eq!(forbidden_show["error"]["serviceKind"], "notPermitted");
    assert_eq!(forbidden_show["error"]["data"]["kind"], "notPermitted");
    record_passed_matrix_cell(&proof, config_guard, "S8", &[])?;
    covered_cells.push("S8 third-party show denied");

    // S9: --human-user stays visibly unverified and uses the ordinary preview/link line.
    let s9_body = "Owner's compact note";
    let s9_receipt = cli_send(&proof, None, &peer.target, s9_body, true).await?;
    let s9_push_id = receipt_push_id(&s9_receipt)?;
    let (_, s9_expected) = dm_notice_and_show(&mut proof, &peer.target, &s9_push_id).await?;
    assert!(s9_expected.starts_with("🧑 Owner (unverified) @"));
    let s9_observed = peer
        .expect_text_with_timeout(s9_body, OBSERVATION_TIMEOUT)
        .await?;
    assert_exact_notice_line(&s9_observed, &s9_expected)?;
    record_passed_matrix_cell(&proof, config_guard, "S9", &[s9_observed])?;
    covered_cells.push("S9 unverified owner push");

    // S7: age a real-format push record in the private proof database, run the
    // production retention operation, and verify the recipient sees notFound.
    let stale_push_id: PushId = uuid::Uuid::now_v7().to_string().try_into()?;
    automation
        .insert_push_record(PushRecordDraft {
            mode: Some(collaboration_protocol::MessageDelivery::Auto),
            guard: None,
            push_id: stale_push_id.clone(),
            kind: PushKind::DirectMessage,
            origin: PushOrigin::Session(sender.clone()),
            origin_router_ref: None,
            target: peer.target.clone(),
            reply_to_push_id: None,
            header_facts: PushHeaderFacts::DirectMessage {
                sender_display_name: None,
            },
            body: Some("expired after 31 days".to_owned()),
            activity: None,
            created_at: Utc::now() - ChronoDuration::days(31),
        })
        .await?;
    automation.prune_push_records(Utc::now(), 500).await?;
    let stale_record = automation.get_push_record(&stale_push_id).await?;
    if stale_record.is_some() {
        return Err("31-day push was not pruned by the production retention operation".into());
    }
    let stale_link = format!(
        "router://{}/push/{}",
        String::from(proof.client.identity().service_id.clone()),
        stale_push_id.as_str()
    );
    let expired_show = cli_show(&proof, &peer.target, &stale_link).await?;
    assert_eq!(expired_show["error"]["serviceKind"], "notFound");
    assert_eq!(expired_show["error"]["data"]["kind"], "notFound");
    record_passed_matrix_cell(&proof, config_guard, "S7", &[])?;
    covered_cells.push("S7 expired show not found");
    record_passed_matrix_cell(&proof, config_guard, "S10", &[user_message_text(&s1_item)?])?;
    covered_cells.push("S10 recipient inputs are single notice lines");

    proof.record(
        "pushDeliveryMatrix",
        json!({"coveredCells":covered_cells,"separateModelCells":["S3","S5","S6"]}),
    )?;
    peer.shutdown().await?;
    config_guard.verify()?;
    proof.client.close().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires a fresh isolated debug Host with the scripted ACP target fixture"]
async fn push_delivery_matrix_reaches_scripted_acp_target() -> ProofResult<()> {
    let config_guard = delivery_matrix_support::ConfigHashGuard::capture()?;
    let result = exercise_scripted_acp_target(&config_guard).await;
    config_guard.verify()?;
    result
}

async fn exercise_scripted_acp_target(
    config_guard: &delivery_matrix_support::ConfigHashGuard,
) -> ProofResult<()> {
    let mut proof = ProofContext::connect().await?;
    let sender = proof.start_thread("ACP push matrix sender").await?;
    let target = create_provider_session(&mut proof, &sender).await?;
    let marker = matrix_marker("acp-target");
    let body = format!("{marker} {}", "A".repeat(1_339 - marker.chars().count()));
    assert_eq!(body.chars().count(), 1_340);
    delivery_matrix_support::mcp_send(&proof, &sender, &target, &body).await?;

    let inbox = proof
        .client
        .message_inbox(PushRecordListParams {
            caller: target.clone(),
            limit: 100,
        })
        .await?;
    let notice = inbox
        .records
        .iter()
        .find(|notice| notice.line.contains(&marker))
        .cloned()
        .ok_or("ACP target push is missing from the recipient inbox")?;
    assert_long_preview_literal(&notice.line, &body)?;
    let show = proof
        .client
        .router_show(PushRecordShowParams {
            caller: target.clone(),
            reference: notice.push_id.as_str().to_owned(),
        })
        .await?;
    assert_eq!(show.record.body.as_deref(), Some(body.as_str()));
    let show_json = serde_json::to_value(&show)?;
    if show_json.pointer("/record/preview").is_some() {
        return Err("router/show introduced an unsupported preview field".into());
    }
    let expected = expected_push_line(&show, &machine_label(&proof)?)?;
    assert_eq!(notice.line, expected);
    let observed = wait_for_acp_input(
        &proof.root.join("provider-prompt-receipts.jsonl"),
        &target,
        &marker,
        OBSERVATION_TIMEOUT,
    )
    .await?;
    assert_exact_notice_line(&observed, &expected)?;
    assert_long_preview_literal(&observed, &body)?;
    proof.record(
        "pushDeliveryMatrixAcp",
        json!({"pushId":show.record.push_id,"target":target,"firstLine":observed}),
    )?;
    config_guard.verify()?;
    proof.client.close().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires the documented automation-debug-host with existing debug model access"]
async fn push_delivery_matrix_holds_codex_recipients() -> ProofResult<()> {
    let mut proof = ProofContext::connect().await?;
    let config_guard = delivery_matrix_support::ConfigHashGuard::capture()?;
    let result = async {
        proof.record(
            "pushDeliveryMatrixRun",
            json!({
                "runId":uuid::Uuid::now_v7().to_string(),"cells":["S3","S5","S6"],
                "route":"documentedAcceptanceHost",
            }),
        )?;
        let unloaded_targets = held_cells::prepare_unloaded_targets(&mut proof).await?;
        let sender = proof.start_thread("Held matrix sender").await?;
        let machine_label = machine_label(&proof)?;
        let mut automation = AutomationStore::open(&proof.root.join("automation.sqlite")).await?;
        let mut covered_cells = Vec::new();
        // S3: a DM sent to an unloaded Codex conversation is stored as held and
        // arrives as the same line after an explicit conversation load.
        held_cells::cover_held_dm_cell(&mut proof, &sender, &unloaded_targets.held_dm).await?;
        config_guard.verify()?;
        covered_cells.push("S3 held DM delivered after Codex load");

        // S5: two independently subscribed roots are held as one batch, then
        // delivered as one notice after loading the Codex recipient.
        held_cells::cover_held_subscription_batch_cell(
            &mut proof,
            &sender,
            &unloaded_targets.held_subscription,
        )
        .await?;
        config_guard.verify()?;
        covered_cells.push("S5 held two-thread subscription batch delivered after Codex load");

        // S6: an unloaded Codex session resumes for a wake; a scheduled run then targets
        // a separate existing native session. Both inputs are matched back to their PushId.
        let wake_target = unloaded_targets.wake;
        let wake_marker = matrix_marker("wake");
        let wake_receipt = cli_wake_send(&proof, &sender, &wake_target, &wake_marker).await?;
        let fire: FireReceipt = serde_json::from_value(
            wake_receipt
                .pointer("/result/record/firstFire")
                .cloned()
                .ok_or("wake receipt omitted firstFire")?,
        )?;
        let wake_origin = RouterOriginRef::Wake {
            wakeup_id: fire.wakeup_id,
            occurrence_id: fire.occurrence_id,
        };
        let wake_record = automation
            .get_push_record_by_origin_reference(&wake_origin)
            .await?
            .ok_or("fired wake has no stored push record")?;
        let wake_show = proof
            .client
            .router_show(PushRecordShowParams {
                caller: wake_target.clone(),
                reference: wake_record.push_id.as_str().to_owned(),
            })
            .await?;
        let wake_expected = expected_push_line(&wake_show, &machine_label)?;
        let wake_item =
            wait_for_codex_input(&mut proof, &wake_target, &wake_marker, OBSERVATION_TIMEOUT)
                .await?;
        assert_exact_notice_line(&user_message_text(&wake_item)?, &wake_expected)?;
        let wake_correlation = record_push_input_correlation(
            &mut proof,
            "wake",
            &wake_show.record.push_id,
            &wake_item,
        )?;

        let schedule_target = proof
            .start_thread("S6 scheduled existing-session recipient")
            .await?;
        let schedule_marker = matrix_marker("schedule-run");
        let schedule_id =
            create_existing_session_schedule(&mut proof, &schedule_target, &schedule_marker)
                .await?;
        let (schedule_origin, schedule_record) = wait_for_schedule_push(
            &mut proof,
            &mut automation,
            schedule_id,
            OBSERVATION_TIMEOUT,
        )
        .await?;
        let schedule_show = proof
            .client
            .router_show(PushRecordShowParams {
                caller: schedule_target.clone(),
                reference: schedule_record.push_id.as_str().to_owned(),
            })
            .await?;
        let schedule_expected = expected_push_line(&schedule_show, &machine_label)?;
        let schedule_item = wait_for_codex_input(
            &mut proof,
            &schedule_target,
            &schedule_marker,
            OBSERVATION_TIMEOUT,
        )
        .await?;
        assert_exact_notice_line(&user_message_text(&schedule_item)?, &schedule_expected)?;
        let schedule_correlation = record_push_input_correlation(
            &mut proof,
            "schedule-run",
            &schedule_show.record.push_id,
            &schedule_item,
        )?;
        proof.record(
            "pushDeliveryCorrelation",
            json!({
                "wake":wake_correlation,
                "scheduleRun":schedule_correlation,
                "scheduleOrigin":schedule_origin,
            }),
        )?;
        record_passed_matrix_cell(
            &proof,
            &config_guard,
            "S6",
            &[
                user_message_text(&wake_item)?,
                user_message_text(&schedule_item)?,
            ],
        )?;
        covered_cells.push("S6 wake and existing-session schedule run");

        config_guard.verify()?;
        proof.record(
            "pushDeliveryModelMatrix",
            json!({"coveredCells":covered_cells}),
        )?;
        Ok(())
    }
    .await;
    config_guard.verify()?;
    proof.client.close().await?;
    result
}

fn record_passed_matrix_cell(
    proof: &ProofContext,
    config_guard: &delivery_matrix_support::ConfigHashGuard,
    cell: &str,
    observed_lines: &[String],
) -> ProofResult<()> {
    config_guard.verify()?;
    proof.record(
        "pushDeliveryMatrixCell",
        json!({
            "cell":cell,"status":"passed","firstLines":observed_lines,
        }),
    )
}

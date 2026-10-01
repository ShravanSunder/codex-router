#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic_in_result_fn
)]
//! Recipient-observed push-format matrix on an isolated debug Host.
#[path = "delivery_matrix/support.rs"]
#[allow(dead_code)]
mod delivery_matrix_support;
#[path = "automation_live_support/proof_context.rs"]
#[allow(dead_code)]
mod proof_context;

use automation_storage::AutomationStore;
use chrono::{Duration as ChronoDuration, Utc};
use collaboration_client::{
    ConversationClient, ConversationCreateActor, ConversationCreateInput,
    protocol::{
        ConversationCreateOutcome, DestinationPreparation, EndpointRef, ExecutionDestination,
        InstructionCreateParams, OperationId, PushRecordListParams, PushRecordShowParams,
        RouterAccess, RunListRequest, ScheduleCreateRequest, ScheduleDefinition,
        ScheduleEnableRequest, SchedulePrepareRequest, SessionRef, TimingRequest,
    },
};
use collaboration_protocol::{
    FireReceipt, MachineLabel, PushHeaderFacts, PushId, PushKind, PushLineInput, PushOrigin,
    PushRecord, PushRecordDraft, PushRecordNotice, PushRecordShowResult, RouterLink,
    RouterOriginRef, render_push_line,
};
use proof_context::{ProofContext, ProofResult};
use serde_json::{Value, json};
use std::{path::Path, time::Duration};
use tokio::process::Command;

const OBSERVATION_TIMEOUT: Duration = Duration::from_secs(120);
const ACP_TARGET_EXPECTED_PROMPTS: usize = 9;

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
    let mut proof = ProofContext::connect().await?;
    let mut peer = delivery_matrix_support::PeerFixture::start(&proof).await?;
    let mut automation = AutomationStore::open(&proof.root.join("automation.sqlite")).await?;
    let sender = proof.start_thread("Push matrix sender").await?;
    let machine_label = machine_label(&proof)?;
    let mut covered_cells = Vec::new();

    // S1: an agent DM reaches a real Codex app-server and matches its inbox line.
    let codex_recipient = proof.start_thread("S1 Codex recipient").await?;
    let s1_body = "S1 is green";
    let s1_receipt = cli_send(&proof, Some(&sender), &codex_recipient, s1_body, false).await?;
    let s1_push_id = receipt_push_id(&s1_receipt)?;
    let (_, s1_expected) = dm_notice_and_show(&mut proof, &codex_recipient, &s1_push_id).await?;
    let s1_item =
        wait_for_codex_input(&mut proof, &codex_recipient, s1_body, OBSERVATION_TIMEOUT).await?;
    assert_exact_notice_line(&user_message_text(&s1_item)?, &s1_expected)?;
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
    assert!(s2_expected.contains(&format!(
        "\"{}\" (+1240)",
        s2_body.chars().take(100).collect::<String>()
    )));
    assert!(s2_expected.contains("(+1240)"));
    let s2_fetched = cli_show(&proof, &peer.target, &s2_show.link).await?;
    assert_eq!(
        s2_fetched.pointer("/result/record/body"),
        Some(&json!(s2_body))
    );
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
    covered_cells.push("S4 reply by id and no-id rejection");

    // S8: a third session cannot fetch A-to-B's DM by its push id or link.
    let third_party = proof.start_thread("S8 third party").await?;
    let forbidden_show = cli_show(&proof, &third_party, &s4_a_show.link).await?;
    assert_eq!(forbidden_show["error"]["serviceKind"], "notPermitted");
    assert_eq!(forbidden_show["error"]["data"]["kind"], "notPermitted");
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
    covered_cells.push("S9 unverified owner push");

    // S6: an unloaded Codex session resumes for a wake; a scheduled run then targets
    // a separate existing native session. Both inputs are matched back to their PushId.
    let wake_target = create_empty_codex_conversation(&proof, &sender).await?;
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
        wait_for_codex_input(&mut proof, &wake_target, &wake_marker, OBSERVATION_TIMEOUT).await?;
    assert_exact_notice_line(&user_message_text(&wake_item)?, &wake_expected)?;
    let wake_correlation =
        record_push_input_correlation(&mut proof, "wake", &wake_show.record.push_id, &wake_item)?;

    let schedule_target = proof
        .start_thread("S6 scheduled existing-session recipient")
        .await?;
    let schedule_marker = matrix_marker("schedule-run");
    let schedule_id =
        create_existing_session_schedule(&mut proof, &schedule_target, &schedule_marker).await?;
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
    covered_cells.push("S6 wake and existing-session schedule run");

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
    covered_cells.push("S7 expired show not found");
    covered_cells.push("S10 recipient inputs are single notice lines");

    proof.record(
        "pushDeliveryMatrix",
        json!({"coveredCells":covered_cells,"uncoveredCells":["S3 needs C2 DM holds","S5 needs C2 held subscription batch"]}),
    )?;
    peer.shutdown().await?;
    proof.client.close().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires a fresh isolated debug Host with the scripted ACP target fixture"]
async fn push_delivery_matrix_reaches_scripted_acp_target() -> ProofResult<()> {
    let mut proof = ProofContext::connect().await?;
    let sender = proof.start_thread("ACP push matrix sender").await?;
    let target = create_provider_session(&mut proof, &sender).await?;
    let marker = matrix_marker("acp-target");
    delivery_matrix_support::mcp_send(&proof, &sender, &target, &marker).await?;

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
    let show = proof
        .client
        .router_show(PushRecordShowParams {
            caller: target.clone(),
            reference: notice.push_id.as_str().to_owned(),
        })
        .await?;
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
    proof.record(
        "pushDeliveryMatrixAcp",
        json!({"pushId":show.record.push_id,"target":target,"firstLine":observed}),
    )?;
    proof.client.close().await?;
    Ok(())
}

struct ConversationFixtureSettings {
    endpoint: EndpointRef,
    access: RouterAccess,
    model: Option<String>,
    effort: Option<String>,
}

async fn create_conversation_fixture(
    proof: &ProofContext,
    creator: &SessionRef,
    settings: ConversationFixtureSettings,
) -> ProofResult<SessionRef> {
    let endpoint = settings.endpoint.clone();
    let conversation = ConversationClient::connect(&proof.service_directory, &endpoint).await?;
    match conversation
        .create(
            ConversationCreateInput {
                operation_id: OperationId::generate(),
                endpoint,
                working_directory: proof.workspace.clone(),
                access: settings.access,
                created_by: ConversationCreateActor::Session(creator.clone()),
                approver: Some(ConversationCreateActor::Session(creator.clone())),
                generation: None,
                model: settings.model,
                mode: None,
                effort: settings.effort,
                fork: None,
                root_message_id: None,
            },
            Duration::from_secs(60),
        )
        .await?
    {
        ConversationCreateOutcome::Created { target, .. }
        | ConversationCreateOutcome::CreatedWithoutSettings { target, .. } => Ok(target),
        ConversationCreateOutcome::Pending { .. } => {
            Err("Conversation fixture creation stayed pending".into())
        }
    }
}

async fn create_provider_session(
    proof: &mut ProofContext,
    creator: &SessionRef,
) -> ProofResult<SessionRef> {
    let provider = proof
        .client
        .list_endpoints()
        .await?
        .endpoints
        .into_iter()
        .find(|entry| String::from(entry.endpoint.endpoint_id.clone()) == "cursor-local")
        .ok_or("Scripted Cursor ACP endpoint missing")?
        .endpoint;
    create_conversation_fixture(
        proof,
        creator,
        ConversationFixtureSettings {
            endpoint: provider,
            access: RouterAccess::WriteRestricted,
            model: None,
            effort: None,
        },
    )
    .await
}

async fn create_empty_codex_conversation(
    proof: &ProofContext,
    creator: &SessionRef,
) -> ProofResult<SessionRef> {
    create_conversation_fixture(
        proof,
        creator,
        ConversationFixtureSettings {
            endpoint: proof.endpoint.clone(),
            access: RouterAccess::WorkspaceWrite,
            model: Some("gpt-5.6-luna".to_owned()),
            effort: Some("low".to_owned()),
        },
    )
    .await
}

async fn cli_send(
    proof: &ProofContext,
    sender: Option<&SessionRef>,
    target: &SessionRef,
    body: &str,
    human_user: bool,
) -> ProofResult<Value> {
    let target_json = serde_json::to_string(target)?;
    let mut arguments = vec![
        "message".to_owned(),
        "send".to_owned(),
        "--to".to_owned(),
        target_json,
        "--text".to_owned(),
        body.to_owned(),
    ];
    if let Some(sender) = sender {
        arguments.extend(["--from".to_owned(), serde_json::to_string(sender)?]);
    }
    if human_user {
        arguments.push("--human-user".to_owned());
    }
    arguments.push("--json".to_owned());
    let response = successful_cli_result(run_cli(proof, None, arguments).await?, "message send")?;
    response
        .pointer("/result/record")
        .cloned()
        .ok_or_else(|| "message send omitted its push receipt".into())
}

async fn cli_reply(
    proof: &ProofContext,
    caller: &SessionRef,
    reference: &str,
    text: &str,
) -> ProofResult<Value> {
    let response = successful_cli_result(
        run_cli(
            proof,
            Some(caller),
            vec![
                "message".to_owned(),
                "reply".to_owned(),
                reference.to_owned(),
                text.to_owned(),
                "--json".to_owned(),
            ],
        )
        .await?,
        "message reply",
    )?;
    response
        .pointer("/result")
        .cloned()
        .ok_or_else(|| "message reply omitted its push receipt".into())
}

async fn assert_message_reply_without_reference_is_rejected(
    proof: &ProofContext,
) -> ProofResult<()> {
    let output = run_cli(
        proof,
        None,
        vec![
            "message".into(),
            "reply".into(),
            "done".into(),
            "--json".into(),
        ],
    )
    .await?;
    if output.status.success() {
        return Err("message reply accepted text without a push reference".into());
    }
    Ok(())
}

async fn cli_show(
    proof: &ProofContext,
    caller: &SessionRef,
    reference: &str,
) -> ProofResult<Value> {
    let output = run_cli(
        proof,
        Some(caller),
        vec!["show".into(), reference.into(), "--json".into()],
    )
    .await?;
    let response: Value = serde_json::from_slice(&output.stdout)?;
    if !output.status.success() {
        return Ok(response);
    }
    if response["kind"] != "result" {
        return Err(format!("show returned an unexpected result: {response}").into());
    }
    Ok(response)
}

async fn run_cli(
    proof: &ProofContext,
    caller: Option<&SessionRef>,
    arguments: Vec<String>,
) -> ProofResult<std::process::Output> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_agent-collaboration"));
    command
        .args(arguments)
        .arg("--service-directory")
        .arg(&proof.service_directory);
    if let Some(caller) = caller {
        set_caller_identity(&mut command, caller)?;
    }
    Ok(tokio::time::timeout(OBSERVATION_TIMEOUT, command.kill_on_drop(true).output()).await??)
}

fn successful_cli_result(output: std::process::Output, operation: &str) -> ProofResult<Value> {
    let response: Value = serde_json::from_slice(&output.stdout)?;
    if !output.status.success() || response["kind"] != "result" {
        return Err(format!(
            "{operation} failed: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(response)
}

fn set_caller_identity(command: &mut Command, caller: &SessionRef) -> ProofResult<()> {
    command
        .env_remove("CODEX_THREAD_ID")
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .env_remove("CURSOR_CONVERSATION_ID");
    let endpoint = String::from(caller.endpoint.endpoint_id.clone());
    let identity_variable = match endpoint.as_str() {
        "codex" | "codex-local" => "CODEX_THREAD_ID",
        "claude" | "claude-local" => "CLAUDE_CODE_SESSION_ID",
        "cursor" | "cursor-local" => "CURSOR_CONVERSATION_ID",
        _ => return Err(format!("unsupported fixture endpoint {endpoint}").into()),
    };
    command.env(identity_variable, String::from(caller.session_id.clone()));
    Ok(())
}

fn receipt_push_id(receipt: &Value) -> ProofResult<String> {
    receipt["pushId"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("push receipt omitted pushId: {receipt}").into())
}

async fn dm_notice_and_show(
    proof: &mut ProofContext,
    recipient: &SessionRef,
    push_id: &str,
) -> ProofResult<(PushRecordShowResult, String)> {
    let inbox = proof
        .client
        .message_inbox(PushRecordListParams {
            caller: recipient.clone(),
            limit: 100,
        })
        .await?;
    let notice: PushRecordNotice = inbox
        .records
        .iter()
        .find(|notice| notice.push_id.as_str() == push_id)
        .cloned()
        .ok_or_else(|| format!("push {push_id} is missing from message/inbox"))?;
    let show = proof
        .client
        .router_show(PushRecordShowParams {
            caller: recipient.clone(),
            reference: notice.link,
        })
        .await?;
    if show.record.target != *recipient || show.record.push_id.as_str() != push_id {
        return Err("router/show returned a different DM record".into());
    }
    let expected = expected_push_line(&show, &machine_label(proof)?)?;
    assert_eq!(notice.line, expected);
    Ok((show, expected))
}

fn machine_label(proof: &ProofContext) -> ProofResult<MachineLabel> {
    proof
        .client
        .machine_label()
        .cloned()
        .ok_or_else(|| "debug Host service manifest omitted machine label".into())
}

fn expected_push_line(show: &PushRecordShowResult, machine: &MachineLabel) -> ProofResult<String> {
    let record = &show.record;
    let link = RouterLink::parse(&show.link)?;
    if link.push_id() != &record.push_id
        || link.machine_id().as_str() != String::from(record.target.endpoint.service_id.clone())
    {
        return Err(format!(
            "router/show link did not match push identity: {}",
            show.link
        )
        .into());
    }
    Ok(render_push_line(&PushLineInput {
        link,
        machine_label: machine.clone(),
        origin: record.origin.clone(),
        header_facts: record.header_facts.clone(),
        body: record.body.clone(),
    })?)
}

fn assert_exact_notice_line(observed: &str, expected: &str) -> ProofResult<()> {
    if observed != expected || observed.contains('\n') || observed.contains('\r') {
        return Err(format!(
            "recipient input was not the exact single push line\nexpected: {expected:?}\nobserved: {observed:?}"
        )
        .into());
    }
    Ok(())
}

fn user_message_text(item: &Value) -> ProofResult<String> {
    if item.get("type").and_then(Value::as_str) != Some("userMessage") {
        return Err("native input observation was not a userMessage".into());
    }
    let parts = item
        .get("content")
        .and_then(Value::as_array)
        .ok_or("native userMessage omitted content")?;
    let text = parts
        .iter()
        .filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect::<String>();
    if text.is_empty() {
        return Err("native userMessage contained no text".into());
    }
    Ok(text)
}

async fn wait_for_codex_input(
    proof: &mut ProofContext,
    target: &SessionRef,
    marker: &str,
    timeout: Duration,
) -> ProofResult<Value> {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut observation = tokio::time::interval(Duration::from_millis(250));
    loop {
        observation.tick().await;
        let turns = proof.turns(target).await?;
        for turn in &turns {
            let Some(items) = turn.get("items").and_then(Value::as_array) else {
                continue;
            };
            for item in items {
                if item.get("type").and_then(Value::as_str) == Some("userMessage")
                    && user_message_text(item).is_ok_and(|text| text.contains(marker))
                {
                    return Ok(item.clone());
                }
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(format!("thread/read did not observe push marker {marker}").into());
        }
    }
}

fn record_push_input_correlation(
    proof: &mut ProofContext,
    producer: &str,
    push_id: &PushId,
    user_message: &Value,
) -> ProofResult<bool> {
    let observed_id = user_message.get("id").and_then(Value::as_str);
    match observed_id {
        Some(observed_id) => {
            if observed_id != push_id.as_str() {
                return Err(format!(
                    "{producer} native userMessage id {observed_id} did not match PushId {}",
                    push_id.as_str()
                )
                .into());
            }
            proof.record(
                "pushInputCorrelation",
                json!({"producer":producer,"pushId":push_id,"observedUserMessageId":observed_id,"status":"asserted"}),
            )?;
            Ok(true)
        }
        None => {
            proof.record(
                "pushInputCorrelationGap",
                json!({"producer":producer,"pushId":push_id,"status":"threadReadHasNoUserMessageId"}),
            )?;
            Ok(false)
        }
    }
}

async fn cli_wake_send(
    proof: &ProofContext,
    sender: &SessionRef,
    target: &SessionRef,
    body: &str,
) -> ProofResult<Value> {
    let sender_json = serde_json::to_string(sender)?;
    let target_json = serde_json::to_string(target)?;
    let mut command = Command::new(env!("CARGO_BIN_EXE_agent-collaboration"));
    command
        .args([
            "wake",
            "send",
            "--from",
            &sender_json,
            "--to",
            &target_json,
            "--text",
            body,
            "--after",
            "1s",
            "--wait-until-first-fire",
            "--operation-id",
            &uuid::Uuid::now_v7().to_string(),
        ])
        .arg("--service-directory")
        .arg(&proof.service_directory)
        .arg("--json");
    let output =
        tokio::time::timeout(OBSERVATION_TIMEOUT, command.kill_on_drop(true).output()).await??;
    let response: Value = serde_json::from_slice(&output.stdout)?;
    if !output.status.success()
        || response.pointer("/result/record/firstFire/kind") != Some(&json!("wakeFired"))
    {
        return Err(format!("wake did not report first fire: {response}").into());
    }
    Ok(response)
}

async fn create_existing_session_schedule(
    proof: &mut ProofContext,
    target: &SessionRef,
    body: &str,
) -> ProofResult<agent_automation::ScheduleId> {
    let instruction = proof
        .client
        .create_instruction(InstructionCreateParams {
            operation_id: OperationId::generate(),
            text: body.to_owned().try_into()?,
        })
        .await?;
    let schedule = proof
        .client
        .create_schedule(ScheduleCreateRequest {
            operation_id: OperationId::generate(),
            definition: ScheduleDefinition {
                instruction_id: instruction.instruction_id,
                timing: TimingRequest::After {
                    seconds: 5.try_into()?,
                },
                enabled: false,
                destination: ExecutionDestination::Unprepared,
                execution_timeout_seconds: Some(120.try_into()?),
                model: Some("gpt-5.6-luna".to_owned()),
                effort: Some("low".to_owned()),
            },
        })
        .await?;
    proof
        .client
        .prepare_schedule(SchedulePrepareRequest {
            operation_id: OperationId::generate(),
            schedule_id: schedule.schedule_id.clone(),
            destination: DestinationPreparation::Existing {
                target: target.clone(),
                cwd: proof.workspace.to_string_lossy().into_owned(),
            },
        })
        .await?;
    let enabled = proof
        .client
        .enable_schedule(ScheduleEnableRequest {
            operation_id: OperationId::generate(),
            schedule_id: schedule.schedule_id.clone(),
        })
        .await?;
    if enabled.next_due_at.is_none() {
        return Err("existing-session schedule has no next due time".into());
    }
    Ok(schedule.schedule_id)
}

async fn wait_for_schedule_push(
    proof: &mut ProofContext,
    automation: &mut AutomationStore,
    schedule_id: agent_automation::ScheduleId,
    timeout: Duration,
) -> ProofResult<(RouterOriginRef, PushRecord)> {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut observation = tokio::time::interval(Duration::from_millis(250));
    loop {
        observation.tick().await;
        let runs = proof
            .client
            .list_runs(RunListRequest {
                schedule_id: schedule_id.clone(),
                cursor: None,
                limit: 25_u32.try_into()?,
            })
            .await?;
        if let Some(run) = runs.records.first() {
            let origin = RouterOriginRef::ScheduleRun {
                schedule_id: schedule_id.clone(),
                run_id: run.run_id.clone(),
            };
            if let Some(record) = automation
                .get_push_record_by_origin_reference(&origin)
                .await?
            {
                return Ok((origin, record));
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("existing-session schedule produced no stored run push".into());
        }
    }
}

async fn wait_for_acp_input(
    receipt_path: &Path,
    target: &SessionRef,
    marker: &str,
    timeout: Duration,
) -> ProofResult<String> {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut observation = tokio::time::interval(Duration::from_millis(250));
    loop {
        observation.tick().await;
        match std::fs::read_to_string(receipt_path) {
            Ok(contents) => {
                for line in contents.lines() {
                    let frame: Value = serde_json::from_str(line)?;
                    if frame["method"] != "session/prompt"
                        || frame["params"]["sessionId"] != String::from(target.session_id.clone())
                    {
                        return Err("ACP recipient log contains another session or method".into());
                    }
                    if let Some(parts) = frame["params"]["prompt"].as_array() {
                        for part in parts {
                            if let Some(text) = part["text"].as_str()
                                && text.contains(marker)
                            {
                                return Ok(text.to_owned());
                            }
                        }
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(format!("ACP session/prompt log omitted marker {marker}").into());
        }
    }
}

fn matrix_marker(producer: &str) -> String {
    format!("PUSH_DELIVERY_MATRIX_{producer}_{}", uuid::Uuid::now_v7())
}

fn user_text_contains_marker(text: &str, marker: &str) -> bool {
    text.contains(marker)
}

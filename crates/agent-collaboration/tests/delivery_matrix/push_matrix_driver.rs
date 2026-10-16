//! CLI producers and recipient observations for the isolated push matrix.
use super::cli_process::run_cli_output;
use super::native_push_correlation::{
    ExpectedNativePushCorrelation, validate_native_push_correlation,
};
use super::owned_resources::OwnedMatrixResources;
use super::{OBSERVATION_TIMEOUT, ProofContext, ProofResult};
use automation_storage::AutomationStore;
use collaboration_client::{
    ConversationClient, ConversationCreateActor, ConversationCreateInput,
    protocol::{
        ConversationCreateOutcome, DestinationPreparation, EndpointRef, ExecutionDestination,
        InstructionCreateParams, OperationId, PushRecordListParams, PushRecordShowParams,
        RouterAccess, RunListRequest, ScheduleCreateRequest, ScheduleDefinition,
        ScheduleEnableRequest, SchedulePrepareRequest, SessionRef, TimingRequest, WakeupId,
    },
};
use collaboration_protocol::{
    DeliveryClientReceipt, MachineLabel, NativeSendAcceptance, PushLineInput, PushRecord,
    PushRecordNotice, PushRecordShowResult, RouterLink, RouterOriginRef, render_push_line,
};
use serde_json::{Value, json};
use std::{path::Path, time::Duration};
use tokio::process::Command;

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
    let conversation = ConversationClient::connect(
        &collaboration_client::CollaborationAccess::api(&proof.service_directory),
        &endpoint,
    )
    .await?;
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

pub(super) async fn create_provider_session(
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

pub(super) async fn cli_send(
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

pub(super) async fn cli_reply(
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
        .pointer("/result/record")
        .cloned()
        .ok_or_else(|| "message reply omitted its push receipt".into())
}

pub(super) async fn assert_message_reply_without_reference_is_rejected(
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

pub(super) async fn cli_show(
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
    proof.record(
        "pushMatrixCliShow",
        json!({
            "caller":caller,"reference":reference,"exitCode":output.status.code(),
            "firstOutputLine":String::from_utf8_lossy(&output.stdout).lines().next(),
        }),
    )?;
    if !output.status.success() {
        return Ok(response);
    }
    if response["kind"] != "result" {
        return Err(format!("show returned an unexpected result: {response}").into());
    }
    Ok(response)
}

pub(super) async fn run_cli(
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
    run_cli_output(command, OBSERVATION_TIMEOUT).await
}

pub(super) fn successful_cli_result(
    output: std::process::Output,
    operation: &str,
) -> ProofResult<Value> {
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

pub(super) fn set_caller_identity(command: &mut Command, caller: &SessionRef) -> ProofResult<()> {
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

pub(super) fn receipt_push_id(receipt: &Value) -> ProofResult<String> {
    receipt["pushId"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("push receipt omitted pushId: {receipt}").into())
}

pub(super) async fn dm_notice_and_show(
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

pub(super) fn machine_label(proof: &ProofContext) -> ProofResult<MachineLabel> {
    Ok(proof.client.machine_label().clone())
}

pub(super) fn expected_push_line(
    show: &PushRecordShowResult,
    machine: &MachineLabel,
) -> ProofResult<String> {
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

pub(super) fn assert_exact_notice_line(observed: &str, expected: &str) -> ProofResult<()> {
    if observed != expected || observed.contains('\n') || observed.contains('\r') {
        return Err(format!(
            "recipient input was not the exact single push line\nexpected: {expected:?}\nobserved: {observed:?}"
        )
        .into());
    }
    Ok(())
}

pub(super) fn user_message_text(item: &Value) -> ProofResult<String> {
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

pub(super) async fn wait_for_codex_input(
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

pub(super) async fn record_push_input_correlation(
    proof: &mut ProofContext,
    producer: &str,
    record: &PushRecord,
    user_message: &Value,
) -> ProofResult<bool> {
    let receipt = record
        .last_outcome
        .as_ref()
        .and_then(|outcome| outcome.client.as_ref());
    let Some(DeliveryClientReceipt::CodexAppServer(receipt)) = receipt else {
        return Err(format!("{producer} omitted native delivery acceptance").into());
    };
    if receipt.target != record.target
        || receipt.generation != proof.generation
        || String::from(receipt.client_user_message_id.clone()) != record.push_id.as_str()
    {
        return Err(
            format!("{producer} native receipt target/generation/correlation mismatch").into(),
        );
    }
    let turn_id = match &receipt.acceptance {
        NativeSendAcceptance::NativeInputAccepted { turn_id, .. }
        | NativeSendAcceptance::SteerAccepted { turn_id, .. } => String::from(turn_id.clone()),
        NativeSendAcceptance::QueueAccepted { .. } => {
            return Err("S6 requires a native accepted turn".into());
        }
    };
    let observed_id = user_message
        .get("id")
        .and_then(Value::as_str)
        .ok_or("native userMessage omitted its generated identity")?;
    if user_message.get("clientId").and_then(Value::as_str) != Some(record.push_id.as_str()) {
        return Err(format!("{producer} native userMessage clientId did not match PushId").into());
    }
    let turns = proof.turns(&record.target).await?;
    validate_native_push_correlation(
        &turns,
        ExpectedNativePushCorrelation {
            push_id: record.push_id.as_str(),
            accepted_turn_id: &turn_id,
            native_item_id: observed_id,
        },
    )?;
    proof.record(
        "pushInputCorrelation",
        json!({
            "producer":producer,"pushId":record.push_id,"observedUserMessageId":observed_id,
            "observedClientId":record.push_id,"acceptedTurnId":turn_id,
            "target":record.target,"generation":receipt.generation,"status":"asserted"
        }),
    )?;
    Ok(true)
}

pub(super) async fn cli_wake_send(
    proof: &ProofContext,
    sender: &SessionRef,
    target: &SessionRef,
    body: &str,
    resources: &mut OwnedMatrixResources,
) -> ProofResult<Value> {
    let sender_json = serde_json::to_string(sender)?;
    let target_json = serde_json::to_string(target)?;
    let operation_id = OperationId::generate();
    let operation_text = String::from(operation_id.clone());
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
            &operation_text,
        ])
        .arg("--service-directory")
        .arg(&proof.service_directory)
        .arg("--json");
    resources.record_wake_submission(operation_id.clone())?;
    let output = run_cli_output(command, OBSERVATION_TIMEOUT).await?;
    let response: Value = serde_json::from_slice(&output.stdout)?;
    retain_created_wake(resources, &operation_id, &response)?;
    if !output.status.success()
        || response.pointer("/result/record/firstFire/kind") != Some(&json!("wakeFired"))
    {
        return Err(format!("wake did not report first fire: {response}").into());
    }
    Ok(response)
}

fn retain_created_wake(
    resources: &mut OwnedMatrixResources,
    operation_id: &OperationId,
    response: &Value,
) -> ProofResult<()> {
    let mut returned_id: Option<WakeupId> = None;
    for path in [
        "/result/record/definition/wakeupId",
        "/wakeupId",
        "/created/definition/wakeupId",
    ] {
        if let Some(value) = response.pointer(path) {
            let observed: WakeupId = serde_json::from_value(value.clone())?;
            if returned_id.as_ref().is_some_and(|known| known != &observed) {
                // Conflicting identities remain submission-only; no guessed cleanup target.
                return Err("wake receipt returned conflicting resource identities".into());
            }
            returned_id = Some(observed);
        }
    }
    if let Some(wakeup_id) = returned_id {
        resources.record_wake_receipt(operation_id, wakeup_id)?;
    }
    Ok(())
}

pub(super) async fn create_existing_session_schedule(
    proof: &mut ProofContext,
    target: &SessionRef,
    body: &str,
    resources: &mut OwnedMatrixResources,
) -> ProofResult<agent_automation::ScheduleId> {
    let instruction_operation = OperationId::generate();
    resources.record_instruction_submission(instruction_operation.clone())?;
    let instruction = proof
        .client
        .create_instruction(InstructionCreateParams {
            operation_id: instruction_operation.clone(),
            text: body.to_owned().try_into()?,
        })
        .await?;
    resources
        .record_instruction_receipt(&instruction_operation, instruction.instruction_id.clone())?;
    let schedule_operation = OperationId::generate();
    resources.record_schedule_submission(schedule_operation.clone())?;
    let schedule = proof
        .client
        .create_schedule(ScheduleCreateRequest {
            operation_id: schedule_operation.clone(),
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
    resources.record_schedule_receipt(&schedule_operation, schedule.schedule_id.clone())?;
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

pub(super) async fn wait_for_schedule_push(
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

pub(super) async fn wait_for_acp_input(
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

pub(super) fn matrix_marker(producer: &str) -> String {
    format!("PUSH_DELIVERY_MATRIX_{producer}_{}", uuid::Uuid::now_v7())
}

pub(super) fn user_text_contains_marker(text: &str, marker: &str) -> bool {
    text.contains(marker)
}

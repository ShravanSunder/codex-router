//! Opt-in live proof. Run only against automation-debug-host, never production or an existing thread.
#[path = "automation_live_support/busy_thread_delivery.rs"]
mod busy_thread_delivery;
#[path = "automation_live_support/debug_backend_restart.rs"]
mod debug_backend_restart;
#[path = "automation_live_support/durable_delivery_recovery.rs"]
mod durable_delivery_recovery;
#[path = "automation_live_support/proof_context.rs"]
mod proof_context;
#[path = "automation_live_support/scheduled_continuity.rs"]
mod scheduled_continuity;
#[path = "automation_live_support/summary_failure_recovery.rs"]
mod summary_failure_recovery;
#[path = "automation_live_support/worker_timeout_proof.rs"]
mod worker_timeout_proof;
use communication_protocol::{
    AutomationPageRequest, DeliveryEvidence, DeliveryListRequest, MessageContent, MessageDelivery,
    NativeSendParams, NativeSendReceipt,
};
use proof_context::{ProofContext, ProofResult, shell_quote};
use serde_json::{Value, json};

#[tokio::test]
#[ignore = "requires an isolated debug Host; interrupts only its own Luna summary and opens a controlled delivery gate"]
async fn summary_recovery_and_durable_delivery_preserve_original_work() -> ProofResult<()> {
    let mut proof = ProofContext::connect().await?;
    worker_timeout_proof::exercise(&mut proof).await?;
    let portable = summary_failure_recovery::exercise(&mut proof).await?;
    durable_delivery_recovery::exercise(&mut proof, portable).await?;
    proof.client.close().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires an isolated debug Host; exercises busy Luna input and owned backend replacement"]
async fn scheduled_work_waits_while_ordinary_messages_steer() -> ProofResult<()> {
    busy_thread_delivery::exercise().await
}

#[tokio::test]
#[ignore = "requires an isolated debug Host; runs two fresh Luna workflows and their summaries"]
async fn fresh_scheduled_run_uses_previous_luna_summary() -> ProofResult<()> {
    scheduled_continuity::exercise().await
}

#[tokio::test]
#[ignore = "requires an isolated debug Host; submits one minimal Luna turn and observes its history"]
async fn fresh_native_history_becomes_readable_without_resubmission() -> ProofResult<()> {
    let mut proof = ProofContext::connect().await?;
    let target = proof.start_thread("Luna history readiness probe").await?;
    let thread_id = String::from(target.session_id.clone());
    let database_path = std::path::PathBuf::from(std::env::var_os("HOME").ok_or("HOME missing")?)
        .join(".codex/state_5.sqlite");
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(database_path)
        .read_only(true)
        .create_if_missing(false);
    let metadata = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await?;
    let receipt = proof
        .client
        .send_agent_message(NativeSendParams {
            target: target.clone(),
            generation: proof.generation.clone(),
            message: MessageContent::Agent {
                sender: target.clone(),
                text: "Output exactly HISTORY_READY. Do not call tools or other agents."
                    .to_owned()
                    .try_into()?,
            },
            delivery: MessageDelivery::Auto,
            client_user_message_id: None,
        })
        .await?;
    let turn_id = match &receipt.acceptance {
        communication_protocol::NativeSendAcceptance::NativeInputAccepted { turn_id, .. } => {
            String::from(turn_id.clone())
        }
        _ => return Err("Fresh history probe did not receive an exact native turn".into()),
    };
    proof.record("historyProbeInputAccepted", json!(receipt))?;
    let started = tokio::time::Instant::now();
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
    loop {
        interval.tick().await;
        let history_mode: Option<String> =
            sqlx::query_scalar("SELECT history_mode FROM threads WHERE id = ?")
                .bind(&thread_id)
                .fetch_optional(&metadata)
                .await?;
        let response = proof
            .native
            .request_validated(
                &proof.schemas,
                codex_native_integration::NativeOperation::ReadThread,
                json!({"threadId":thread_id,"includeTurns":true}),
            )
            .await;
        match response {
            Ok(response) => {
                if response.pointer("/thread/id").and_then(Value::as_str)
                    != Some(thread_id.as_str())
                {
                    return Err("History probe returned a different native thread".into());
                }
                let turn = response
                    .pointer("/thread/turns")
                    .and_then(Value::as_array)
                    .and_then(|turns| {
                        turns.iter().find(|turn| {
                            turn.get("id").and_then(Value::as_str) == Some(turn_id.as_str())
                        })
                    });
                proof.record(
                    "historyReadinessObserved",
                    json!({
                        "elapsedMs":started.elapsed().as_millis(),"historyMode":history_mode,
                        "target":target,"turn":turn
                    }),
                )?;
                if turn.is_some_and(|turn| {
                    turn.get("status").and_then(Value::as_str) == Some("completed")
                        && proof_context::agent_text(turn).contains("HISTORY_READY")
                }) {
                    metadata.close().await;
                    proof.client.close().await?;
                    return Ok(());
                }
            }
            Err(error) => {
                let rejection = proof.native.take_last_rejection();
                proof.record(
                    "historyReadinessRejected",
                    json!({
                        "elapsedMs":started.elapsed().as_millis(),"historyMode":history_mode,
                        "target":target,"rejection":rejection
                    }),
                )?;
                if !matches!(
                    error,
                    codex_native_integration::NativeConnectionError::Rejected { .. }
                ) {
                    return Err(error.into());
                }
            }
        }
        if started.elapsed() >= std::time::Duration::from_secs(45) {
            return Err("Exact fresh turn history was not readable and completed within 45 seconds; no input was resent".into());
        }
    }
}

#[tokio::test]
#[ignore = "requires an explicitly launched isolated automation-debug-host and real Luna access"]
async fn luna_agents_arrange_wake_and_reply_through_the_real_cli() -> ProofResult<()> {
    let mut proof = ProofContext::connect().await?;
    let python = proof_context::python_executable().await?;
    let forbidden_socket = proof.root.join("other-control.sock");
    let _forbidden_listener = tokio::net::UnixListener::bind(&forbidden_socket)?;
    let forbidden_tcp = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
    let forbidden_port = forbidden_tcp.local_addr()?.port();
    let alpha = proof.start_thread("Luna A").await?;
    let beta = proof.start_thread("Luna B").await?;
    let identity = communication_protocol::OperationId::generate();
    let wake_marker = format!("WAKE_{}", identity.as_str());
    let reply_marker = format!("REPLY_{}", identity.as_str());
    let acknowledgement = format!("ACK_{}", identity.as_str());
    let beta_done = format!("B_DONE_{}", identity.as_str());
    let cli = shell_quote(env!("CARGO_BIN_EXE_agent-sessions"));
    let directory = shell_quote(&proof.service_directory.to_string_lossy());
    let alpha_address = shell_quote(&serde_json::to_string(&alpha)?);
    let beta_address = shell_quote(&serde_json::to_string(&beta)?);
    let reply = format!(
        "{reply_marker}. This is the requested explicit peer reply. Do not call any tools or send another message. Output exactly {acknowledgement}."
    );
    let reply_file = proof.workspace.join("peer-reply-message.txt");
    std::fs::write(&reply_file, &reply)?;
    let reply_command = format!(
        "{cli} message send --from {beta_address} --to {alpha_address} --text-file {} --service-directory {directory} --json",
        shell_quote(&reply_file.to_string_lossy())
    );
    let beta_task = format!(
        "{wake_marker}\nExecute this exact CLI command once to reply to Luna A:\n{reply_command}\nAfter successful CLI acceptance output exactly {beta_done}. If the command fails, report its exact failure and do not resend. Do not use a subagent or another communication mechanism."
    );
    let wake_file = proof.workspace.join("peer-wake-message.txt");
    std::fs::write(&wake_file, &beta_task)?;
    let wake_command = format!(
        "{cli} wake send --from {alpha_address} --to {beta_address} --text-file {} --after 1s --wait-until-first-fire --operation-id {} --service-directory {directory} --json",
        shell_quote(&wake_file.to_string_lossy()),
        identity.as_str()
    );
    let permission_script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../scripts/proof-tools/socket-permission-probe.py")
        .canonicalize()?;
    let permission_command = format!(
        "{} {} {} {forbidden_port}",
        shell_quote(&python.to_string_lossy()),
        shell_quote(&permission_script.to_string_lossy()),
        shell_quote(&forbidden_socket.to_string_lossy())
    );
    let alpha_task = format!(
        "Create one timed message for Luna B using the actual CLI from your sandbox. First execute this permission probe and require exit zero with SOCKET_BOUNDARY_OK and TCP_BOUNDARY_OK: {permission_command}\nIf it fails, stop and report. Then run {cli} wake send --help. Then execute exactly once:\n{wake_command}\nReport the firing receipt. If any CLI command fails, report its error without retrying. Do not use subagents or another communication mechanism. A peer reply may arrive afterward; follow its acknowledgement instruction."
    );
    let initial = proof
        .client
        .send_agent_message(NativeSendParams {
            target: alpha.clone(),
            generation: proof.generation.clone(),
            message: MessageContent::Agent {
                sender: alpha.clone(),
                text: alpha_task.try_into()?,
            },
            delivery: MessageDelivery::Auto,
            client_user_message_id: None,
        })
        .await?;
    proof.record("alphaInputAccepted", json!(initial))?;
    let initial_turn = match &initial.acceptance {
        communication_protocol::NativeSendAcceptance::NativeInputAccepted { turn_id, .. }
        | communication_protocol::NativeSendAcceptance::SteerAccepted { turn_id, .. } => {
            String::from(turn_id.clone())
        }
        _ => return Err("Initial sender did not receive a native turn identity".into()),
    };
    proof
        .wait_for_arranged_wake(&alpha, &initial_turn, &wake_marker)
        .await?;
    let beta_turns = proof.wait_for_text(&beta, &beta_done).await?;
    let alpha_turns = proof.wait_for_text(&alpha, &acknowledgement).await?;
    let permissions_verified = alpha_turns
        .iter()
        .filter_map(|turn| turn.get("items").and_then(Value::as_array))
        .flatten()
        .filter(|item| {
            item.get("type").and_then(Value::as_str) == Some("commandExecution")
                && item.get("exitCode").and_then(Value::as_i64) == Some(0)
        })
        .filter_map(|item| item.get("aggregatedOutput").and_then(Value::as_str))
        .any(|output| output.contains("SOCKET_BOUNDARY_OK") && output.contains("TCP_BOUNDARY_OK"));
    if !permissions_verified {
        return Err(
            "No actual sandbox evidence retained for forbidden socket and TCP denial".into(),
        );
    }
    let explicit_reply = beta_turns
        .iter()
        .filter_map(|turn| turn.get("items").and_then(Value::as_array))
        .flatten()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("commandExecution"))
        .filter(|item| item.get("exitCode").and_then(Value::as_i64) == Some(0))
        .filter_map(|item| item.get("aggregatedOutput").and_then(Value::as_str))
        .flat_map(str::lines)
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|output| output.get("result").cloned())
        .filter_map(|result| serde_json::from_value::<NativeSendReceipt>(result).ok())
        .find(|receipt| receipt.target == alpha)
        .ok_or(
            "B emitted a completion marker but no successful real CLI reply receipt was observed",
        )?;
    let wakes = proof
        .client
        .list_wakeups(AutomationPageRequest {
            cursor: None,
            limit: 100.try_into()?,
        })
        .await?;
    let wake = wakes
        .records
        .iter()
        .find(|wake| match &wake.definition.message.content {
            MessageContent::Agent { text, .. } => text.as_str().contains(&wake_marker),
            _ => false,
        })
        .ok_or("A did not create its durable wake through the CLI")?;
    if wake.first_fire.is_none() {
        return Err("wake has no first-firing evidence".into());
    }
    let deliveries = proof
        .client
        .list_deliveries(DeliveryListRequest {
            wakeup_id: Some(wake.definition.wakeup_id.clone()),
            cursor: None,
            limit: 100.try_into()?,
        })
        .await?;
    if !deliveries.records.iter().any(|delivery| {
        delivery.target == beta && matches!(delivery.evidence, DeliveryEvidence::Accepted { .. })
    }) {
        return Err(
            "firing was mistaken for native acceptance; delivery lacks an accepted receipt".into(),
        );
    }
    let incoming_prefix = format!(
        "Agent communication\nSelf-declared sender: {}\nIntended recipient: {}\n\n{reply_marker}",
        serde_json::to_string(&beta)?,
        serde_json::to_string(&alpha)?
    );
    if !alpha_turns.iter().any(|turn| {
        turn.get("items")
            .and_then(Value::as_array)
            .is_some_and(|items| {
                items.iter().enumerate().any(|(index, item)| {
                    let incoming = item.get("type").and_then(Value::as_str) == Some("userMessage")
                        && item
                            .get("content")
                            .and_then(Value::as_array)
                            .is_some_and(|content| {
                                content.iter().any(|input| {
                                    input
                                        .get("text")
                                        .and_then(Value::as_str)
                                        .is_some_and(|text| text.starts_with(&incoming_prefix))
                                })
                            });
                    incoming
                        && items.iter().skip(index + 1).any(|later| {
                            later.get("type").and_then(Value::as_str) == Some("agentMessage")
                                && later
                                    .get("text")
                                    .and_then(Value::as_str)
                                    .is_some_and(|text| text.contains(&acknowledgement))
                        })
                })
            })
    }) {
        return Err(
            "A's acknowledgement did not follow the actual incoming declared peer message".into(),
        );
    }
    proof.record("agentCliRoundTripVerified", json!({"alpha":alpha,"beta":beta,"wakeupId":wake.definition.wakeup_id,"firstFire":wake.first_fire,"explicitReplyReceipt":explicit_reply,"deliveries":deliveries.records,"acknowledgement":acknowledgement}))?;
    proof.client.close().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires an isolated debug Host; reads only a thread created by an earlier proof"]
async fn recorded_test_history_is_readable_without_resume() -> ProofResult<()> {
    let mut proof = ProofContext::connect().await?;
    let source = std::path::PathBuf::from(
        std::env::var_os("CODEX_AUTOMATION_SOURCE_PROOF_ROOT")
            .ok_or("source proof root required")?,
    )
    .canonicalize()?;
    if source.parent() != proof.root.parent() {
        return Err("Source must be a sibling private proof root under /tmp".into());
    }
    let context: Value =
        serde_json::from_slice(&std::fs::read(source.join("debug-host-context.json"))?)?;
    if context.get("profile").and_then(Value::as_str) != Some("codex-router-debug")
        || context.get("model").and_then(Value::as_str) != Some("gpt-5.6-luna")
    {
        return Err("Source was not a Luna debug proof".into());
    }
    let events = std::fs::read_to_string(source.join("proof-events.jsonl"))?;
    let target: communication_protocol::SessionRef = events
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|event| {
            event.get("event").and_then(Value::as_str) == Some("freshThread")
                && event.pointer("/details/role").and_then(Value::as_str) == Some("Luna A")
        })
        .and_then(|event| event.pointer("/details/target").cloned())
        .map(serde_json::from_value)
        .transpose()?
        .ok_or("fresh Alpha identity missing")?;
    let result = proof
        .native
        .request_validated(
            &proof.schemas,
            codex_native_integration::NativeOperation::ReadThread,
            json!({"threadId":String::from(target.session_id.clone()),"includeTurns":true}),
        )
        .await;
    let result = match result {
        Ok(result) => result,
        Err(error) => {
            let rejection = proof.native.take_last_rejection();
            proof.record(
                "threadReadRejected",
                json!({"target":target,"rejection":rejection}),
            )?;
            return Err(error.into());
        }
    };
    if result.pointer("/thread/id").and_then(Value::as_str)
        != Some(String::from(target.session_id.clone()).as_str())
        || result
            .pointer("/thread/turns")
            .and_then(Value::as_array)
            .is_none_or(Vec::is_empty)
    {
        return Err("thread/read did not return the exact saved test thread and its turns".into());
    }
    proof.record(
        "readOnlySavedHistoryVerified",
        json!({"target":target,"response":result}),
    )?;
    proof.client.close().await?;
    Ok(())
}

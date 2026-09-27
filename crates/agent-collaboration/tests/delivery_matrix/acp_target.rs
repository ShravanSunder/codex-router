//! Recipient-observed matrix for a Router-hosted scripted ACP session.
use super::{
    board_listen_push_targets, cli_send, cli_send_receipt,
    delivery_matrix_support::ConfigHashGuard,
    matrix_marker, mcp_send,
    proof_context::{ProofContext, ProofResult},
    schedule_send, wake_send,
};
use collaboration_client::{
    ConversationClient, ConversationCreateActor, ConversationCreateInput,
    ConversationOperationResult, ConversationPromptInput, PublicPromptContent,
    board::Identity,
    protocol::{
        ApprovalDecideParams, ConversationCreateOutcome, OperationId, RouterAccess, SessionRef,
    },
};
use serde_json::{Value, json};
use std::{path::Path, time::Duration};
use tokio::io::AsyncWriteExt as _;
use tokio_util::sync::CancellationToken;

pub(super) async fn exercise_acp_target_matrix(config_guard: &ConfigHashGuard) -> ProofResult<()> {
    let mut proof = ProofContext::connect().await?;
    let sender = proof.start_thread("ACP matrix sender").await?;
    let target = create_provider_session(&mut proof, &sender, "cursor-local", &sender).await?;
    let receipt_path = proof.root.join("provider-prompt-receipts.jsonl");
    let mut cells = Vec::new();
    let mut markers = Vec::new();

    for producer in ["cliMessageSend", "mcpMessageSend"] {
        let marker = matrix_marker(producer, "providerAcp");
        if producer == "cliMessageSend" {
            cli_send(&proof, &sender, &target, &marker).await?;
        } else {
            mcp_send(&proof, &sender, &target, &marker).await?;
        }
        observe_single_prompt(&receipt_path, &target, &marker, Duration::from_secs(90)).await?;
        record_cell(
            &mut proof,
            &mut cells,
            &mut markers,
            config_guard,
            producer,
            &marker,
        )?;
    }

    let marker = matrix_marker("wakeSend", "providerAcp");
    wake_send(&proof, &sender, &target, &marker).await?;
    observe_single_prompt(&receipt_path, &target, &marker, Duration::from_secs(90)).await?;
    record_cell(
        &mut proof,
        &mut cells,
        &mut markers,
        config_guard,
        "wakeSend",
        &marker,
    )?;

    let marker = matrix_marker("scheduledRun", "providerAcp");
    schedule_send(&mut proof, &target, &marker).await?;
    observe_single_prompt(&receipt_path, &target, &marker, Duration::from_secs(150)).await?;
    record_cell(
        &mut proof,
        &mut cells,
        &mut markers,
        config_guard,
        "scheduledRun",
        &marker,
    )?;

    let marker = matrix_marker("boardListen", "providerAcp");
    board_listen_push_targets(&mut proof, &sender, &[(&target, &marker)]).await?;
    observe_single_prompt(&receipt_path, &target, &marker, Duration::from_secs(400)).await?;
    record_cell(
        &mut proof,
        &mut cells,
        &mut markers,
        config_guard,
        "boardListen",
        &marker,
    )?;

    let request_id = deliver_pending_approval(&mut proof, &sender, &target, &receipt_path).await?;
    record_cell(
        &mut proof,
        &mut cells,
        &mut markers,
        config_guard,
        "approvalNotice",
        &request_id,
    )?;

    let gate_path = proof.root.join("busy-prompt-gate.sock");
    let gate = tokio::net::UnixListener::bind(&gate_path)?;
    let hold_marker = matrix_marker("busyHold", "providerAcp");
    let busy_marker = matrix_marker("busyCliMessageSend", "providerAcp");
    let hold_text = hold_marker.clone().try_into()?;
    let service_directory = proof.service_directory.clone();
    let endpoint = target.endpoint.clone();
    let target_for_prompt = target.clone();
    let sender_for_prompt = sender.clone();
    let cancellation = CancellationToken::new();
    let _cancel_on_drop = cancellation.clone().drop_guard();
    let mut held_prompt = tokio::spawn(async move {
        let conversation = ConversationClient::connect(&service_directory, &endpoint).await?;
        conversation
            .prompt(
                ConversationPromptInput {
                    operation_id: Some(OperationId::generate()),
                    target: target_for_prompt,
                    working_directory: None,
                    requested_by: sender_for_prompt.clone(),
                    approver: Some(sender_for_prompt.clone()),
                    message: PublicPromptContent::Agent {
                        sender: sender_for_prompt,
                        text: hold_text,
                    },
                    effort: None,
                    generation: None,
                },
                Duration::from_secs(120),
                cancellation,
            )
            .await
    });
    observe_single_prompt(
        &receipt_path,
        &target,
        &hold_marker,
        Duration::from_secs(60),
    )
    .await?;
    let (mut gate_stream, _) =
        tokio::time::timeout(Duration::from_secs(30), gate.accept()).await??;
    let busy_receipt = cli_send_receipt(&proof, &sender, &target, &busy_marker).await?;
    if busy_receipt.pointer("/outcome/kind") != Some(&json!("queued")) {
        return Err(format!("Busy provider auto delivery did not queue: {busy_receipt}").into());
    }
    if prompt_count(&receipt_path, &target, &busy_marker)? != 0 {
        return Err("Busy input reached the provider before its active turn settled".into());
    }
    gate_stream.write_all(b"release").await?;
    match tokio::time::timeout(Duration::from_secs(90), &mut held_prompt).await? {
        Ok(Ok(ConversationOperationResult::Completed { .. })) => {}
        other => return Err(format!("Held provider prompt did not settle: {other:?}").into()),
    }
    observe_single_prompt(
        &receipt_path,
        &target,
        &busy_marker,
        Duration::from_secs(90),
    )
    .await?;
    markers.push(hold_marker);
    record_cell(
        &mut proof,
        &mut cells,
        &mut markers,
        config_guard,
        "busyCliMessageSend",
        &busy_marker,
    )?;

    assert_exactly_once(&receipt_path, &target, &markers)?;
    proof.record(
        "providerAcpDeliveryMatrix",
        json!({"cells":cells,"target":target}),
    )?;
    proof.client.close().await?;
    Ok(())
}

async fn create_provider_session(
    proof: &mut ProofContext,
    creator: &SessionRef,
    endpoint_id: &str,
    approver: &SessionRef,
) -> ProofResult<SessionRef> {
    let endpoint = proof
        .client
        .list_endpoints()
        .await?
        .endpoints
        .into_iter()
        .find(|entry| String::from(entry.endpoint.endpoint_id.clone()) == endpoint_id)
        .ok_or("Scripted ACP endpoint missing")?
        .endpoint;
    let conversation = ConversationClient::connect(&proof.service_directory, &endpoint).await?;
    match conversation
        .create(
            ConversationCreateInput {
                operation_id: OperationId::generate(),
                endpoint,
                working_directory: proof.workspace.clone(),
                access: RouterAccess::WriteRestricted,
                created_by: ConversationCreateActor::Session(creator.clone()),
                approver: Some(ConversationCreateActor::Session(approver.clone())),
                generation: None,
                model: None,
                mode: None,
                effort: None,
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
            Err("Scripted ACP create stayed pending".into())
        }
    }
}

async fn deliver_pending_approval(
    proof: &mut ProofContext,
    creator: &SessionRef,
    approver: &SessionRef,
    receipt_path: &Path,
) -> ProofResult<String> {
    let requester = create_provider_session(proof, creator, "claude-local", approver).await?;
    let service_directory = proof.service_directory.clone();
    let endpoint = requester.endpoint.clone();
    let requester_for_prompt = requester.clone();
    let creator_for_prompt = creator.clone();
    let approver_for_prompt = approver.clone();
    let prompt_text = "Request matrix permission and await its decision"
        .to_owned()
        .try_into()?;
    let cancellation = CancellationToken::new();
    let _cancel_on_drop = cancellation.clone().drop_guard();
    let prompt_task = tokio::spawn(async move {
        let conversation = ConversationClient::connect(&service_directory, &endpoint).await?;
        conversation
            .prompt(
                ConversationPromptInput {
                    operation_id: Some(OperationId::generate()),
                    target: requester_for_prompt,
                    working_directory: None,
                    requested_by: creator_for_prompt.clone(),
                    approver: Some(approver_for_prompt),
                    message: PublicPromptContent::Agent {
                        sender: creator_for_prompt,
                        text: prompt_text,
                    },
                    effort: None,
                    generation: None,
                },
                Duration::from_secs(120),
                cancellation,
            )
            .await
    });
    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    let request_id = loop {
        let pending = proof.client.list_pending_approvals(true).await?;
        if let Some(record) = pending.approvals.iter().find(|record| {
            record.requester == *creator
                && record.approver == *approver
                && record.operation.get("target") == Some(&json!(requester))
        }) {
            break record.request_id.clone();
        }
        if prompt_task.is_finished() || tokio::time::Instant::now() >= deadline {
            return Err("Provider approval did not become pending".into());
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    };
    observe_single_prompt(receipt_path, approver, &request_id, Duration::from_secs(90)).await?;
    let still_pending = proof
        .client
        .list_pending_approvals(true)
        .await?
        .approvals
        .iter()
        .any(|record| record.request_id == request_id && record.approver == *approver);
    if !still_pending {
        return Err("Approval notice reached ACP recipient after approval ceased pending".into());
    }
    proof
        .client
        .decide_approval(ApprovalDecideParams {
            request_id: request_id.clone(),
            decision: None,
            option_id: Some("deny-once".to_owned()),
            note: None,
            acknowledge_persistent: false,
            actor: Identity::Session {
                session: serde_json::from_value(json!(approver))?,
            },
        })
        .await?;
    match tokio::time::timeout(Duration::from_secs(90), prompt_task).await? {
        Ok(Ok(ConversationOperationResult::Completed { .. })) => Ok(request_id),
        other => Err(format!("Approval requester did not settle: {other:?}").into()),
    }
}

fn prompt_count(path: &Path, target: &SessionRef, marker: &str) -> ProofResult<usize> {
    let contents = match std::fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error.into()),
    };
    let mut count = 0;
    for line in contents.lines() {
        let frame: Value = serde_json::from_str(line)?;
        if frame["method"] != "session/prompt"
            || frame["params"]["sessionId"] != json!(String::from(target.session_id.clone()))
        {
            return Err("ACP recipient log contains another session or method".into());
        }
        if frame["params"]["prompt"].as_array().is_some_and(|parts| {
            parts.iter().any(|part| {
                part["text"]
                    .as_str()
                    .is_some_and(|text| super::user_text_contains_marker(text, marker))
            })
        }) {
            count += 1;
        }
    }
    Ok(count)
}

async fn observe_single_prompt(
    path: &Path,
    target: &SessionRef,
    marker: &str,
    timeout: Duration,
) -> ProofResult<()> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        match prompt_count(path, target, marker)? {
            1 => return Ok(()),
            count if count > 1 => {
                return Err(format!("ACP marker {marker} delivered {count} times").into());
            }
            _ if tokio::time::Instant::now() >= deadline => {
                return Err(format!("ACP recipient did not receive marker {marker}").into());
            }
            _ => tokio::time::sleep(Duration::from_millis(250)).await,
        }
    }
}

fn assert_exactly_once(path: &Path, target: &SessionRef, markers: &[String]) -> ProofResult<()> {
    for marker in markers {
        let count = prompt_count(path, target, marker)?;
        if count != 1 {
            return Err(format!("ACP marker {marker} appeared in {count} prompts").into());
        }
    }
    Ok(())
}

fn record_cell(
    proof: &mut ProofContext,
    cells: &mut Vec<Value>,
    markers: &mut Vec<String>,
    config_guard: &ConfigHashGuard,
    producer: &str,
    marker: &str,
) -> ProofResult<()> {
    markers.push(marker.to_owned());
    cells.push(
        json!({"producer":producer,"target":"providerAcp","status":"pass",
        "evidence":"one observed session/prompt frame","marker":marker}),
    );
    proof.record(
        "providerAcpCell",
        cells.last().cloned().ok_or("missing cell")?,
    )?;
    config_guard.verify()
}

#[test]
fn recipient_log_counts_prompt_frames_and_rejects_duplicate_delivery() -> ProofResult<()> {
    use std::io::Write as _;
    let target: SessionRef = serde_json::from_value(json!({
        "endpoint":{"serviceId":"0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89","endpointId":"cursor-local"},
        "sessionId":"matrix-acp-target"
    }))?;
    let marker = "DELIVERY_MATRIX_unique_providerAcp";
    let frame = json!({"jsonrpc":"2.0","id":1,"method":"session/prompt","params":{
        "sessionId":"matrix-acp-target","prompt":[{"type":"text","text":marker}]
    }});
    let mut log = tempfile::NamedTempFile::new()?;
    writeln!(log, "{frame}")?;
    if prompt_count(log.path(), &target, marker)? != 1 {
        return Err("One prompt frame was not counted once".into());
    }
    writeln!(log, "{frame}")?;
    if prompt_count(log.path(), &target, marker)? != 2
        || assert_exactly_once(log.path(), &target, &[marker.to_owned()]).is_ok()
    {
        return Err("Duplicate prompt frames were not rejected".into());
    }
    Ok(())
}

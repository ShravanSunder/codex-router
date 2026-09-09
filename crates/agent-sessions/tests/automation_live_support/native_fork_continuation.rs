//! A scheduled fork retains native context and executes independently of its source.
use super::proof_context::{ProofContext, ProofResult, agent_text};
use communication_protocol::{
    DestinationPreparation, ExecutionDestination, InstructionCreateParams, MessageContent,
    MessageDelivery, NativeSendAcceptance, NativeSendParams, OperationId, RunState,
    ScheduleCreateRequest, ScheduleDefinition, SchedulePrepareRequest, ScheduleUpdateRequest,
    TimingRequest, WorkerOutcome,
};
use serde_json::json;

pub async fn exercise() -> ProofResult<()> {
    let mut proof = ProofContext::connect().await?;
    let source = proof.start_thread("Luna fork source").await?;
    let token = format!("FORK_CONTEXT_{}", OperationId::generate().as_str());
    let receipt = proof.client.send_agent_message(NativeSendParams {
        target: source.clone(), generation: proof.generation.clone(),
        message: MessageContent::Agent { sender: source.clone(), text: format!("Remember this context token: {token}. Output exactly {token}. Do not call tools or spawn agents.").try_into()? },
        delivery: MessageDelivery::Auto, client_user_message_id: None,
    }).await?;
    let NativeSendAcceptance::NativeInputAccepted { turn_id, .. } = receipt.acceptance else {
        return Err("Fresh source did not accept a native turn".into());
    };
    proof.wait_for_text(&source, &token).await?;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut interval = tokio::time::interval(std::time::Duration::from_millis(100));
    loop {
        interval.tick().await;
        if proof.turns(&source).await?.iter().any(|turn| {
            turn.get("id").and_then(serde_json::Value::as_str)
                == Some(String::from(turn_id.clone()).as_str())
                && turn.get("status").and_then(serde_json::Value::as_str) == Some("completed")
        }) {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("Source turn did not finish before fork preparation".into());
        }
    }
    let instruction = proof.client.create_instruction(InstructionCreateParams {
        operation_id: OperationId::generate(),
        text: "Recover the FORK_CONTEXT token from the prior conversation and output 'FORK_RECOVERED: ' followed by that exact token. Do not invent a token, call tools or spawn agents.".to_owned().try_into()?,
    }).await?;
    let schedule = proof
        .client
        .create_schedule(ScheduleCreateRequest {
            operation_id: OperationId::generate(),
            definition: ScheduleDefinition {
                instruction_id: instruction.instruction_id,
                timing: TimingRequest::After {
                    seconds: 60.try_into()?,
                },
                enabled: false,
                destination: ExecutionDestination::Unprepared,
                execution_timeout_seconds: Some(120.try_into()?),
            },
        })
        .await?;
    let mut prepared = proof
        .client
        .prepare_schedule(SchedulePrepareRequest {
            operation_id: OperationId::generate(),
            schedule_id: schedule.schedule_id.clone(),
            destination: DestinationPreparation::Fork {
                source: source.clone(),
                through_turn_id: turn_id.clone(),
                cwd: proof.workspace.to_string_lossy().into_owned(),
            },
        })
        .await?;
    let ExecutionDestination::OwnedThread { target, .. } = &prepared.definition.destination else {
        return Err("Fork preparation did not establish an owned execution thread".into());
    };
    let target = target.clone();
    if target == source {
        return Err("Fork reused source identity".into());
    }
    proof.record(
        "nativeForkPrepared",
        json!({"source":source,"sourceTurn":turn_id,"schedule":prepared}),
    )?;
    prepared.definition.enabled = true;
    prepared.definition.timing = TimingRequest::At {
        at: (chrono::Utc::now() + chrono::Duration::seconds(2))
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
            .try_into()?,
    };
    proof
        .client
        .update_schedule(ScheduleUpdateRequest {
            operation_id: OperationId::generate(),
            schedule_id: prepared.schedule_id,
            expected_change_id: prepared.change_id,
            definition: prepared.definition,
        })
        .await?;
    let run =
        super::scheduled_continuity::wait_for_finished_run(&mut proof, &schedule.schedule_id, 1)
            .await?;
    let RunState::Finished {
        execution,
        outcome: WorkerOutcome::Completed { .. },
        ..
    } = &run.state
    else {
        return Err("Forked scheduled worker did not complete".into());
    };
    if execution.target != target || run.summary.is_some() {
        return Err(
            "Fork continuation changed destination or required an unnecessary summary".into(),
        );
    }
    let turns = proof.turns(&target).await?;
    if !turns.iter().any(|turn| {
        turn.get("id").and_then(serde_json::Value::as_str) == Some(execution.turn_id.as_str())
            && agent_text(turn).contains(&format!("FORK_RECOVERED: {token}"))
    }) {
        return Err("Fork execution did not recover source conversation context".into());
    }
    let source_turns = proof.turns(&source).await?;
    if source_turns.len() != 1
        || source_turns
            .first()
            .and_then(|turn| turn.get("id"))
            .and_then(serde_json::Value::as_str)
            != Some(String::from(turn_id).as_str())
    {
        return Err("Schedule changed the source thread's execution history".into());
    }
    proof.record(
        "nativeForkContinuationVerified",
        json!({"source":source,"fork":target,"run":run}),
    )?;
    proof.client.close().await?;
    Ok(())
}

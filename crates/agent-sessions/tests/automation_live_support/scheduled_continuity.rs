//! Real scheduled execution and continuity through the SDK, native threads and Luna summaries.
use super::proof_context::{ProofContext, ProofResult, agent_text};
use communication_protocol::{
    ContinuityInput, ExecutionDestination, InstructionCreateParams, InstructionUpdateParams,
    OperationId, RunListRequest, RunSnapshot, RunState, ScheduleCreateRequest, ScheduleDefinition,
    ScheduleId, ScheduleShowRequest, ScheduleUpdateRequest, TimingRequest, WorkerOutcome,
};
use serde_json::json;
use std::time::Duration;

pub async fn exercise() -> ProofResult<()> {
    let mut proof = ProofContext::connect().await?;
    let token = format!("CONTINUITY_{}", OperationId::generate().as_str());
    let instruction = proof.client.create_instruction(InstructionCreateParams {
        operation_id: OperationId::generate(),
        text: format!("The next scheduled run must recover this continuity token from your summary: {token}. Output exactly 'Continuity token: {token}'. Do not call tools, change files or spawn agents.").try_into()?,
    }).await?;
    let schedule = proof
        .client
        .create_schedule(ScheduleCreateRequest {
            operation_id: OperationId::generate(),
            definition: ScheduleDefinition {
                instruction_id: instruction.instruction_id.clone(),
                timing: TimingRequest::After {
                    seconds: 1.try_into()?,
                },
                enabled: true,
                destination: ExecutionDestination::FreshEachRun {
                    endpoint: proof.endpoint.clone(),
                    cwd: proof.workspace.to_string_lossy().into_owned(),
                },
                execution_timeout_seconds: Some(120.try_into()?),
            },
        })
        .await?;
    proof.record("freshScheduleCreated", json!(schedule))?;
    let first = wait_for_finished_run(&mut proof, &schedule.schedule_id, 1).await?;
    let summary = first
        .summary
        .as_ref()
        .ok_or("First workflow finished without its required summary")?;
    if summary.source_run_id != first.run_id || !summary.text.contains(&token) {
        proof.record("continuitySummaryRejected", json!(first))?;
        return Err(
            "Luna summary did not preserve its exact source Run and required continuity token"
                .into(),
        );
    }
    let RunState::Finished {
        execution: first_execution,
        outcome: WorkerOutcome::Completed { .. },
        ..
    } = &first.state
    else {
        return Err("First scheduled worker did not complete successfully".into());
    };
    proof.client.update_instruction(InstructionUpdateParams {
        operation_id: OperationId::generate(),
        instruction_id: instruction.instruction_id,
        expected_revision_id: instruction.revision_id,
        text: "Recover the required continuity token from the previous-run summary in your context. Output exactly 'RECOVERED: ' followed by that token. If no token is provided, output MISSING_CONTINUITY. Do not invent a token, call tools, change files or spawn agents.".to_owned().try_into()?,
    }).await?;
    let mut current = proof
        .client
        .read_schedule(ScheduleShowRequest {
            schedule_id: schedule.schedule_id.clone(),
        })
        .await?;
    current.definition.timing = TimingRequest::At {
        at: (chrono::Utc::now() + chrono::Duration::seconds(1))
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
            .try_into()?,
    };
    proof
        .client
        .update_schedule(ScheduleUpdateRequest {
            operation_id: OperationId::generate(),
            schedule_id: current.schedule_id,
            expected_change_id: current.change_id,
            definition: current.definition,
        })
        .await?;
    let second = wait_for_finished_run(&mut proof, &schedule.schedule_id, 2).await?;
    let RunState::Finished {
        inputs,
        execution,
        outcome: WorkerOutcome::Completed { .. },
        ..
    } = &second.state
    else {
        return Err("Second scheduled worker did not complete successfully".into());
    };
    if execution.target == first_execution.target || second.run_id == first.run_id {
        return Err(
            "Fresh scheduled execution reused its previous native thread or Run identity".into(),
        );
    }
    if inputs.instruction_text.as_str().contains(&token)
        || !matches!(&inputs.continuity, ContinuityInput::LocalSummary { text, source_run_id, source_target, source_turn_id }
            if text == &summary.text && source_run_id == &first.run_id
                && source_target == &first_execution.target && source_turn_id == &first_execution.turn_id)
    {
        return Err("Second Run did not capture the exact prior summary independently of its current instructions".into());
    }
    let turns = proof.turns(&execution.target).await?;
    let turn = turns
        .iter()
        .find(|turn| {
            turn.get("id").and_then(serde_json::Value::as_str) == Some(execution.turn_id.as_str())
        })
        .ok_or("Exact second worker turn missing from native history")?;
    if !agent_text(turn).contains(&format!("RECOVERED: {token}")) {
        proof.record(
            "nativeContinuityRejected",
            json!({"run":second,"turn":turn}),
        )?;
        return Err(
            "Fresh Luna worker did not actually recover the token from prior-summary context"
                .into(),
        );
    }
    proof.record(
        "scheduledContinuityVerified",
        json!({"first":first,"second":second}),
    )?;
    proof.client.close().await?;
    Ok(())
}

pub(super) async fn wait_for_finished_run(
    proof: &mut ProofContext,
    schedule_id: &ScheduleId,
    expected_count: usize,
) -> ProofResult<RunSnapshot> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
    let mut interval = tokio::time::interval(Duration::from_millis(200));
    let mut last_state = None;
    let mut saw_summary = false;
    loop {
        interval.tick().await;
        let page = proof
            .client
            .list_runs(RunListRequest {
                schedule_id: schedule_id.clone(),
                cursor: None,
                limit: 100.try_into()?,
            })
            .await?;
        if page.records.len() > expected_count || page.next_cursor.is_some() {
            return Err("One-shot schedule created unexpected additional Runs".into());
        }
        if page.records.len() == expected_count {
            let run = page.records.last().ok_or("Scheduled Run missing")?;
            let encoded = serde_json::to_value(&run.state)?;
            let state = encoded
                .get("kind")
                .and_then(serde_json::Value::as_str)
                .ok_or("Run phase missing")?
                .to_owned();
            if last_state.as_ref() != Some(&state) {
                proof.record("scheduledRunObserved", json!(run))?;
                last_state = Some(state);
            }
            match &run.state {
                RunState::SummaryRequired { .. } | RunState::SummaryRunning { .. } => {
                    let current = proof
                        .client
                        .read_schedule(ScheduleShowRequest {
                            schedule_id: schedule_id.clone(),
                        })
                        .await?;
                    if current.active_run_id.as_ref() == Some(&run.run_id) {
                        saw_summary = true;
                    } else {
                        // These are separate read snapshots; accept only an actually finished Run
                        // if it completed between the two reads, never a lost execution slot.
                        let latest = proof
                            .client
                            .read_run(communication_protocol::RunShowRequest {
                                run_id: run.run_id.clone(),
                            })
                            .await?;
                        if !matches!(latest.state, RunState::Finished { .. }) {
                            return Err(
                                "Required summarization lost the schedule's active Run".into()
                            );
                        }
                    }
                }
                RunState::Finished { .. } => {
                    if !saw_summary {
                        return Err(
                            "Live proof never observed required summary retaining Run occupancy"
                                .into(),
                        );
                    }
                    return Ok(run.clone());
                }
                RunState::PreparationFailed { .. }
                | RunState::SummaryBlocked { .. }
                | RunState::Uncertain { .. } => {
                    return Err(format!("Scheduled workflow cannot complete; inspect run {} and private proof events", run.run_id.as_str()).into());
                }
                _ => {}
            }
        }
        if tokio::time::Instant::now() >= deadline {
            proof.record("scheduledRunWaitExpired", json!({"runs":page.records}))?;
            return Err(
                "Scheduled workflow did not finish within the bounded proof deadline".into(),
            );
        }
    }
}

//! A real native worker is stopped by its captured schedule timeout, not by the test driver.
use super::proof_context::{ProofContext, ProofResult};
use communication_protocol::{
    CessationEvidence, DestinationPreparation, ExecutionDestination, InstructionCreateParams,
    OperationId, RunListRequest, RunState, ScheduleCreateRequest, ScheduleDefinition,
    ScheduleEnableRequest, SchedulePrepareRequest, TimingRequest, WorkerOutcome,
};
use serde_json::{Value, json};
use std::time::Duration;

pub async fn exercise(proof: &mut ProofContext) -> ProofResult<()> {
    let status = proof.client.automation_status().await?;
    let configuration = status
        .configuration
        .ok_or("Fresh automation defaults unavailable")?;
    if !status.storage_available
        || u32::from(configuration.execution_timeout_seconds) != 3600
        || u32::from(configuration.summary_timeout_seconds) != 900
    {
        return Err(
            "Fresh service did not expose the required one-hour and fifteen-minute defaults".into(),
        );
    }
    let target = proof.start_thread("Luna timed worker").await?;
    let instructions = proof.client.create_instruction(InstructionCreateParams {
        operation_id: OperationId::generate(),
        text: "Execute exactly /bin/sleep 20 using the shell tool, then output WORKER_FINISHED_SLEEP. Do not perform other work or spawn agents.".to_owned().try_into()?,
    }).await?;
    let schedule = proof
        .client
        .create_schedule(ScheduleCreateRequest {
            operation_id: OperationId::generate(),
            definition: ScheduleDefinition {
                instruction_id: instructions.instruction_id,
                timing: TimingRequest::After {
                    seconds: 5.try_into()?,
                },
                enabled: false,
                destination: ExecutionDestination::Unprepared,
                execution_timeout_seconds: Some(2.try_into()?),
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
    let schedule = proof
        .client
        .enable_schedule(ScheduleEnableRequest {
            operation_id: OperationId::generate(),
            schedule_id: schedule.schedule_id,
        })
        .await?;
    if schedule.next_due_at.is_none() {
        return Err("Timed worker setup missed its one-shot due time".into());
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    let mut interval = tokio::time::interval(Duration::from_millis(100));
    let mut last_state = None;
    loop {
        interval.tick().await;
        let page = proof
            .client
            .list_runs(RunListRequest {
                schedule_id: schedule.schedule_id.clone(),
                cursor: None,
                limit: 100.try_into()?,
            })
            .await?;
        if page.records.len() > 1 || page.next_cursor.is_some() {
            return Err("Timed one-shot allocated extra Runs".into());
        }
        if let Some(run) = page.records.first() {
            let encoded = serde_json::to_value(&run.state)?;
            let state = encoded
                .get("kind")
                .and_then(Value::as_str)
                .ok_or("Run phase missing")?
                .to_owned();
            if last_state.as_ref() != Some(&state) {
                proof.record("timedWorkerObserved", json!(run))?;
                last_state = Some(state);
            }
            match &run.state {
                RunState::Finished {
                    execution,
                    outcome: WorkerOutcome::Interrupted { .. },
                    ..
                } => {
                    if execution.target != target
                        || u32::from(execution.effective_timeout_seconds) != 2
                        || !matches!(
                            run.execution_evidence.native.cessation,
                            CessationEvidence::Confirmed
                        )
                        || run.summary.is_some()
                    {
                        return Err("Timed worker lost its exact target, override, cessation or continued-thread semantics".into());
                    }
                    let start = chrono::DateTime::parse_from_rfc3339(&String::from(
                        execution.started_at.clone(),
                    ))?;
                    let end = chrono::DateTime::parse_from_rfc3339(&String::from(
                        execution.deadline_at.clone(),
                    ))?;
                    if (end - start).num_milliseconds() != 2000 {
                        return Err(
                            "Captured native dispatch budget differs from the two-second override"
                                .into(),
                        );
                    }
                    let turns = proof.turns(&target).await?;
                    if !turns.iter().any(|turn| {
                        turn.get("id").and_then(Value::as_str) == Some(execution.turn_id.as_str())
                            && turn.get("status").and_then(Value::as_str) == Some("interrupted")
                    }) {
                        return Err("Native history did not confirm interruption of the exact timed worker turn".into());
                    }
                    proof.record("nativeWorkerTimeoutVerified", json!(run))?;
                    return Ok(());
                }
                RunState::Finished { .. }
                | RunState::PreparationFailed { .. }
                | RunState::Uncertain { .. } => {
                    return Err(
                        "Native timeout scenario failed; inspect timed worker evidence".into(),
                    );
                }
                _ => {}
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("Timed native worker did not reach confirmed cessation".into());
        }
    }
}

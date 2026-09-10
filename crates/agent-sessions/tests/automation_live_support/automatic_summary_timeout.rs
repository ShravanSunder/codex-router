//! Only the service timer requests interruption; the test observes the exact summary turn.
use super::proof_context::{ProofContext, ProofResult, agent_text};
use communication_protocol::{
    AutomationConfigureRequest, CessationEvidence, ExecutionDestination, InstructionCreateParams,
    OperationId, RunListRequest, RunState, RunSummariesRequest, ScheduleCreateRequest,
    ScheduleDefinition, ScheduleShowRequest, SummaryInspectionState, TimingRequest, WorkerOutcome,
};
use serde_json::{Value, json};
use std::time::Duration;
const SUMMARY_TIMEOUT_SECONDS: u32 = 5;

pub async fn exercise() -> ProofResult<()> {
    let mut proof = ProofContext::connect().await?;
    proof
        .client
        .configure_automation(AutomationConfigureRequest {
            operation_id: OperationId::generate(),
            execution_timeout_seconds: 120.try_into()?,
            summary_timeout_seconds: SUMMARY_TIMEOUT_SECONDS.try_into()?,
        })
        .await?;
    let instruction = proof
        .client
        .create_instruction(InstructionCreateParams {
            operation_id: OperationId::generate(),
            text: "Output exactly SUMMARY_TIMEOUT_WORKER_DONE. Do not call tools or spawn agents."
                .to_owned()
                .try_into()?,
        })
        .await?;
    let schedule = proof
        .client
        .create_schedule(ScheduleCreateRequest {
            operation_id: OperationId::generate(),
            definition: ScheduleDefinition {
                instruction_id: instruction.instruction_id,
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
    let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
    let mut interval = tokio::time::interval(Duration::from_millis(100));
    let mut last_state = None;
    let blocked = loop {
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
            return Err("Summary timeout one-shot allocated additional Runs".into());
        }
        if let Some(run) = page.records.first() {
            let state = serde_json::to_value(&run.state)?;
            let state = state
                .get("kind")
                .and_then(Value::as_str)
                .ok_or("Run phase missing")?
                .to_owned();
            if last_state.as_ref() != Some(&state) {
                proof.record("automaticSummaryTimeoutObserved", json!(run))?;
                last_state = Some(state);
            }
            match &run.state {
                RunState::SummaryBlocked {
                    outcome: WorkerOutcome::Completed { .. },
                    ..
                } => break run.clone(),
                RunState::Finished { .. }
                | RunState::PreparationFailed { .. }
                | RunState::Uncertain { .. } => {
                    return Err("Automatic summary timeout was not established; no manual interrupt or retry issued".into());
                }
                _ => {}
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(
                "Summary timer did not reach confirmed cessation within proof deadline".into(),
            );
        }
    };
    let RunState::SummaryBlocked {
        execution,
        outcome: WorkerOutcome::Completed { .. },
        ..
    } = &blocked.state
    else {
        return Err("Summary failure changed the worker outcome".into());
    };
    let attempts = proof
        .client
        .read_run_summaries(RunSummariesRequest {
            run_id: blocked.run_id.clone(),
            cursor: None,
            limit: 100.try_into()?,
        })
        .await?;
    let [summary] = attempts.records.as_slice() else {
        return Err("Expected exactly one summary attempt".into());
    };
    proof.record("automaticSummaryTimeoutAttemptInspected", json!(summary))?;
    if u32::from(summary.effective_timeout_seconds) != SUMMARY_TIMEOUT_SECONDS
        || !matches!(summary.state, SummaryInspectionState::Failed)
        || !matches!(summary.cessation, CessationEvidence::Confirmed)
        || !summary.retry_eligible
        || blocked.summary.is_some()
    {
        return Err(
            "Summary timeout lost captured budget, confirmed cessation or worker exclusion".into(),
        );
    }
    let target = summary.target.as_ref().ok_or("Summary target missing")?;
    let turn_id = summary
        .native_turn_id
        .as_ref()
        .ok_or("Summary turn missing")?;
    if target == &execution.target {
        return Err("Summary reused the worker's native thread".into());
    }
    let started = chrono::DateTime::parse_from_rfc3339(&String::from(
        summary.started_at.clone().ok_or("Summary start missing")?,
    ))?;
    let expires = chrono::DateTime::parse_from_rfc3339(&String::from(
        summary
            .deadline_at
            .clone()
            .ok_or("Summary deadline missing")?,
    ))?;
    if (expires - started).num_milliseconds() != i64::from(SUMMARY_TIMEOUT_SECONDS) * 1000
        || chrono::Utc::now() < expires
    {
        return Err("Summary failed outside its captured timeout boundary".into());
    }
    let turns = proof.turns(target).await?;
    if !turns.iter().any(|turn| {
        turn.get("id").and_then(Value::as_str) == Some(turn_id)
            && turn.get("status").and_then(Value::as_str) == Some("interrupted")
    }) {
        return Err("Native history did not confirm interruption of the exact summary turn".into());
    }
    let worker = proof.turns(&execution.target).await?;
    if !worker.iter().any(|turn| {
        turn.get("id").and_then(Value::as_str) == Some(execution.turn_id.as_str())
            && turn.get("status").and_then(Value::as_str) == Some("completed")
            && agent_text(turn).contains("SUMMARY_TIMEOUT_WORKER_DONE")
    }) {
        return Err("Worker completion was not independently verified from native history".into());
    }
    let current = proof
        .client
        .read_schedule(ScheduleShowRequest {
            schedule_id: schedule.schedule_id,
        })
        .await?;
    if current.active_run_id.as_ref() != Some(&blocked.run_id) {
        return Err("Timed-out required summary released the schedule slot".into());
    }
    proof.record(
        "automaticNativeSummaryTimeoutVerified",
        json!({"run":blocked,"summary":summary}),
    )?;
    proof.client.close().await?;
    Ok(())
}

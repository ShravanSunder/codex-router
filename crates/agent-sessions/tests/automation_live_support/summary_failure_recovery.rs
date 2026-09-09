//! A failed native summary preserves the worker result and requires explicit same-Run recovery.
use super::proof_context::{ProofContext, ProofResult};
use communication_protocol::{
    AutomationConfigureRequest, CessationEvidence, ExecutionDestination, InstructionCreateParams,
    OperationId, RunListRequest, RunRecoveryRequest, RunSnapshot, RunState, RunSummariesRequest,
    ScheduleCreateRequest, ScheduleDefinition, ScheduleId, ScheduleShowRequest,
    SummaryInspectionState, TimingRequest, WorkerOutcome,
};
use serde_json::json;
use std::time::Duration;

pub struct PortableRunProof {
    pub schedule_id: ScheduleId,
    pub source_change_id: communication_protocol::ChangeId,
    pub package_utf8: String,
    pub summary_text: String,
}

pub async fn exercise(proof: &mut ProofContext) -> ProofResult<PortableRunProof> {
    proof
        .client
        .configure_automation(AutomationConfigureRequest {
            operation_id: OperationId::generate(),
            execution_timeout_seconds: 120.try_into()?,
            summary_timeout_seconds: 120.try_into()?,
        })
        .await?;
    let instruction = proof
        .client
        .create_instruction(InstructionCreateParams {
            operation_id: OperationId::generate(),
            text: "Output exactly ORIGINAL_WORKER_RESULT. Do not call tools or spawn agents."
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
    let mut interrupted_attempt = None;
    let blocked = loop {
        interval.tick().await;
        let run = current_run(proof, &schedule.schedule_id).await?;
        if let Some(run) = run {
            match &run.state {
                RunState::SummaryRunning {
                    outcome: WorkerOutcome::Completed { .. },
                    summary_attempt_id,
                    ..
                } if interrupted_attempt.is_none() => {
                    let attempts = proof
                        .client
                        .read_run_summaries(RunSummariesRequest {
                            run_id: run.run_id.clone(),
                            cursor: None,
                            limit: 100.try_into()?,
                        })
                        .await?;
                    if let Some(attempt) = attempts.records.iter().find(|attempt| {
                        &attempt.summary_attempt_id == summary_attempt_id
                            && matches!(attempt.state, SummaryInspectionState::Running)
                    }) {
                        let target = attempt
                            .target
                            .as_ref()
                            .ok_or("Running summary target missing")?;
                        let turn_id = attempt
                            .native_turn_id
                            .as_ref()
                            .ok_or("Running summary turn missing")?;
                        let receipt = proof
                            .client
                            .interrupt_turn(target, &proof.generation, turn_id)
                            .await?;
                        proof.record(
                            "exactSummaryInterruptionRequested",
                            json!({"attempt":attempt,"receipt":receipt}),
                        )?;
                        interrupted_attempt = Some(attempt.summary_attempt_id.clone());
                    }
                }
                RunState::SummaryBlocked {
                    outcome: WorkerOutcome::Completed { .. },
                    ..
                } if interrupted_attempt.is_some() => break run,
                RunState::Finished { .. }
                | RunState::PreparationFailed { .. }
                | RunState::Uncertain { .. } => {
                    proof.record("unexpectedSummaryRecoveryState", json!(run))?;
                    return Err(
                        "Summary failure scenario was not established; no retry issued".into(),
                    );
                }
                _ => {}
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("Summary interruption did not reach a safely blocked state".into());
        }
    };
    let current = proof
        .client
        .read_schedule(ScheduleShowRequest {
            schedule_id: schedule.schedule_id.clone(),
        })
        .await?;
    if current.active_run_id.as_ref() != Some(&blocked.run_id) || blocked.summary.is_some() {
        return Err("Failed required summary lost occupancy or fabricated summary text".into());
    }
    let attempts = proof
        .client
        .read_run_summaries(RunSummariesRequest {
            run_id: blocked.run_id.clone(),
            cursor: None,
            limit: 100.try_into()?,
        })
        .await?;
    let [failed] = attempts.records.as_slice() else {
        return Err("Unexpected first summary history".into());
    };
    if Some(&failed.summary_attempt_id) != interrupted_attempt.as_ref()
        || !matches!(failed.state, SummaryInspectionState::Failed)
        || !matches!(failed.cessation, CessationEvidence::Confirmed)
        || !failed.retry_eligible
    {
        return Err("Summary retry lacks confirmed cessation of its exact previous attempt".into());
    }
    proof.record(
        "failedSummaryPreservesWorkerAndOccupancy",
        json!({"run":blocked,"attempt":failed}),
    )?;
    let retry = RunRecoveryRequest {
        operation_id: OperationId::generate(),
        run_id: blocked.run_id.clone(),
    };
    let first_retry = proof.client.retry_summary(retry.clone()).await?;
    let replayed_retry = proof.client.retry_summary(retry).await?;
    if serde_json::to_value(&first_retry)? != serde_json::to_value(&replayed_retry)? {
        return Err("Identical summary-retry operation did not return its original result".into());
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
    let recovered = loop {
        interval.tick().await;
        let run = current_run(proof, &schedule.schedule_id)
            .await?
            .ok_or("Run disappeared during summary recovery")?;
        if run.run_id != blocked.run_id {
            return Err("Summary recovery created another workflow Run".into());
        }
        match &run.state {
            RunState::Finished {
                outcome: WorkerOutcome::Completed { .. },
                ..
            } => break run,
            RunState::SummaryBlocked { .. }
            | RunState::PreparationFailed { .. }
            | RunState::Uncertain { .. } => {
                proof.record("summaryRetryFailed", json!(run))?;
                return Err("Explicit summary retry failed; worker must not be rerun".into());
            }
            _ => {}
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(
                "Explicit summary retry did not complete within its observation deadline".into(),
            );
        }
    };
    if serde_json::to_value(&recovered.execution_evidence)?
        != serde_json::to_value(&blocked.execution_evidence)?
    {
        return Err("Summary retry changed the original worker execution evidence".into());
    }
    let summary = recovered
        .summary
        .as_ref()
        .ok_or("Recovered Run lacks summary text")?;
    let attempts = proof
        .client
        .read_run_summaries(RunSummariesRequest {
            run_id: recovered.run_id.clone(),
            cursor: None,
            limit: 100.try_into()?,
        })
        .await?;
    if attempts.records.len() != 2
        || attempts.next_cursor.is_some()
        || !attempts.records.iter().any(|attempt| {
            matches!(attempt.state, SummaryInspectionState::Completed)
                && attempt.summary_attempt_id == summary.summary_attempt_id
        })
        || summary.summary_attempt_id == failed.summary_attempt_id
    {
        return Err("Summary retry lost history or allocated duplicate attempts".into());
    }
    let exported = proof
        .client
        .export_schedule(ScheduleShowRequest {
            schedule_id: schedule.schedule_id.clone(),
        })
        .await?;
    proof.record(
        "sameRunSummaryRecoveryVerified",
        json!({"run":recovered,"attempts":attempts}),
    )?;
    Ok(PortableRunProof {
        schedule_id: schedule.schedule_id,
        source_change_id: schedule.change_id,
        package_utf8: exported.package_utf8,
        summary_text: summary.text.clone(),
    })
}

async fn current_run(
    proof: &mut ProofContext,
    schedule_id: &ScheduleId,
) -> ProofResult<Option<RunSnapshot>> {
    let page = proof
        .client
        .list_runs(RunListRequest {
            schedule_id: schedule_id.clone(),
            cursor: None,
            limit: 100.try_into()?,
        })
        .await?;
    if page.records.len() > 1 || page.next_cursor.is_some() {
        return Err("One-shot summary recovery created additional worker Runs".into());
    }
    Ok(page.records.into_iter().next())
}

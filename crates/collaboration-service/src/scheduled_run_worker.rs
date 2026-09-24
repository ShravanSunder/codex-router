//! One bounded scheduled-run step through the injected execution route.
use crate::{
    DeliveryContractError, DeliveryPrecondition, NativeControlBackend, RunObservationContext,
    RunSubmission, ScheduleDestination, ScheduledRunExecution,
};
use agent_automation::{
    ExecutionDestination, PreparationEffect, RouteEffectEvidence, RunId, RunPhase,
};
use automation_storage::{
    AutomationStore, RunSubmissionOutcome, RunSubmissionResult, RunUncertainty, StorageError,
};
use collaboration_protocol::{
    CodexGeneration, DestinationPreparation, EndpointRef, RunExecution, SessionRef,
};
use std::sync::Arc;
use tokio::sync::Mutex;

#[cfg(test)]
#[path = "scheduled_input_validation_tests.rs"]
mod input_validation_tests;
#[cfg(test)]
#[path = "scheduled_run_route_tests.rs"]
mod route_tests;
#[cfg(test)]
#[path = "worker_timeout_crash_tests.rs"]
mod timeout_crash_tests;

#[cfg(test)]
pub(crate) fn worker_timeout_checkpoint(stage: &str) {
    timeout_crash_tests::checkpoint(stage);
}

type StoredRun = agent_automation::RunRecord<
    SessionRef,
    EndpointRef,
    CodexGeneration,
    crate::stored_run_receipt::StoredRunReceipt,
>;

#[derive(Clone)]
pub(crate) struct ScheduledRunWorker {
    pub store: Arc<Mutex<AutomationStore>>,
    pub execution: Arc<dyn ScheduledRunExecution>,
    pub backend: Option<NativeControlBackend>,
    pub configuration: crate::AutomationConfigurationHandle,
}

impl ScheduledRunWorker {
    pub async fn step(&self, id: RunId) -> Result<(), StorageError> {
        let record = self
            .store
            .lock()
            .await
            .read_run::<SessionRef, EndpointRef, CodexGeneration, crate::stored_run_receipt::StoredRunReceipt>(&id)
            .await?;
        if matches!(
            record.phase,
            RunPhase::SummaryRequired | RunPhase::SummaryRunning | RunPhase::SummaryBlocked
        ) {
            return self.step_summary(record).await;
        }
        match record.phase {
            RunPhase::Preparing => self.step_preparation(record).await,
            RunPhase::Executing | RunPhase::Stopping | RunPhase::Uncertain => {
                self.step_observation(record).await
            }
            _ => Ok(()),
        }
    }

    async fn step_summary(&self, record: StoredRun) -> Result<(), StorageError> {
        let Some(backend) = &self.backend else {
            return Ok(());
        };
        let Ok(admission) = backend.gate.acquire() else {
            return Ok(());
        };
        let seconds = if record.phase == RunPhase::SummaryRequired {
            let lease = self.configuration.admission_lease().await;
            let Some(configuration) = lease.configuration() else {
                return Ok(());
            };
            u32::from(configuration.summary_timeout_seconds)
        } else {
            record
                .summary_attempt
                .as_ref()
                .ok_or(StorageError::InvalidRecord)?
                .effective_timeout_seconds
        };
        let needs_source = record.summary_attempt.as_ref().is_some_and(|attempt| {
            attempt.phase == agent_automation::SummaryPhase::Preparing && attempt.target.is_some()
        });
        let source = if needs_source {
            let context = RunObservationContext {
                run_id: record.run_id.clone(),
                phase: record.phase,
                recorded: record
                    .evidence
                    .route
                    .clone()
                    .ok_or(StorageError::InvalidRecord)?,
                inputs: record.inputs.clone().ok_or(StorageError::InvalidRecord)?,
            };
            match self.execution.summary_source(context).await {
                Ok(source) => Some(source),
                Err(DeliveryContractError::ClientOperation) => {
                    Some(crate::RunSummarySource::Unavailable {
                        reason: "scheduled source route is unavailable".into(),
                    })
                }
                Err(_) => return Err(StorageError::InvalidRecord),
            }
        } else {
            None
        };
        crate::summary_native_worker::step(crate::summary_native_worker::SummaryStep {
            work: crate::summary_native_worker::SummaryWork::Advance,
            store: &self.store,
            admission: &admission,
            summary_endpoint: backend.endpoint.clone(),
            source,
            record,
            timeout_seconds: seconds,
        })
        .await
    }

    async fn step_observation(&self, record: StoredRun) -> Result<(), StorageError> {
        let Some(recorded) = record.evidence.route.clone() else {
            return Err(StorageError::InvalidRecord);
        };
        let inputs = record.inputs.clone().ok_or(StorageError::InvalidRecord)?;
        let context = RunObservationContext {
            run_id: record.run_id.clone(),
            phase: record.phase,
            recorded: recorded.clone(),
            inputs,
        };
        let settlement = match self.execution.observe_settlement(context).await {
            Ok(settlement) => settlement,
            Err(DeliveryContractError::ClientOperation) => return Ok(()),
            Err(_) => return Err(StorageError::InvalidRecord),
        };
        if crate::run_reconciliation::persist_settlement(&self.store, &record, settlement).await? {
            return Ok(());
        }
        if record.phase == RunPhase::Stopping
            || !record.evidence.timing.as_ref().is_some_and(|timing| {
                timing.deadline_at_ms <= chrono::Utc::now().timestamp_millis()
            })
        {
            return Ok(());
        }
        let sink = crate::scheduled_run_evidence_sink::StoredRunEvidenceSink::new(
            Arc::clone(&self.store),
            self.configuration.clone(),
            record.run_id.clone(),
            record.schedule_id.clone(),
            Some(recorded.clone()),
        );
        let context = RunObservationContext {
            run_id: record.run_id,
            phase: record.phase,
            recorded,
            inputs: record.inputs.ok_or(StorageError::InvalidRecord)?,
        };
        match self.execution.request_stop(context, &sink).await {
            Ok(_) | Err(DeliveryContractError::ClientOperation) => {}
            Err(_) => return Err(StorageError::InvalidRecord),
        }
        Ok(())
    }

    async fn step_preparation(&self, record: StoredRun) -> Result<(), StorageError> {
        let id = record.run_id.clone();
        let inputs = record.inputs.clone().ok_or(StorageError::InvalidRecord)?;
        if record.evidence.timing.is_some()
            || record
                .evidence
                .route
                .as_ref()
                .is_some_and(route_preparation_unknown)
        {
            if let Some(RouteEffectEvidence::CodexAppServer(mut native)) = record.evidence.route {
                native.submission = agent_automation::SubmissionEffect::Unknown;
                self.store
                    .lock()
                    .await
                    .retain_run_uncertainty::<SessionRef, EndpointRef, CodexGeneration, crate::stored_run_receipt::StoredRunReceipt>(
                        RunUncertainty { run_id: id, effects: native },
                    )
                    .await?;
            }
            return Ok(());
        }
        let selected_evidence = if let Some(recorded) = record.evidence.route.clone() {
            recorded
        } else {
            let destination = match preparation_destination(&inputs)? {
                DestinationPreparation::Existing { target, .. } => {
                    ScheduleDestination::Existing { target }
                }
                DestinationPreparation::Fresh { endpoint, .. } => {
                    ScheduleDestination::Fresh { endpoint }
                }
                DestinationPreparation::Fork { .. } => return Err(StorageError::InvalidRecord),
            };
            match self.execution.initial_evidence(&destination).await {
                Ok(evidence) => evidence,
                Err(DeliveryContractError::ClientOperation) => return Ok(()),
                Err(_) => return Err(StorageError::InvalidRecord),
            }
        };
        let selected_target = route_target(&selected_evidence).or_else(|| captured_target(&inputs));
        let text = match render_instructions(
            &inputs,
            record.schedule_id.as_str(),
            id.as_str(),
            selected_target.as_ref(),
        ) {
            Ok(text) => text,
            Err(_) => {
                self.store.lock().await.fail_run_preparation::<SessionRef, EndpointRef, CodexGeneration, crate::stored_run_receipt::StoredRunReceipt>(
                    automation_storage::RunPreparationFailure {
                        run_id: id,
                        effects: selected_evidence,
                        explanation: "Combined instructions and continuity exceed the supported request size; no input was submitted; any earlier preparation effects remain recorded. Shorten instructions or continuity for future runs.".into(),
                        now_ms: chrono::Utc::now().timestamp_millis(),
                    },
                ).await?;
                return Ok(());
            }
        };
        if record.evidence.route.is_none() {
            return self.prepare_target(record, inputs, text).await;
        }
        let recorded = record
            .evidence
            .route
            .clone()
            .ok_or(StorageError::InvalidRecord)?;
        let Some(target) = route_target(&recorded).or_else(|| captured_target(&inputs)) else {
            return self.prepare_target(record, inputs, text).await;
        };
        let sink = crate::scheduled_run_evidence_sink::StoredRunEvidenceSink::new(
            Arc::clone(&self.store),
            self.configuration.clone(),
            id.clone(),
            record.schedule_id,
            Some(recorded.clone()),
        );
        let submission = self
            .execution
            .submit_run(
                crate::ScheduledRunSubmission {
                    run_id: id.clone(),
                    target,
                    message: text.try_into().map_err(|_| StorageError::InvalidRecord)?,
                    precondition: DeliveryPrecondition::Unpinned,
                    inputs,
                    recorded: recorded.clone(),
                },
                &sink,
            )
            .await;
        let submitted = match submission {
            Ok(submitted) => submitted,
            Err(DeliveryContractError::ClientOperation) => return Ok(()),
            Err(_) => return Err(StorageError::InvalidRecord),
        };
        let latest = sink.latest().await.unwrap_or(recorded);
        self.persist_submission(id, latest, submitted).await
    }

    async fn prepare_target(
        &self,
        record: StoredRun,
        inputs: agent_automation::CapturedRunInputs<SessionRef, EndpointRef>,
        text: String,
    ) -> Result<(), StorageError> {
        let destination = preparation_destination(&inputs)?;
        let sink = crate::scheduled_run_evidence_sink::StoredRunEvidenceSink::new(
            Arc::clone(&self.store),
            self.configuration.clone(),
            record.run_id.clone(),
            record.schedule_id.clone(),
            None,
        );
        let prepared = match destination {
            DestinationPreparation::Existing { target, .. } => {
                self.execution.prepare_existing_target(&target, &sink).await
            }
            DestinationPreparation::Fresh { endpoint, cwd } => {
                self.execution
                    .prepare_fresh_session(
                        crate::FreshSessionRequest {
                            endpoint,
                            working_directory: cwd,
                            message: text.try_into().map_err(|_| StorageError::InvalidRecord)?,
                            inputs,
                        },
                        &sink,
                    )
                    .await
            }
            DestinationPreparation::Fork { .. } => return Err(StorageError::InvalidRecord),
        };
        match prepared {
            Ok(_) => Ok(()),
            Err(DeliveryContractError::ClientOperation) => {
                let Some(evidence) = sink.latest().await else {
                    return Ok(());
                };
                if let RouteEffectEvidence::CodexAppServer(native) = &evidence
                    && route_preparation_unknown(&evidence)
                {
                    self.store.lock().await.retain_run_uncertainty::<SessionRef, EndpointRef, CodexGeneration, crate::stored_run_receipt::StoredRunReceipt>(
                        RunUncertainty { run_id: record.run_id, effects: native.clone() },
                    ).await?;
                } else {
                    self.store.lock().await.fail_run_preparation::<SessionRef, EndpointRef, CodexGeneration, crate::stored_run_receipt::StoredRunReceipt>(
                        automation_storage::RunPreparationFailure {
                            run_id: record.run_id,
                            effects: evidence,
                            explanation: "Route preparation could not confirm its target; no worker input was submitted.".into(),
                            now_ms: chrono::Utc::now().timestamp_millis(),
                        },
                    ).await?;
                }
                Ok(())
            }
            Err(_) => Err(StorageError::InvalidRecord),
        }
    }

    async fn persist_submission(
        &self,
        id: RunId,
        effects: RouteEffectEvidence<SessionRef, CodexGeneration>,
        submission: RunSubmission,
    ) -> Result<(), StorageError> {
        let outcome = match submission {
            RunSubmission::NotStartedBusy => return Ok(()),
            RunSubmission::Started(acceptance) => match acceptance.execution {
                RunExecution::CodexAppServer(native) => RunSubmissionOutcome::Accepted {
                    turn_id: native.turn_id,
                    receipt: crate::stored_run_receipt::StoredRunReceipt::Current(
                        acceptance.receipt,
                    ),
                },
                RunExecution::ProviderAcp { .. } => RunSubmissionOutcome::ProviderAdmitted {
                    receipt: crate::stored_run_receipt::StoredRunReceipt::Current(
                        acceptance.receipt,
                    ),
                },
                RunExecution::ClaudeCodePeer { written_at, .. } => {
                    let timestamp = chrono::DateTime::parse_from_rfc3339(&String::from(written_at))
                        .map_err(|_| StorageError::InvalidRecord)?
                        .timestamp_millis();
                    RunSubmissionOutcome::PeerWritten {
                        written_at_ms: timestamp,
                        receipt: crate::stored_run_receipt::StoredRunReceipt::Current(
                            acceptance.receipt,
                        ),
                    }
                }
            },
            RunSubmission::Rejected(rejection) => RunSubmissionOutcome::Rejected {
                explanation: rejection
                    .detail
                    .unwrap_or_else(|| format!("{:?}", rejection.reason)),
            },
            RunSubmission::Unknown => RunSubmissionOutcome::Unknown {
                explanation: "Scheduled start outcome unknown; no automatic resend.".into(),
            },
        };
        self.store
            .lock()
            .await
            .record_run_submission::<_, EndpointRef, _, _>(RunSubmissionResult {
                run_id: id,
                effects,
                outcome,
            })
            .await?;
        Ok(())
    }
}

fn route_preparation_unknown(evidence: &RouteEffectEvidence<SessionRef, CodexGeneration>) -> bool {
    matches!(evidence,
        RouteEffectEvidence::CodexAppServer(native)
            if native.allocation == PreparationEffect::Unknown
                || native.resume == PreparationEffect::Unknown)
}

fn route_target(evidence: &RouteEffectEvidence<SessionRef, CodexGeneration>) -> Option<SessionRef> {
    match evidence {
        RouteEffectEvidence::CodexAppServer(native) => native.target.clone(),
        RouteEffectEvidence::ProviderAcp(provider) => provider.target.clone(),
        RouteEffectEvidence::ClaudeCodePeer(_) => None,
    }
}

fn captured_target(
    inputs: &agent_automation::CapturedRunInputs<SessionRef, EndpointRef>,
) -> Option<SessionRef> {
    match &inputs.execution_configuration.destination {
        ExecutionDestination::OwnedThread { target, .. } => Some(target.clone()),
        _ => None,
    }
}

fn preparation_destination(
    inputs: &agent_automation::CapturedRunInputs<SessionRef, EndpointRef>,
) -> Result<DestinationPreparation, StorageError> {
    match &inputs.execution_configuration.destination {
        ExecutionDestination::Unprepared | ExecutionDestination::FreshEachRunUnprepared => {
            Err(StorageError::InvalidRecord)
        }
        ExecutionDestination::OwnedThread { target, cwd } => Ok(DestinationPreparation::Existing {
            target: target.clone(),
            cwd: cwd.clone(),
        }),
        ExecutionDestination::FreshEachRun { endpoint, cwd } => Ok(DestinationPreparation::Fresh {
            endpoint: endpoint.clone(),
            cwd: cwd.clone(),
        }),
    }
}
fn render_instructions(
    inputs: &agent_automation::CapturedRunInputs<SessionRef, EndpointRef>,
    schedule_id: &str,
    run_id: &str,
    target: Option<&SessionRef>,
) -> Result<String, StorageError> {
    let mut text = format!(
        "Agent communication\nSelf-declared sender: local scheduled automation {schedule_id}\nIntended recipient: {}\nRun: {run_id}\n\n{}",
        target
            .map(serde_json::to_string)
            .transpose()
            .map_err(|_| StorageError::InvalidRecord)?
            .unwrap_or_else(|| "new execution thread".into()),
        inputs.instruction_text.as_str()
    );
    // Continued threads already have their context. Only new execution threads need continuity text.
    if target.is_none()
        && matches!(
            inputs.execution_configuration.destination,
            ExecutionDestination::FreshEachRun { .. }
        )
    {
        match &inputs.continuity {
            agent_automation::ContinuityInput::LocalSummary { text: summary, .. }
            | agent_automation::ContinuityInput::ImportedSummary { text: summary, .. } => {
                text.push_str("\n\nPrevious run summary (context, not proof):\n");
                text.push_str(summary);
            }
            _ => {}
        }
    }
    if text.len() > collaboration_protocol::MAX_CONTROL_FRAME_BYTES {
        return Err(StorageError::InvalidRecord);
    }
    Ok(text)
}

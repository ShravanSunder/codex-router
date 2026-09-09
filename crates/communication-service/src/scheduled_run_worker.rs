//! One bounded step for a Run: prepare, wait for idleness, submit once, or observe its exact turn.
use crate::NativeControlBackend;
use agent_automation::{ExecutionDestination, PreparationEffect, RunId, RunPhase};
use automation_storage::{
    AutomationStore, RunCompletion, RunPreparationIntent, RunPreparedTarget, RunUncertainty,
    StorageError, ThreadBindingClaim,
};
use communication_protocol::{
    CodexGeneration, DestinationPreparation, EndpointRef, NativeSendReceipt, SessionRef,
};
use std::sync::Arc;
use tokio::sync::Mutex;
#[derive(Clone)]
pub(crate) struct ScheduledRunWorker {
    pub store: Arc<Mutex<AutomationStore>>,
    pub backend: Option<NativeControlBackend>,
    pub timeout_seconds: u32,
    pub summary_timeout_seconds: u32,
}
impl ScheduledRunWorker {
    pub async fn step(&self, id: RunId) -> Result<(), StorageError> {
        let record = self
            .store
            .lock()
            .await
            .read_run::<SessionRef, EndpointRef, CodexGeneration, NativeSendReceipt>(&id)
            .await?;
        let Some(backend) = &self.backend else {
            return Ok(());
        };
        let Ok(admission) = backend.gate.acquire() else {
            return Ok(());
        };
        if matches!(
            record.phase,
            RunPhase::SummaryRequired | RunPhase::SummaryRunning | RunPhase::SummaryBlocked
        ) {
            return crate::summary_native_worker::step(crate::summary_native_worker::SummaryStep {
                store: &self.store,
                admission: &admission,
                record,
                timeout_seconds: self.summary_timeout_seconds,
            })
            .await;
        }
        if matches!(
            record.phase,
            RunPhase::Executing | RunPhase::Stopping | RunPhase::Uncertain
        ) {
            let (Some(target), Some(turn_id)) =
                (&record.evidence.native.target, &record.native_turn_id)
            else {
                return Ok(());
            };
            if record.phase != RunPhase::Stopping
                && record.evidence.timing.as_ref().is_some_and(|time| {
                    time.deadline_at_ms <= chrono::Utc::now().timestamp_millis()
                })
            {
                if self
                    .store
                    .lock()
                    .await
                    .begin_run_stop::<SessionRef, EndpointRef, CodexGeneration, NativeSendReceipt>(
                        &id, turn_id,
                    )
                    .await?
                    && let Some(schemas) = admission.schemas()
                    && let Ok(mut connection) =
                        codex_native_integration::NativeProtocolConnection::connect(
                            admission.backend_path(),
                        )
                        .await
                {
                    let _response=connection.request_validated(&schemas,codex_native_integration::NativeOperation::InterruptTurn,serde_json::json!({"threadId":String::from(target.session_id.clone()),"turnId":turn_id})).await;
                }
                return Ok(());
            }
            let turn = tokio::time::timeout(
                std::time::Duration::from_secs(30),
                crate::scheduled_native_observation::read_turn(&admission, target, turn_id),
            )
            .await;
            let turn = match turn {
                Ok(Ok(Some(turn))) => turn,
                _ => return Ok(()),
            };
            let status = turn.get("status").and_then(serde_json::Value::as_str);
            let outcome = match status {
                Some("completed") => {
                    Some(agent_automation::WorkerOutcome::Completed { explanation: None })
                }
                Some("failed") => Some(agent_automation::WorkerOutcome::Failed {
                    explanation: Some("Native turn failed; inspect its recorded output.".into()),
                }),
                Some("interrupted") => Some(agent_automation::WorkerOutcome::Interrupted {
                    explanation: Some("Native turn was interrupted.".into()),
                }),
                _ => None,
            };
            if let Some(outcome) = outcome {
                self.store.lock().await.complete_run_turn::<SessionRef,EndpointRef,CodexGeneration,NativeSendReceipt>(RunCompletion{run_id:id,native_turn_id:turn_id.clone(),outcome,now_ms:chrono::Utc::now().timestamp_millis()}).await?;
            }
            return Ok(());
        }
        if record.phase != RunPhase::Preparing {
            return Ok(());
        }
        let inputs = record.inputs.ok_or(StorageError::InvalidRecord)?;
        if record.evidence.timing.is_some()
            || matches!(
                record.evidence.native.allocation,
                PreparationEffect::Unknown
            )
            || matches!(record.evidence.native.resume, PreparationEffect::Unknown)
        {
            let mut effects = record.evidence.native;
            effects.submission = agent_automation::SubmissionEffect::Unknown;
            self.store
                .lock()
                .await
                .retain_run_uncertainty::<SessionRef, EndpointRef, CodexGeneration, NativeSendReceipt>(RunUncertainty {
                    run_id: id,
                    effects,
                })
                .await?;
            return Ok(());
        }
        let target = record.evidence.native.target.clone();
        let Some(target) = target else {
            let destination = match &inputs.execution_configuration.destination {
                ExecutionDestination::Unprepared => return Err(StorageError::InvalidRecord),
                ExecutionDestination::OwnedThread { target, cwd } => {
                    DestinationPreparation::Existing {
                        target: target.clone(),
                        cwd: cwd.clone(),
                    }
                }
                ExecutionDestination::FreshEachRun { endpoint, cwd } => {
                    DestinationPreparation::Fresh {
                        endpoint: endpoint.clone(),
                        cwd: cwd.clone(),
                    }
                }
            };
            if *crate::native_thread_preparation::endpoint(&destination) != backend.endpoint {
                return Err(StorageError::ActivationUnavailable);
            }
            let mut intent = record.evidence.native;
            intent.generation = Some(admission.generation().clone());
            if matches!(destination, DestinationPreparation::Fresh { .. }) {
                intent.allocation = PreparationEffect::Unknown;
            }
            if !self
                .store
                .lock()
                .await
                .begin_run_preparation::<_, EndpointRef, _, NativeSendReceipt>(
                    RunPreparationIntent {
                        run_id: id.clone(),
                        effects: intent,
                    },
                )
                .await?
            {
                return Ok(());
            }
            let text =
                render_instructions(&inputs, record.schedule_id.as_str(), id.as_str(), None)?;
            let prepared = crate::native_thread_preparation::prepare(
                crate::native_thread_preparation::NativePreparationInput {
                    admission: &admission,
                    destination: &destination,
                    instruction_text: &text,
                },
            )
            .await;
            match prepared {
                Ok((target, effects)) => {
                    self.store
                        .lock()
                        .await
                        .record_run_target::<SessionRef, EndpointRef, CodexGeneration, NativeSendReceipt>(
                            RunPreparedTarget {
                                run_id: id,
                                effects: serde_json::from_value(
                                    serde_json::to_value(effects)
                                        .map_err(|_| StorageError::InvalidRecord)?,
                                )
                                .map_err(|_| StorageError::InvalidRecord)?,
                                binding: ThreadBindingClaim {
                                    schedule_id: record.schedule_id,
                                    service_id: String::from(target.endpoint.service_id),
                                    endpoint_id: String::from(target.endpoint.endpoint_id),
                                    thread_id: String::from(target.session_id),
                                    now_ms: chrono::Utc::now().timestamp_millis(),
                                },
                            },
                        )
                        .await?;
                }
                Err(error) => {
                    let effects = serde_json::from_value(
                        serde_json::to_value(error.effects)
                            .map_err(|_| StorageError::InvalidRecord)?,
                    )
                    .map_err(|_| StorageError::InvalidRecord)?;
                    self.store
                        .lock()
                        .await
                        .retain_run_uncertainty::<SessionRef, EndpointRef, CodexGeneration, NativeSendReceipt>(
                            RunUncertainty {
                                run_id: id,
                                effects,
                            },
                        )
                        .await?;
                }
            }
            return Ok(());
        };
        if target.endpoint != backend.endpoint {
            return Err(StorageError::ActivationUnavailable);
        }
        let text = render_instructions(
            &inputs,
            record.schedule_id.as_str(),
            id.as_str(),
            Some(&target),
        )?;
        crate::scheduled_native_dispatch::dispatch(
            crate::scheduled_native_dispatch::ScheduledDispatch {
                admission: &admission,
                store: &self.store,
                run_id: id,
                schedule_id: record.schedule_id,
                target,
                text,
                effects: record.evidence.native,
                timeout_seconds: self.timeout_seconds,
            },
        )
        .await
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
    if text.len() > communication_protocol::MAX_CONTROL_FRAME_BYTES {
        return Err(StorageError::InvalidRecord);
    }
    Ok(text)
}

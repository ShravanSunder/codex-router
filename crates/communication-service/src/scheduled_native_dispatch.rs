//! Scheduled starts wait for native idleness; they never choose the ordinary message auto-steer branch.
use crate::NativeAdmission;
use agent_automation::{
    CessationEvidence, NativeEffectEvidence, PreparationEffect, RunId, SubmissionEffect,
};
use automation_storage::{
    AutomationStore, RunDispatchIntent, RunSubmissionOutcome, RunSubmissionResult, StorageError,
};
use codex_native_integration::{NativeConnectionError, NativeOperation, NativeProtocolConnection};
use communication_protocol::{
    AcceptedResumeEffect, CodexGeneration, EndpointRef, MessageInputKind, MessageRepresentation,
    NativeInputDisposition, NativeInputOperation, NativeSendAcceptance, NativeSendReceipt,
    SessionRef,
};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::Mutex;
pub(crate) struct ScheduledDispatch<'a> {
    pub admission: &'a NativeAdmission,
    pub store: &'a Arc<Mutex<AutomationStore>>,
    pub run_id: RunId,
    pub schedule_id: agent_automation::ScheduleId,
    pub target: SessionRef,
    pub text: String,
    pub effects: NativeEffectEvidence<SessionRef, CodexGeneration>,
    pub configuration: &'a crate::AutomationConfigurationHandle,
}
pub(crate) async fn dispatch(mut input: ScheduledDispatch<'_>) -> Result<(), StorageError> {
    let Some(schemas) = input.admission.schemas() else {
        return Ok(());
    };
    let connection = NativeProtocolConnection::connect(input.admission.backend_path()).await;
    let Ok(mut connection) = connection else {
        return Ok(());
    };
    let id = String::from(input.target.session_id.clone());
    let read = connection
        .request_validated(
            &schemas,
            NativeOperation::ReadThread,
            json!({"threadId":id,"includeTurns":false}),
        )
        .await;
    let Ok(read) = read else {
        return Ok(());
    };
    if read.pointer("/thread/id").and_then(Value::as_str) != Some(id.as_str()) {
        return Err(StorageError::InvalidRecord);
    }
    let state = read.pointer("/thread/status/type").and_then(Value::as_str);
    if state == Some("active") {
        return Ok(());
    }
    if state == Some("notLoaded") {
        let configuration_lease = input.configuration.admission_lease().await;
        if configuration_lease.configuration().is_none() {
            return Ok(());
        }
        input.effects.resume = PreparationEffect::Unknown;
        // Resume has an effect even though it does not consume the execution budget.
        input
            .store
            .lock()
            .await
            .retain_run_uncertainty::<_, EndpointRef, _, NativeSendReceipt>(
                automation_storage::RunUncertainty {
                    run_id: input.run_id.clone(),
                    effects: input.effects.clone(),
                },
            )
            .await?;
        drop(configuration_lease);
        let resumed = connection
            .request_validated(
                &schemas,
                NativeOperation::ResumeThread,
                json!({"threadId":id}),
            )
            .await;
        match resumed {
            Ok(resumed) => {
                if resumed.pointer("/thread/id").and_then(Value::as_str) != Some(id.as_str()) {
                    return Ok(());
                }
                input.effects.resume = PreparationEffect::Accepted;
            }
            Err(NativeConnectionError::Rejected { .. }) => {
                input.effects.resume = PreparationEffect::Rejected;
            }
            Err(NativeConnectionError::InvalidInput | NativeConnectionError::Unavailable) => {
                input.effects.resume = PreparationEffect::NotDispatched;
            }
            Err(_) => return Ok(()),
        }
        input
            .store
            .lock()
            .await
            .record_run_target::<_, EndpointRef, _, NativeSendReceipt>(
                automation_storage::RunPreparedTarget {
                    run_id: input.run_id,
                    effects: input.effects,
                    binding: automation_storage::ThreadBindingClaim {
                        schedule_id: input.schedule_id,
                        service_id: String::from(input.target.endpoint.service_id),
                        endpoint_id: String::from(input.target.endpoint.endpoint_id),
                        thread_id: id,
                        now_ms: chrono::Utc::now().timestamp_millis(),
                    },
                },
            )
            .await?;
        // Known non-submission returns to preparation without an execution budget.
        // Recheck residency next pass; a resumed thread may already be busy.
        return Ok(());
    }
    if state != Some("idle") {
        return Ok(());
    }
    input.effects.generation = Some(input.admission.generation().clone());
    input.effects.submission = SubmissionEffect::Dispatching;
    input.effects.client_user_message_id = Some(input.run_id.as_str().into());
    input.effects.cessation = CessationEvidence::Unconfirmed;
    let configuration_lease = input.configuration.admission_lease().await;
    let Some(configuration) = configuration_lease.configuration() else {
        return Ok(());
    };
    input
        .store
        .lock()
        .await
        .begin_run_dispatch::<_, EndpointRef, _, NativeSendReceipt>(RunDispatchIntent {
            run_id: input.run_id.clone(),
            effects: input.effects.clone(),
            configured_timeout_seconds: u32::from(configuration.execution_timeout_seconds),
            now_ms: chrono::Utc::now().timestamp_millis(),
        })
        .await?;
    drop(configuration_lease);
    let retired = input.admission.retirement();
    let response=tokio::time::timeout(std::time::Duration::from_secs(30),async{tokio::select!{biased;result=connection.request_validated(&schemas,NativeOperation::StartTurn,json!({"threadId":id,"input":[{"type":"text","text":input.text}],"clientUserMessageId":input.run_id.as_str()}))=>result,_=retired.cancelled()=>Err(NativeConnectionError::OutcomeUnknown)}}).await.unwrap_or(Err(NativeConnectionError::OutcomeUnknown));
    let outcome = match response {
        Ok(response) => {
            if let Some(turn_id) = response
                .pointer("/turn/id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
            {
                input.effects.submission = SubmissionEffect::Accepted;
                input.effects.native_turn_id = Some(turn_id.into());
                let receipt = NativeSendReceipt {
                    target: input.target,
                    generation: input.admission.generation().clone(),
                    input_kind: MessageInputKind::Agent,
                    representation: MessageRepresentation::DeclaredAgentText,
                    client_user_message_id: input
                        .run_id
                        .as_str()
                        .to_owned()
                        .try_into()
                        .map_err(|_| StorageError::InvalidRecord)?,
                    resume_effect: if input.effects.resume == PreparationEffect::Accepted {
                        AcceptedResumeEffect::Accepted
                    } else {
                        AcceptedResumeEffect::NotRequested
                    },
                    acceptance: NativeSendAcceptance::NativeInputAccepted {
                        operation: NativeInputOperation::TurnStart,
                        disposition: NativeInputDisposition::StartedOrSteered,
                        turn_id: turn_id
                            .to_owned()
                            .try_into()
                            .map_err(|_| StorageError::InvalidRecord)?,
                    },
                };
                RunSubmissionOutcome::Accepted {
                    turn_id: turn_id.into(),
                    receipt,
                }
            } else {
                input.effects.submission = SubmissionEffect::Unknown;
                RunSubmissionOutcome::Unknown {
                    explanation: "Native start returned no usable turn identity.".into(),
                }
            }
        }
        Err(NativeConnectionError::Rejected { .. }) => {
            input.effects.submission = SubmissionEffect::Rejected;
            RunSubmissionOutcome::Rejected {
                explanation: "Native scheduled start was rejected.".into(),
            }
        }
        Err(NativeConnectionError::InvalidInput | NativeConnectionError::Unavailable) => {
            input.effects.submission = SubmissionEffect::NotDispatched;
            RunSubmissionOutcome::Rejected {
                explanation: "Native scheduled start was not dispatched.".into(),
            }
        }
        Err(_) => {
            input.effects.submission = SubmissionEffect::Unknown;
            RunSubmissionOutcome::Unknown {
                explanation: "Native start outcome unknown; no automatic resend.".into(),
            }
        }
    };
    input
        .store
        .lock()
        .await
        .record_run_submission::<_, EndpointRef, _, _>(RunSubmissionResult {
            run_id: input.run_id,
            effects: input.effects,
            outcome,
        })
        .await?;
    Ok(())
}

//! Native scheduled submission waits for idleness and records intent before starting a turn.
use crate::{
    DeliveryContractError, DeliveryPrecondition, NativeControlBackend, RunAcceptance,
    RunEvidenceDisposition, RunEvidenceSink, RunSubmission, ScheduledRunSubmission,
};
use agent_automation::{
    CessationEvidence, PreparationEffect, RouteEffectEvidence, SubmissionEffect,
};
use codex_native_integration::{NativeConnectionError, NativeOperation, NativeProtocolConnection};
use collaboration_protocol::{
    AcceptedResumeEffect, DeliveryNextAction, DeliveryRejection, DeliveryRejectionReason,
    MessageInputKind, MessageRepresentation, NativeExecution, NativeInputDisposition,
    NativeInputOperation, NativeSendAcceptance, NativeSendReceipt, RunExecution,
};
use serde_json::{Value, json};

pub(crate) async fn dispatch(
    backend: &NativeControlBackend,
    run: ScheduledRunSubmission,
    sink: &dyn RunEvidenceSink,
) -> Result<RunSubmission, DeliveryContractError> {
    let RouteEffectEvidence::CodexAppServer(mut effects) = run.recorded else {
        return Err(DeliveryContractError::InvalidEvidence);
    };
    if effects.target.as_ref() != Some(&run.target) || run.target.endpoint != backend.endpoint {
        return Err(DeliveryContractError::InvalidEvidence);
    }
    let admission = backend
        .gate
        .acquire()
        .map_err(|_| DeliveryContractError::ClientOperation)?;
    if let DeliveryPrecondition::EndpointGeneration { expected } = &run.precondition
        && expected != admission.generation()
    {
        return Ok(RunSubmission::Rejected(DeliveryRejection {
            reason: DeliveryRejectionReason::StaleGeneration,
            next_action: DeliveryNextAction::InspectTarget,
            client_code: None,
            detail: Some("Scheduled target generation changed before submission".into()),
        }));
    }
    let schemas = admission
        .schemas()
        .ok_or(DeliveryContractError::ClientOperation)?;
    let Ok(mut connection) = NativeProtocolConnection::connect(admission.backend_path()).await
    else {
        return Ok(RunSubmission::NotStartedBusy);
    };
    let thread_id = String::from(run.target.session_id.clone());
    let read = connection
        .request_validated(
            &schemas,
            NativeOperation::ReadThread,
            json!({"threadId":thread_id,"includeTurns":false}),
        )
        .await;
    let Ok(read) = read else {
        return Ok(RunSubmission::NotStartedBusy);
    };
    if read.pointer("/thread/id").and_then(Value::as_str) != Some(thread_id.as_str()) {
        return Err(DeliveryContractError::InvalidEvidence);
    }
    match read.pointer("/thread/status/type").and_then(Value::as_str) {
        Some("active") => return Ok(RunSubmission::NotStartedBusy),
        Some("notLoaded") => {
            effects.resume = PreparationEffect::Unknown;
            if matches!(
                sink.record(RouteEffectEvidence::CodexAppServer(effects.clone()))
                    .await?,
                RunEvidenceDisposition::AdmissionRefused
            ) {
                return Ok(RunSubmission::NotStartedBusy);
            }
            let resumed = connection
                .request_validated(
                    &schemas,
                    NativeOperation::ResumeThread,
                    json!({"threadId":thread_id,"excludeTurns":true}),
                )
                .await;
            effects.resume = match resumed {
                Ok(value)
                    if value.pointer("/thread/id").and_then(Value::as_str)
                        == Some(thread_id.as_str()) =>
                {
                    PreparationEffect::Accepted
                }
                Ok(_) => return Ok(RunSubmission::Unknown),
                Err(NativeConnectionError::Rejected { .. }) => PreparationEffect::Rejected,
                Err(NativeConnectionError::InvalidInput | NativeConnectionError::Unavailable) => {
                    PreparationEffect::NotDispatched
                }
                Err(_) => return Ok(RunSubmission::Unknown),
            };
            sink.record(RouteEffectEvidence::CodexAppServer(effects))
                .await?;
            return Ok(RunSubmission::NotStartedBusy);
        }
        Some("idle") => {}
        _ => return Ok(RunSubmission::NotStartedBusy),
    }
    effects.generation = Some(admission.generation().clone());
    effects.submission = SubmissionEffect::Dispatching;
    effects.client_user_message_id = Some(run.run_id.as_str().into());
    effects.cessation = CessationEvidence::Unconfirmed;
    let timing = match sink
        .record(RouteEffectEvidence::CodexAppServer(effects.clone()))
        .await?
    {
        RunEvidenceDisposition::Recorded {
            timing: Some(timing),
        } => timing,
        RunEvidenceDisposition::Recorded { timing: None } => {
            return Err(DeliveryContractError::InvalidEvidence);
        }
        RunEvidenceDisposition::AdmissionRefused => return Ok(RunSubmission::NotStartedBusy),
    };
    let retired = admission.retirement();
    let response = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        tokio::select! {
            biased;
            result = connection.request_validated(
                &schemas,
                NativeOperation::StartTurn,
                json!({"threadId":thread_id,"input":[{"type":"text","text":run.message.as_str()}],
                    "clientUserMessageId":run.run_id.as_str(),
                    "effort":run.inputs.execution_configuration.effort.as_deref().unwrap_or_default()}),
            ) => result,
            _ = retired.cancelled() => Err(NativeConnectionError::OutcomeUnknown),
        }
    })
    .await
    .unwrap_or(Err(NativeConnectionError::OutcomeUnknown));
    let outcome = match response {
        Ok(response) => {
            let Some(turn_id) = response
                .pointer("/turn/id")
                .and_then(Value::as_str)
                .filter(|turn_id| !turn_id.is_empty())
            else {
                effects.submission = SubmissionEffect::Unknown;
                sink.record(RouteEffectEvidence::CodexAppServer(effects))
                    .await?;
                return Ok(RunSubmission::Unknown);
            };
            effects.submission = SubmissionEffect::Accepted;
            effects.native_turn_id = Some(turn_id.into());
            let receipt = NativeSendReceipt {
                target: run.target.clone(),
                generation: admission.generation().clone(),
                input_kind: MessageInputKind::Agent,
                representation: MessageRepresentation::DeclaredAgentText,
                client_user_message_id: run
                    .run_id
                    .as_str()
                    .to_owned()
                    .try_into()
                    .map_err(|_| DeliveryContractError::InvalidEvidence)?,
                resume_effect: if effects.resume == PreparationEffect::Accepted {
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
                        .map_err(|_| DeliveryContractError::InvalidEvidence)?,
                },
            };
            let execution = RunExecution::CodexAppServer(NativeExecution {
                target: run.target,
                turn_id: turn_id.to_owned(),
                started_at: crate::wakeup_projection::timestamp(timing.dispatch_started_at_ms)
                    .map_err(|_| DeliveryContractError::InvalidEvidence)?,
                deadline_at: crate::wakeup_projection::timestamp(timing.deadline_at_ms)
                    .map_err(|_| DeliveryContractError::InvalidEvidence)?,
                effective_timeout_seconds: timing
                    .effective_timeout_seconds
                    .try_into()
                    .map_err(|_| DeliveryContractError::InvalidEvidence)?,
            });
            RunSubmission::Started(Box::new(RunAcceptance {
                execution,
                receipt: receipt.into(),
            }))
        }
        Err(NativeConnectionError::Rejected { .. }) => {
            effects.submission = SubmissionEffect::Rejected;
            RunSubmission::Rejected(DeliveryRejection {
                reason: DeliveryRejectionReason::Unknown,
                next_action: DeliveryNextAction::InspectTarget,
                client_code: None,
                detail: Some("Native scheduled start was rejected.".into()),
            })
        }
        Err(NativeConnectionError::InvalidInput | NativeConnectionError::Unavailable) => {
            effects.submission = SubmissionEffect::NotDispatched;
            RunSubmission::Rejected(DeliveryRejection {
                reason: DeliveryRejectionReason::EndpointUnavailable,
                next_action: DeliveryNextAction::RetryLater,
                client_code: None,
                detail: Some("Native scheduled start was not dispatched.".into()),
            })
        }
        Err(_) => {
            effects.submission = SubmissionEffect::Unknown;
            RunSubmission::Unknown
        }
    };
    sink.record(RouteEffectEvidence::CodexAppServer(effects))
        .await?;
    Ok(outcome)
}

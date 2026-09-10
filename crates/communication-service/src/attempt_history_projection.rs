//! Historical attempt output preserves exact identity; only the current safely stopped summary can retry.
use automation_storage::{AutomationStore, StorageError};
use communication_protocol::{
    AttemptInspection, CodexGeneration, DeliveryEvidence, NativeSendReceipt, SessionRef,
    SummaryInspection,
};

pub(crate) async fn delivery(
    store: &mut AutomationStore,
    delivery_id: &agent_automation::DeliveryId,
    attempt: agent_automation::DeliveryAttempt<SessionRef, CodexGeneration>,
) -> Result<AttemptInspection, StorageError> {
    let effects = serde_json::from_value(
        serde_json::to_value(&attempt.effects).map_err(|_| StorageError::InvalidRecord)?,
    )
    .map_err(|_| StorageError::InvalidRecord)?;
    let evidence = match attempt.outcome {
        agent_automation::AttemptOutcome::InProgress => DeliveryEvidence::Dispatching {
            attempt_id: attempt.attempt_id.clone(),
            effects,
        },
        agent_automation::AttemptOutcome::KnownNotSubmitted { reason, .. } => {
            DeliveryEvidence::KnownNotSubmitted {
                attempt_id: attempt.attempt_id.clone(),
                reason,
                effects,
            }
        }
        agent_automation::AttemptOutcome::Unknown { reason } => DeliveryEvidence::OutcomeUnknown {
            attempt_id: attempt.attempt_id.clone(),
            effects,
            explanation: reason,
        },
        agent_automation::AttemptOutcome::Accepted => {
            let current = store
                .read_delivery::<SessionRef, CodexGeneration, NativeSendReceipt>(delivery_id)
                .await?;
            if current
                .attempt
                .as_ref()
                .is_none_or(|current| current.attempt_id != attempt.attempt_id)
            {
                return Err(StorageError::InvalidRecord);
            }
            DeliveryEvidence::Accepted {
                attempt_id: attempt.attempt_id.clone(),
                receipt: current.receipt.ok_or(StorageError::InvalidRecord)?,
            }
        }
    };
    Ok(AttemptInspection {
        attempt_id: attempt.attempt_id,
        delivery_id: delivery_id.clone(),
        attempt_number: attempt.attempt_number,
        began_at: timestamp(attempt.started_at_ms)?,
        ended_at: attempt.completed_at_ms.map(timestamp).transpose()?,
        evidence,
    })
}
pub(crate) fn summary(
    run_id: &agent_automation::RunId,
    attempt: agent_automation::SummaryAttempt<SessionRef, CodexGeneration>,
    retry_allowed: bool,
) -> Result<SummaryInspection, StorageError> {
    let safely_stopped = attempt.effects.cessation
        == agent_automation::CessationEvidence::Confirmed
        || (matches!(
            attempt.effects.submission,
            agent_automation::SubmissionEffect::NotDispatched
                | agent_automation::SubmissionEffect::Rejected
        ) && attempt.effects.allocation != agent_automation::PreparationEffect::Unknown
            && attempt.effects.resume != agent_automation::PreparationEffect::Unknown);
    Ok(SummaryInspection {
        summary_attempt_id: attempt.attempt_id,
        run_id: run_id.clone(),
        target: attempt.target,
        native_turn_id: attempt.native_turn_id,
        effective_timeout_seconds: attempt
            .effective_timeout_seconds
            .try_into()
            .map_err(|_| StorageError::InvalidRecord)?,
        started_at: Some(timestamp(attempt.started_at_ms)?),
        deadline_at: Some(timestamp(attempt.deadline_at_ms)?),
        state: serde_json::from_value(
            serde_json::to_value(attempt.phase).map_err(|_| StorageError::InvalidRecord)?,
        )
        .map_err(|_| StorageError::InvalidRecord)?,
        cessation: serde_json::from_value(
            serde_json::to_value(attempt.effects.cessation)
                .map_err(|_| StorageError::InvalidRecord)?,
        )
        .map_err(|_| StorageError::InvalidRecord)?,
        retry_eligible: retry_allowed && safely_stopped,
        explanation: attempt.explanation,
        summary_run_id: (attempt.phase == agent_automation::SummaryPhase::Completed)
            .then(|| run_id.clone()),
    })
}
fn timestamp(value: i64) -> Result<communication_protocol::ObservationTimestamp, StorageError> {
    crate::wakeup_projection::timestamp(value).map_err(|_| StorageError::InvalidRecord)
}

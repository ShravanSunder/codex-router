//! Validate and project selected route evidence for SDK and CLI inspection.
use agent_automation::{AttemptOutcome, DeliveryStatus};
use automation_storage::DeliveryRecord;
use collaboration_protocol::{
    CodexGeneration, DeliveryDisposition, DeliveryEvidence, DeliveryInspection, DeliverySource,
    MessageDelivery, SessionRef,
};
pub(crate) fn snapshot(
    record: DeliveryRecord<
        SessionRef,
        CodexGeneration,
        crate::stored_delivery_receipt::StoredDeliveryReceipt,
    >,
) -> Result<DeliveryInspection, ()> {
    let mode = match record.mode.as_str() {
        "auto" => MessageDelivery::Auto,
        "steer" => MessageDelivery::Steer,
        "queue" => MessageDelivery::Queue,
        _ => return Err(()),
    };
    let disposition = match record.status {
        DeliveryStatus::Pending | DeliveryStatus::Retryable | DeliveryStatus::Dispatching => {
            DeliveryDisposition::Pending
        }
        DeliveryStatus::Accepted => DeliveryDisposition::Accepted,
        DeliveryStatus::Discarded => DeliveryDisposition::Discarded,
        DeliveryStatus::Failed => DeliveryDisposition::Failed,
        DeliveryStatus::Uncertain => DeliveryDisposition::Uncertain,
    };
    let evidence = match record.attempt {
        None if matches!(
            record.status,
            DeliveryStatus::Pending | DeliveryStatus::Discarded
        ) =>
        {
            DeliveryEvidence::NotDispatched
        }
        None => return Err(()),
        Some(attempt) if attempt.effects.is_none() => match attempt.outcome {
            AttemptOutcome::InProgress if record.status == DeliveryStatus::Dispatching => {
                DeliveryEvidence::Dispatching {
                    attempt_id: attempt.attempt_id,
                    effects: None,
                }
            }
            AttemptOutcome::KnownNotSubmitted { reason, .. }
                if matches!(
                    record.status,
                    DeliveryStatus::Retryable | DeliveryStatus::Failed | DeliveryStatus::Discarded
                ) =>
            {
                DeliveryEvidence::KnownNotSubmitted {
                    attempt_id: attempt.attempt_id,
                    reason,
                    effects: None,
                    receipt: record.receipt.map(|stored| stored.into_public()),
                }
            }
            _ => return Err(()),
        },
        Some(attempt) => {
            let effects =
                crate::delivery_route_projection::project(attempt.effects.as_ref().ok_or(())?)?;
            match attempt.outcome {
                AttemptOutcome::InProgress if record.status == DeliveryStatus::Dispatching => {
                    DeliveryEvidence::Dispatching {
                        attempt_id: attempt.attempt_id,
                        effects: Some(effects),
                    }
                }
                AttemptOutcome::Accepted if record.status == DeliveryStatus::Accepted => {
                    DeliveryEvidence::Accepted {
                        attempt_id: attempt.attempt_id,
                        receipt: record.receipt.ok_or(())?.into_public(),
                    }
                }
                AttemptOutcome::KnownNotSubmitted { reason, .. }
                    if matches!(
                        record.status,
                        DeliveryStatus::Retryable
                            | DeliveryStatus::Failed
                            | DeliveryStatus::Discarded
                    ) =>
                {
                    DeliveryEvidence::KnownNotSubmitted {
                        attempt_id: attempt.attempt_id,
                        reason,
                        effects: Some(effects),
                        receipt: record.receipt.map(|stored| stored.into_public()),
                    }
                }
                AttemptOutcome::Unknown { reason }
                    if record.status == DeliveryStatus::Uncertain =>
                {
                    DeliveryEvidence::OutcomeUnknown {
                        attempt_id: attempt.attempt_id,
                        effects,
                        explanation: reason,
                        receipt: record.receipt.map(|stored| stored.into_public()),
                    }
                }
                _ => return Err(()),
            }
        }
    };
    Ok(DeliveryInspection {
        delivery_id: record.delivery_id,
        target: record.target,
        mode,
        source: DeliverySource::Wake {
            wakeup_id: record.wakeup_id,
            occurrence_id: record.occurrence_id,
        },
        eligible_at: crate::wakeup_projection::timestamp(record.eligible_at_ms)?,
        expires_at: record
            .expires_at_ms
            .map(crate::wakeup_projection::timestamp)
            .transpose()?,
        disposition,
        evidence,
    })
}

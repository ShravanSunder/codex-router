//! Map a client-neutral accepted outcome to storage's typed acceptance class.
use agent_automation::AcceptedDeliveryEffect;
use collaboration_protocol::DeliveryOutcome;

pub(crate) fn accepted_delivery_effect(
    outcome: &DeliveryOutcome,
) -> Option<AcceptedDeliveryEffect> {
    match outcome {
        DeliveryOutcome::Started => Some(AcceptedDeliveryEffect::Started),
        DeliveryOutcome::Steered => Some(AcceptedDeliveryEffect::Steered),
        DeliveryOutcome::StartedOrSteered => Some(AcceptedDeliveryEffect::StartedOrSteered),
        DeliveryOutcome::Queued => Some(AcceptedDeliveryEffect::Queued),
        DeliveryOutcome::PeerMessageWritten => Some(AcceptedDeliveryEffect::PeerMessageWritten),
        DeliveryOutcome::NotSubmitted { .. }
        | DeliveryOutcome::Rejected(_)
        | DeliveryOutcome::Unknown => None,
    }
}

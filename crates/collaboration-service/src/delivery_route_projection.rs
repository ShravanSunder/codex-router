//! Convert persisted route evidence to the public inspection shape.
use agent_automation::RouteEffectEvidence;
use collaboration_protocol::{
    CodexGeneration, DeliveryRouteEvidence, OperationId, ProviderBindingId, SessionRef,
};

pub(crate) fn project(
    evidence: &RouteEffectEvidence<SessionRef, CodexGeneration>,
) -> Result<DeliveryRouteEvidence, ()> {
    match evidence {
        RouteEffectEvidence::CodexAppServer(native) => Ok(DeliveryRouteEvidence::CodexAppServer(
            serde_json::from_value(serde_json::to_value(native).map_err(|_| ())?)
                .map_err(|_| ())?,
        )),
        RouteEffectEvidence::ProviderAcp(provider) => Ok(DeliveryRouteEvidence::ProviderAcp {
            binding_id: ProviderBindingId::try_from(provider.binding.as_str().to_owned())
                .map_err(|_| ())?,
            generation: provider.generation.clone(),
            target: provider.target.clone(),
            operation_id: OperationId::try_from(provider.attempt_id.as_str().to_owned())
                .map_err(|_| ())?,
            submission: serde_json::from_value(
                serde_json::to_value(provider.submission).map_err(|_| ())?,
            )
            .map_err(|_| ())?,
        }),
        RouteEffectEvidence::ClaudeCodePeer(peer) => Ok(DeliveryRouteEvidence::ClaudeCodePeer {
            session_id: peer.session_id.clone(),
            write: peer.write,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_automation::{
        AttemptId, AttemptOutcome, DeliveryAttempt, DeliveryId, DeliveryStatus, OccurrenceId,
        WakeupId,
    };
    use automation_storage::DeliveryRecord;
    use collaboration_protocol::{
        DeliveryClientReceipt, DeliveryEvidence, DeliveryOutcome, DeliveryReceipt,
        SessionReachability,
    };
    use serde_json::json;

    fn inspect_selected(
        target: SessionRef,
        attempt: DeliveryAttempt<SessionRef, CodexGeneration>,
        receipt: DeliveryReceipt,
    ) -> Result<collaboration_protocol::DeliveryInspection, Box<dyn std::error::Error>> {
        let record = DeliveryRecord {
            delivery_id: DeliveryId::generate(),
            wakeup_id: WakeupId::generate(),
            occurrence_id: OccurrenceId::generate(),
            target,
            mode: "auto".into(),
            status: DeliveryStatus::Accepted,
            eligible_at_ms: 1_000,
            expires_at_ms: None,
            attempt: Some(attempt),
            receipt: Some(crate::stored_delivery_receipt::StoredDeliveryReceipt::Current(receipt)),
        };
        crate::delivery_projection::snapshot(record)
            .map_err(|_| "selected route projection rejected stored evidence".into())
    }

    fn inspect_dispatching(
        target: SessionRef,
        effects: RouteEffectEvidence<SessionRef, CodexGeneration>,
    ) -> Result<collaboration_protocol::DeliveryInspection, Box<dyn std::error::Error>> {
        let record = DeliveryRecord::<_, _, crate::stored_delivery_receipt::StoredDeliveryReceipt> {
            delivery_id: DeliveryId::generate(),
            wakeup_id: WakeupId::generate(),
            occurrence_id: OccurrenceId::generate(),
            target,
            mode: "auto".into(),
            status: DeliveryStatus::Dispatching,
            eligible_at_ms: 1_000,
            expires_at_ms: None,
            attempt: Some(DeliveryAttempt {
                attempt_id: AttemptId::generate(),
                attempt_number: 1,
                started_at_ms: 1_000,
                completed_at_ms: None,
                discard_on_non_submission: false,
                effects: Some(effects),
                outcome: AttemptOutcome::InProgress,
            }),
            receipt: None,
        };
        crate::delivery_projection::snapshot(record)
            .map_err(|_| "dispatching route projection rejected stored evidence".into())
    }

    #[test]
    fn rejected_unselected_attempt_keeps_structured_receipt()
    -> Result<(), Box<dyn std::error::Error>> {
        let target: SessionRef = serde_json::from_value(json!({
            "endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"claude-local"},
            "sessionId":"target"
        }))?;
        let receipt: DeliveryReceipt = serde_json::from_value(json!({
            "outcome":{"kind":"rejected","reason":"noRoute","nextAction":"correctRequest","clientCode":null,"detail":"no route"},
            "reachability":null,"client":null
        }))?;
        let record = DeliveryRecord::<_, _, crate::stored_delivery_receipt::StoredDeliveryReceipt> {
            delivery_id: DeliveryId::generate(),
            wakeup_id: WakeupId::generate(),
            occurrence_id: OccurrenceId::generate(),
            target,
            mode: "auto".into(),
            status: DeliveryStatus::Failed,
            eligible_at_ms: 1_000,
            expires_at_ms: None,
            attempt: Some(DeliveryAttempt {
                attempt_id: AttemptId::generate(),
                attempt_number: 1,
                started_at_ms: 1_000,
                completed_at_ms: Some(1_001),
                discard_on_non_submission: false,
                effects: None,
                outcome: AttemptOutcome::KnownNotSubmitted {
                    reason: "no route".into(),
                    retryable: false,
                },
            }),
            receipt: Some(crate::stored_delivery_receipt::StoredDeliveryReceipt::Current(receipt)),
        };
        let inspection =
            crate::delivery_projection::snapshot(record).map_err(|_| "projection failed")?;
        let DeliveryEvidence::KnownNotSubmitted {
            effects: None,
            receipt: Some(receipt),
            ..
        } = inspection.evidence
        else {
            return Err("structured rejection was lost".into());
        };
        if !matches!(receipt.outcome, DeliveryOutcome::Rejected(_)) {
            return Err("rejection changed outcome".into());
        }
        Ok(())
    }

    #[test]
    fn provider_and_peer_attempts_project_through_public_inspection()
    -> Result<(), Box<dyn std::error::Error>> {
        let service = "00000000-0000-4000-8000-000000000001";
        let generation: CodexGeneration =
            serde_json::from_value(json!({"serviceEpoch":service,"generation":1}))?;
        let provider_target: SessionRef = serde_json::from_value(json!({
            "endpoint":{"serviceId":service,"endpointId":"claude-local"},"sessionId":"provider-thread"
        }))?;
        let provider_attempt = AttemptId::generate();
        let provider_effects: RouteEffectEvidence<SessionRef, CodexGeneration> =
            serde_json::from_value(json!({
                "kind":"providerAcp","target":provider_target,"generation":generation,
                "binding":"binding-one","attemptId":provider_attempt,
                "submission":"accepted","settlement":"notObserved"
            }))?;
        let provider_dispatch =
            inspect_dispatching(provider_target.clone(), provider_effects.clone())?;
        if !matches!(
            provider_dispatch.evidence,
            DeliveryEvidence::Dispatching {
                effects: Some(DeliveryRouteEvidence::ProviderAcp { .. }),
                ..
            }
        ) {
            return Err("provider attempt did not expose provider route evidence".into());
        }
        let provider_operation = OperationId::try_from(provider_attempt.as_str().to_owned())?;
        let provider = inspect_selected(
            provider_target,
            DeliveryAttempt {
                attempt_id: provider_attempt,
                attempt_number: 1,
                started_at_ms: 1_000,
                completed_at_ms: Some(2_000),
                discard_on_non_submission: false,
                effects: Some(provider_effects),
                outcome: AttemptOutcome::Accepted,
            },
            DeliveryReceipt {
                outcome: DeliveryOutcome::Started,
                reachability: Some(SessionReachability::ProviderAcp),
                client: Some(DeliveryClientReceipt::ProviderAcp {
                    operation_id: provider_operation,
                }),
            },
        )?;
        if !matches!(provider.evidence, DeliveryEvidence::Accepted { receipt, .. }
            if matches!(receipt.client, Some(DeliveryClientReceipt::ProviderAcp { .. })))
        {
            return Err("provider acceptance lost operation identity".into());
        }

        let peer_target: SessionRef = serde_json::from_value(json!({
            "endpoint":{"serviceId":service,"endpointId":"claude-local"},"sessionId":"peer-thread"
        }))?;
        let peer_effects: RouteEffectEvidence<SessionRef, CodexGeneration> =
            serde_json::from_value(json!({
                "kind":"claudeCodePeer","sessionId":"peer-thread","processId":42,"write":"written"
            }))?;
        let peer_dispatch = inspect_dispatching(peer_target.clone(), peer_effects.clone())?;
        if !matches!(
            peer_dispatch.evidence,
            DeliveryEvidence::Dispatching {
                effects: Some(DeliveryRouteEvidence::ClaudeCodePeer { .. }),
                ..
            }
        ) {
            return Err("peer attempt did not expose peer route evidence".into());
        }
        let peer = inspect_selected(
            peer_target,
            DeliveryAttempt {
                attempt_id: AttemptId::generate(),
                attempt_number: 1,
                started_at_ms: 1_000,
                completed_at_ms: Some(2_000),
                discard_on_non_submission: false,
                effects: Some(peer_effects),
                outcome: AttemptOutcome::Accepted,
            },
            DeliveryReceipt {
                outcome: DeliveryOutcome::PeerMessageWritten,
                reachability: Some(SessionReachability::ClaudeCodePeer),
                client: Some(DeliveryClientReceipt::ClaudeCodePeer),
            },
        )?;
        if !matches!(peer.evidence, DeliveryEvidence::Accepted { receipt, .. }
            if matches!(receipt.client, Some(DeliveryClientReceipt::ClaudeCodePeer)))
        {
            return Err("peer write gained a completion claim or lost its client".into());
        }
        Ok(())
    }
}

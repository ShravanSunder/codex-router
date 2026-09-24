//! An attempt may finish only with evidence from its selected client.
use agent_automation::{
    AcceptedDeliveryEffect, PeerWriteEffect, PreparationEffect, RouteEffectEvidence,
    SubmissionEffect,
};

pub(crate) enum DeliveryEvidenceResult {
    Accepted(AcceptedDeliveryEffect),
    KnownNotSubmitted,
    Unknown,
}

pub(crate) fn allows_completion<TTarget: PartialEq, TGeneration: PartialEq>(
    recorded: Option<&RouteEffectEvidence<TTarget, TGeneration>>,
    reported: Option<&RouteEffectEvidence<TTarget, TGeneration>>,
    result: DeliveryEvidenceResult,
) -> bool {
    let route_matches = match (recorded, reported) {
        (None, None) => return matches!(result, DeliveryEvidenceResult::KnownNotSubmitted),
        (None, Some(_)) => matches!(result, DeliveryEvidenceResult::KnownNotSubmitted),
        (
            Some(RouteEffectEvidence::CodexAppServer(_)),
            Some(RouteEffectEvidence::CodexAppServer(_)),
        ) => true,
        (
            Some(RouteEffectEvidence::ProviderAcp(before)),
            Some(RouteEffectEvidence::ProviderAcp(after)),
        ) => {
            before.target == after.target
                && before.generation == after.generation
                && before.binding == after.binding
                && before.attempt_id == after.attempt_id
        }
        (
            Some(RouteEffectEvidence::ClaudeCodePeer(before)),
            Some(RouteEffectEvidence::ClaudeCodePeer(after)),
        ) => before.session_id == after.session_id && before.process_id == after.process_id,
        _ => false,
    };
    if !route_matches {
        return false;
    }
    match (reported, result) {
        (
            Some(RouteEffectEvidence::CodexAppServer(native)),
            DeliveryEvidenceResult::Accepted(effect),
        ) => {
            native.submission == SubmissionEffect::Accepted
                && matches!(
                    effect,
                    AcceptedDeliveryEffect::Started
                        | AcceptedDeliveryEffect::Steered
                        | AcceptedDeliveryEffect::StartedOrSteered
                        | AcceptedDeliveryEffect::Queued
                )
        }
        (
            Some(RouteEffectEvidence::CodexAppServer(native)),
            DeliveryEvidenceResult::KnownNotSubmitted,
        ) => matches!(
            native.submission,
            SubmissionEffect::NotDispatched | SubmissionEffect::Rejected
        ),
        (Some(RouteEffectEvidence::CodexAppServer(native)), DeliveryEvidenceResult::Unknown) => {
            native.submission == SubmissionEffect::Unknown
                || native.resume == PreparationEffect::Unknown
                || native.allocation == PreparationEffect::Unknown
        }
        (
            Some(RouteEffectEvidence::ProviderAcp(provider)),
            DeliveryEvidenceResult::Accepted(effect),
        ) => {
            matches!(
                (recorded, provider.submission, effect),
                (
                    Some(RouteEffectEvidence::ProviderAcp(before)),
                    SubmissionEffect::RouterQueued,
                    AcceptedDeliveryEffect::Queued,
                ) if before.submission == SubmissionEffect::RouterQueued
            ) || matches!(
                (recorded, provider.submission, effect),
                (
                    Some(RouteEffectEvidence::ProviderAcp(before)),
                    SubmissionEffect::Accepted,
                    AcceptedDeliveryEffect::Started | AcceptedDeliveryEffect::Steered,
                ) if before.submission == SubmissionEffect::Dispatching
            )
        }
        (
            Some(RouteEffectEvidence::ProviderAcp(provider)),
            DeliveryEvidenceResult::KnownNotSubmitted,
        ) => {
            matches!(
                provider.submission,
                SubmissionEffect::NotDispatched | SubmissionEffect::Rejected
            )
        }
        (Some(RouteEffectEvidence::ProviderAcp(provider)), DeliveryEvidenceResult::Unknown) => {
            provider.submission == SubmissionEffect::Unknown
        }
        (
            Some(RouteEffectEvidence::ClaudeCodePeer(peer)),
            DeliveryEvidenceResult::Accepted(effect),
        ) => {
            peer.write == PeerWriteEffect::Written
                && effect == AcceptedDeliveryEffect::PeerMessageWritten
        }
        (
            Some(RouteEffectEvidence::ClaudeCodePeer(peer)),
            DeliveryEvidenceResult::KnownNotSubmitted,
        ) => peer.write == PeerWriteEffect::NotDispatched,
        (Some(RouteEffectEvidence::ClaudeCodePeer(peer)), DeliveryEvidenceResult::Unknown) => {
            peer.write == PeerWriteEffect::Unknown
        }
        (None, DeliveryEvidenceResult::KnownNotSubmitted) => true,
        (None, _) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_automation::{
        AttemptId, CessationEvidence, ClaudeCodePeerEffectEvidence, NativeEffectEvidence,
        ProviderAcpEffectEvidence, ProviderBindingReference, ProviderSettlementEffect,
    };

    fn provider(submission: SubmissionEffect) -> RouteEffectEvidence<String, String> {
        RouteEffectEvidence::ProviderAcp(ProviderAcpEffectEvidence {
            target: Some("target".to_owned()),
            generation: "generation".to_owned(),
            binding: ProviderBindingReference::try_from("binding".to_owned()).expect("binding"),
            attempt_id: AttemptId::try_from("018f1f62-6571-7ef0-8f0c-001122334499".to_owned())
                .expect("attempt"),
            submission,
            settlement: ProviderSettlementEffect::NotObserved,
        })
    }

    fn native(submission: SubmissionEffect) -> RouteEffectEvidence<String, String> {
        RouteEffectEvidence::CodexAppServer(NativeEffectEvidence {
            target: Some("target".to_owned()),
            generation: Some("generation".to_owned()),
            client_user_message_id: None,
            native_turn_id: None,
            native_submission_id: None,
            allocation: PreparationEffect::NotRequested,
            resume: PreparationEffect::NotRequested,
            submission,
            cessation: CessationEvidence::NotApplicable,
        })
    }

    fn peer(write: PeerWriteEffect) -> RouteEffectEvidence<String, String> {
        RouteEffectEvidence::ClaudeCodePeer(ClaudeCodePeerEffectEvidence {
            session_id: "target".to_owned().try_into().expect("peer session"),
            process_id: 42.try_into().expect("process"),
            write,
        })
    }

    #[test]
    fn accepted_effect_matches_the_selected_client_evidence() {
        let native_before = native(SubmissionEffect::Dispatching);
        let native_after = native(SubmissionEffect::Accepted);
        let provider_before = provider(SubmissionEffect::Dispatching);
        let provider_after = provider(SubmissionEffect::Accepted);
        let queued_before = provider(SubmissionEffect::RouterQueued);
        let queued_after = provider(SubmissionEffect::RouterQueued);
        let peer_before = peer(PeerWriteEffect::Dispatching);
        let peer_after = peer(PeerWriteEffect::Written);
        let effects = [
            AcceptedDeliveryEffect::Started,
            AcceptedDeliveryEffect::Steered,
            AcceptedDeliveryEffect::StartedOrSteered,
            AcceptedDeliveryEffect::Queued,
            AcceptedDeliveryEffect::PeerMessageWritten,
        ];
        for effect in effects {
            let native_legal = effect != AcceptedDeliveryEffect::PeerMessageWritten;
            assert_eq!(
                allows_completion(
                    Some(&native_before),
                    Some(&native_after),
                    DeliveryEvidenceResult::Accepted(effect)
                ),
                native_legal,
                "native {effect:?}"
            );
            let provider_legal = matches!(
                effect,
                AcceptedDeliveryEffect::Started | AcceptedDeliveryEffect::Steered
            );
            assert_eq!(
                allows_completion(
                    Some(&provider_before),
                    Some(&provider_after),
                    DeliveryEvidenceResult::Accepted(effect)
                ),
                provider_legal,
                "provider {effect:?}"
            );
            assert_eq!(
                allows_completion(
                    Some(&queued_before),
                    Some(&queued_after),
                    DeliveryEvidenceResult::Accepted(effect)
                ),
                effect == AcceptedDeliveryEffect::Queued,
                "router queue {effect:?}"
            );
            assert_eq!(
                allows_completion(
                    Some(&peer_before),
                    Some(&peer_after),
                    DeliveryEvidenceResult::Accepted(effect)
                ),
                effect == AcceptedDeliveryEffect::PeerMessageWritten,
                "peer {effect:?}"
            );
        }
    }
}

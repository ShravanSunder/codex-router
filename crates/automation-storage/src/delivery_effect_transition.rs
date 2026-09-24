//! An attempt may finish only with evidence from its selected client.
use agent_automation::{PeerWriteEffect, PreparationEffect, RouteEffectEvidence, SubmissionEffect};

pub(crate) enum DeliveryEvidenceResult {
    Accepted,
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
        (Some(RouteEffectEvidence::CodexAppServer(native)), DeliveryEvidenceResult::Accepted) => {
            native.submission == SubmissionEffect::Accepted
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
        (Some(RouteEffectEvidence::ProviderAcp(provider)), DeliveryEvidenceResult::Accepted) => {
            provider.submission == SubmissionEffect::Accepted
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
        (Some(RouteEffectEvidence::ClaudeCodePeer(peer)), DeliveryEvidenceResult::Accepted) => {
            peer.write == PeerWriteEffect::Written
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

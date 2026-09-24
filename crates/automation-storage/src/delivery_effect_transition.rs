//! An attempt may finish only with evidence from its selected client.
use agent_automation::{PeerWriteEffect, PreparationEffect, RouteEffectEvidence, SubmissionEffect};

pub(crate) enum DeliveryEvidenceResult {
    Accepted,
    KnownNotSubmitted,
    Unknown,
}

pub(crate) fn allows_completion<TTarget: PartialEq, TGeneration: PartialEq>(
    recorded: Option<&RouteEffectEvidence<TTarget, TGeneration>>,
    reported: &RouteEffectEvidence<TTarget, TGeneration>,
    result: DeliveryEvidenceResult,
) -> bool {
    let route_matches = match (recorded, reported) {
        (None, _) => matches!(result, DeliveryEvidenceResult::KnownNotSubmitted),
        (Some(RouteEffectEvidence::CodexAppServer(_)), RouteEffectEvidence::CodexAppServer(_)) => {
            true
        }
        (
            Some(RouteEffectEvidence::ProviderAcp(before)),
            RouteEffectEvidence::ProviderAcp(after),
        ) => {
            before.target == after.target
                && before.generation == after.generation
                && before.binding == after.binding
                && before.attempt_id == after.attempt_id
        }
        (
            Some(RouteEffectEvidence::ClaudeCodePeer(before)),
            RouteEffectEvidence::ClaudeCodePeer(after),
        ) => before.session_id == after.session_id && before.process_id == after.process_id,
        _ => false,
    };
    if !route_matches {
        return false;
    }
    match (reported, result) {
        (RouteEffectEvidence::CodexAppServer(native), DeliveryEvidenceResult::Accepted) => {
            native.submission == SubmissionEffect::Accepted
        }
        (
            RouteEffectEvidence::CodexAppServer(native),
            DeliveryEvidenceResult::KnownNotSubmitted,
        ) => matches!(
            native.submission,
            SubmissionEffect::NotDispatched | SubmissionEffect::Rejected
        ),
        (RouteEffectEvidence::CodexAppServer(native), DeliveryEvidenceResult::Unknown) => {
            native.submission == SubmissionEffect::Unknown
                || native.resume == PreparationEffect::Unknown
                || native.allocation == PreparationEffect::Unknown
        }
        (RouteEffectEvidence::ProviderAcp(provider), DeliveryEvidenceResult::Accepted) => {
            provider.submission == SubmissionEffect::Accepted
        }
        (RouteEffectEvidence::ProviderAcp(provider), DeliveryEvidenceResult::KnownNotSubmitted) => {
            matches!(
                provider.submission,
                SubmissionEffect::NotDispatched | SubmissionEffect::Rejected
            )
        }
        (RouteEffectEvidence::ProviderAcp(provider), DeliveryEvidenceResult::Unknown) => {
            provider.submission == SubmissionEffect::Unknown
        }
        (RouteEffectEvidence::ClaudeCodePeer(peer), DeliveryEvidenceResult::Accepted) => {
            peer.write == PeerWriteEffect::Written
        }
        (RouteEffectEvidence::ClaudeCodePeer(peer), DeliveryEvidenceResult::KnownNotSubmitted) => {
            peer.write == PeerWriteEffect::NotDispatched
        }
        (RouteEffectEvidence::ClaudeCodePeer(peer), DeliveryEvidenceResult::Unknown) => {
            peer.write == PeerWriteEffect::Unknown
        }
    }
}

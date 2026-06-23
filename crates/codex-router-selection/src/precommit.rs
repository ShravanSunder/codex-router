//! Explicit precommit rotation classification.

/// Failure class observed before a response is committed to Codex.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrecommitFailure {
    /// Upstream rejected the selected account credentials.
    AuthenticationRejected,
    /// Selected account is exhausted for the requested route.
    QuotaExhausted,
    /// Timeout belongs to Codex/provider behavior, not router gating.
    Timeout,
    /// Response could not be classified safely.
    MalformedResponse,
}

/// Router action after a precommit failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrecommitRotationDecision {
    /// Retry with another eligible account.
    RotateAccount,
    /// Return the failure to Codex without inventing router policy.
    ReturnToCodex,
}

/// Classifies whether a precommit failure may rotate accounts.
#[must_use]
pub const fn classify_precommit_failure(failure: PrecommitFailure) -> PrecommitRotationDecision {
    match failure {
        PrecommitFailure::AuthenticationRejected | PrecommitFailure::QuotaExhausted => {
            PrecommitRotationDecision::RotateAccount
        }
        PrecommitFailure::Timeout | PrecommitFailure::MalformedResponse => {
            PrecommitRotationDecision::ReturnToCodex
        }
    }
}

/// Named precommit classifier used by proxy integration.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PrecommitFailureClassifier;

impl PrecommitFailureClassifier {
    /// Classifies whether a precommit failure may rotate accounts.
    #[must_use]
    pub const fn classify(self, failure: PrecommitFailure) -> PrecommitRotationDecision {
        classify_precommit_failure(failure)
    }
}

//! Provider-independent outcomes used by the request attempt policy.

use crate::route_profile::WindowKind;

/// Evidence returned by a provider edge before response commitment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AttemptOutcome {
    /// A 2xx response is ready to commit; completion success is confirmed separately.
    Success,
    /// Explicit evidence attributes rejection to these shared usage windows.
    SharedWindowExhausted {
        /// Rejected shared window identities.
        windows: Vec<WindowKind>,
        /// Reported reset times in UTC Unix seconds, aligned with `windows`.
        resets: Vec<Option<u64>>,
    },
    /// Complete evidence identifies an invalid or expired OAuth credential.
    CredentialRejected,
    /// The provider response must reach the client without recovery.
    PassThrough(PassThroughReason),
    /// The provider returned no response; recovery is prohibited.
    NoResponse(TransportFailure),
}

/// Why a provider rejection cannot justify account recovery.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PassThroughReason {
    /// Provider overload.
    Overloaded,
    /// Provider server failure.
    ServerError,
    /// Request-rate throttling.
    RateThrottled,
    /// Model-specific or overage-only limit.
    ModelOrOverageLimit,
    /// Invalid or unsupported request.
    RequestRejected,
    /// A usage limit without account-window attribution.
    UnattributedLimit,
    /// Absent, incomplete, malformed, or contradictory attribution evidence.
    MalformedEvidence,
}

/// Redacted transport failure before any provider response arrived.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransportFailure {
    /// A connection to the provider could not be established or completed.
    Connection,
    /// The attempt timed out without receiving a response.
    Timeout,
}

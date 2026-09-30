//! Non-secret health and provider-use claim for one active credential generation.

/// Login claim age after which maintenance may reclaim an abandoned OAuth activation.
pub const LOGIN_CREDENTIAL_CLAIM_TIMEOUT_SECONDS: u64 = 300;

/// The durable renewal state for an account's current credential generation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CredentialMaintenanceState {
    Healthy,
    Retrying,
    ReauthRequired,
    Unrefreshable,
    InProgress,
}

impl CredentialMaintenanceState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Retrying => "retrying",
            Self::ReauthRequired => "reauth_required",
            Self::Unrefreshable => "unrefreshable",
            Self::InProgress => "in_progress",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "healthy" => Some(Self::Healthy),
            "retrying" => Some(Self::Retrying),
            "reauth_required" => Some(Self::ReauthRequired),
            "unrefreshable" => Some(Self::Unrefreshable),
            "in_progress" => Some(Self::InProgress),
            _ => None,
        }
    }
}

/// Secret-safe classification of the most recent renewal failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CredentialFailureClass {
    TransportUnspent,
    RateLimited,
    ProviderTemporary,
    ProviderRejected,
    ProviderOutcomeAmbiguous,
    MalformedResponse,
    LocalPersistence,
    RotationCommitFailed,
}

impl CredentialFailureClass {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TransportUnspent => "transport_unspent",
            Self::RateLimited => "rate_limited",
            Self::ProviderTemporary => "provider_temporary",
            Self::ProviderRejected => "provider_rejected",
            Self::ProviderOutcomeAmbiguous => "provider_outcome_ambiguous",
            Self::MalformedResponse => "malformed_response",
            Self::LocalPersistence => "local_persistence",
            Self::RotationCommitFailed => "rotation_commit_failed",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "transport_unspent" => Some(Self::TransportUnspent),
            "rate_limited" => Some(Self::RateLimited),
            "provider_temporary" => Some(Self::ProviderTemporary),
            "provider_rejected" => Some(Self::ProviderRejected),
            "provider_outcome_ambiguous" => Some(Self::ProviderOutcomeAmbiguous),
            "malformed_response" => Some(Self::MalformedResponse),
            "local_persistence" => Some(Self::LocalPersistence),
            "rotation_commit_failed" => Some(Self::RotationCommitFailed),
            _ => None,
        }
    }
}

/// Persisted claim and operator-visible health, keyed to the active generation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CredentialMaintenanceRecord {
    pub credential_generation: u64,
    pub state: CredentialMaintenanceState,
    pub failure_class: Option<CredentialFailureClass>,
    pub last_success_unix_seconds: Option<u64>,
    pub next_attempt_unix_seconds: Option<u64>,
    pub claimed_successor_generation: Option<u64>,
    pub claim_purpose: Option<ClaimPurpose>,
    pub claim_started_unix_seconds: Option<u64>,
    pub claim_prior_state: Option<CredentialMaintenanceState>,
    pub consecutive_failures: u32,
}

/// Why a credential generation is being claimed.
///
/// This value selects guards for a claim stored in the existing
/// `credential_maintenance` row.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClaimPurpose {
    Refresh,
    Login,
}

impl ClaimPurpose {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Refresh => "refresh",
            Self::Login => "login",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "refresh" => Some(Self::Refresh),
            "login" => Some(Self::Login),
            _ => None,
        }
    }
}

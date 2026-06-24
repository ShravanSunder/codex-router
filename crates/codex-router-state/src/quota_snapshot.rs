//! SQLite quota snapshot DTOs.

use codex_router_core::ids::AccountId;

use crate::account::AccountStatus;

/// Source that produced a persisted quota snapshot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuotaSnapshotSource {
    /// Deterministic mock endpoint.
    MockEndpoint,
    /// OpenAI quota endpoint.
    OpenAiEndpoint,
    /// Stale marker written after credential mutation.
    CredentialMutation,
}

impl QuotaSnapshotSource {
    /// Serializes source to SQLite.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MockEndpoint => "mock_endpoint",
            Self::OpenAiEndpoint => "openai_endpoint",
            Self::CredentialMutation => "credential_mutation",
        }
    }

    /// Parses source from SQLite.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "mock_endpoint" => Some(Self::MockEndpoint),
            "openai_endpoint" => Some(Self::OpenAiEndpoint),
            "credential_mutation" => Some(Self::CredentialMutation),
            _ => None,
        }
    }
}

/// Durable quota snapshot row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PersistedQuotaSnapshot {
    account_id: AccountId,
    source: QuotaSnapshotSource,
    observed_unix_seconds: u64,
    route_band: String,
    remaining_headroom: u32,
    reset_unix_seconds: Option<u64>,
    reset_credits_available: Option<u32>,
    stale_penalty: bool,
}

/// Selector-facing quota window status.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SelectorQuotaWindowStatus {
    /// Window can be used for account selection.
    Eligible,
    /// Window is stale but may be used as a fallback.
    Stale,
    /// Window must not be used for selection.
    Ineligible,
    /// Window state is unknown.
    Unknown,
}

/// Redacted provider refresh failure class.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuotaRefreshErrorClass {
    /// Provider returned a non-auth quota error.
    ProviderError,
    /// Credential resolution or provider auth failed.
    AuthError,
    /// Network transport failed.
    NetworkError,
    /// Provider response could not be parsed.
    ParseError,
    /// Provider asked us to slow down.
    RateLimited,
}

impl QuotaRefreshErrorClass {
    /// Serializes error class to SQLite.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ProviderError => "provider_error",
            Self::AuthError => "auth_error",
            Self::NetworkError => "network_error",
            Self::ParseError => "parse_error",
            Self::RateLimited => "rate_limited",
        }
    }

    /// Parses error class from SQLite.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "provider_error" => Some(Self::ProviderError),
            "auth_error" => Some(Self::AuthError),
            "network_error" => Some(Self::NetworkError),
            "parse_error" => Some(Self::ParseError),
            "rate_limited" => Some(Self::RateLimited),
            _ => None,
        }
    }
}

/// Source of a refresh status view row.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuotaRefreshStatusSource {
    /// Explicit durable refresh status row.
    Recorded,
    /// Selector rows exist from an older schema, but refresh status is absent.
    LegacyMissingRefreshStatus,
}

/// Durable refresh status view for one account and route band.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QuotaRefreshStatusView {
    account_id: AccountId,
    route_band: String,
    status_source: QuotaRefreshStatusSource,
    last_success_unix_seconds: Option<u64>,
    last_attempt_unix_seconds: Option<u64>,
    last_error_class: Option<QuotaRefreshErrorClass>,
    stale_after_unix_seconds: Option<u64>,
}

impl QuotaRefreshStatusView {
    /// Creates a recorded refresh status row.
    #[must_use]
    pub fn recorded(
        account_id: AccountId,
        route_band: impl Into<String>,
        last_success_unix_seconds: Option<u64>,
        last_attempt_unix_seconds: Option<u64>,
        last_error_class: Option<QuotaRefreshErrorClass>,
        stale_after_unix_seconds: Option<u64>,
    ) -> Self {
        Self {
            account_id,
            route_band: route_band.into(),
            status_source: QuotaRefreshStatusSource::Recorded,
            last_success_unix_seconds,
            last_attempt_unix_seconds,
            last_error_class,
            stale_after_unix_seconds,
        }
    }

    /// Creates a legacy missing-status row for existing selector windows.
    #[must_use]
    pub fn legacy_missing_refresh_status(
        account_id: AccountId,
        route_band: impl Into<String>,
    ) -> Self {
        Self {
            account_id,
            route_band: route_band.into(),
            status_source: QuotaRefreshStatusSource::LegacyMissingRefreshStatus,
            last_success_unix_seconds: None,
            last_attempt_unix_seconds: None,
            last_error_class: None,
            stale_after_unix_seconds: None,
        }
    }

    /// Returns account id.
    #[must_use]
    pub const fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    /// Returns route band.
    #[must_use]
    pub fn route_band(&self) -> &str {
        &self.route_band
    }

    /// Returns source.
    #[must_use]
    pub const fn status_source(&self) -> QuotaRefreshStatusSource {
        self.status_source
    }

    /// Returns last successful refresh time.
    #[must_use]
    pub const fn last_success_unix_seconds(&self) -> Option<u64> {
        self.last_success_unix_seconds
    }

    /// Returns last attempted refresh time.
    #[must_use]
    pub const fn last_attempt_unix_seconds(&self) -> Option<u64> {
        self.last_attempt_unix_seconds
    }

    /// Returns redacted error class.
    #[must_use]
    pub const fn last_error_class(&self) -> Option<QuotaRefreshErrorClass> {
        self.last_error_class
    }

    /// Returns stale-after time.
    #[must_use]
    pub const fn stale_after_unix_seconds(&self) -> Option<u64> {
        self.stale_after_unix_seconds
    }
}

impl SelectorQuotaWindowStatus {
    /// Serializes status to SQLite.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Eligible => "eligible",
            Self::Stale => "stale",
            Self::Ineligible => "ineligible",
            Self::Unknown => "unknown",
        }
    }

    /// Parses status from SQLite.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "eligible" => Some(Self::Eligible),
            "stale" => Some(Self::Stale),
            "ineligible" => Some(Self::Ineligible),
            "unknown" => Some(Self::Unknown),
            _ => None,
        }
    }
}

/// Durable selector input for one provider quota window.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PersistedSelectorQuotaWindow {
    account_id: AccountId,
    route_band: String,
    limit_window_seconds: u64,
    status: SelectorQuotaWindowStatus,
    remaining_headroom: u32,
    reset_unix_seconds: Option<u64>,
    effective: bool,
    observed_unix_seconds: u64,
}

impl PersistedSelectorQuotaWindow {
    /// Creates selector quota window input.
    #[must_use]
    pub fn new(
        account_id: AccountId,
        route_band: impl Into<String>,
        limit_window_seconds: u64,
        status: SelectorQuotaWindowStatus,
    ) -> Self {
        Self {
            account_id,
            route_band: route_band.into(),
            limit_window_seconds,
            status,
            remaining_headroom: 0,
            reset_unix_seconds: None,
            effective: false,
            observed_unix_seconds: 0,
        }
    }

    /// Sets remaining headroom.
    #[must_use]
    pub const fn with_remaining_headroom(mut self, remaining_headroom: u32) -> Self {
        self.remaining_headroom = remaining_headroom;
        self
    }

    /// Sets reset time.
    #[must_use]
    pub const fn with_reset_unix_seconds(mut self, reset_unix_seconds: u64) -> Self {
        self.reset_unix_seconds = Some(reset_unix_seconds);
        self
    }

    /// Marks this as the effective selector window.
    #[must_use]
    pub const fn with_effective(mut self, effective: bool) -> Self {
        self.effective = effective;
        self
    }

    /// Sets observation time.
    #[must_use]
    pub const fn with_observed_unix_seconds(mut self, observed_unix_seconds: u64) -> Self {
        self.observed_unix_seconds = observed_unix_seconds;
        self
    }

    /// Returns account id.
    #[must_use]
    pub const fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    /// Returns route band.
    #[must_use]
    pub fn route_band(&self) -> &str {
        &self.route_band
    }

    /// Returns provider limit window seconds.
    #[must_use]
    pub const fn limit_window_seconds(&self) -> u64 {
        self.limit_window_seconds
    }

    /// Returns status.
    #[must_use]
    pub const fn status(&self) -> SelectorQuotaWindowStatus {
        self.status
    }

    /// Returns remaining headroom.
    #[must_use]
    pub const fn remaining_headroom(&self) -> u32 {
        self.remaining_headroom
    }

    /// Returns reset time.
    #[must_use]
    pub const fn reset_unix_seconds(&self) -> Option<u64> {
        self.reset_unix_seconds
    }

    /// Returns whether this is the effective selector window.
    #[must_use]
    pub const fn effective(&self) -> bool {
        self.effective
    }

    /// Returns observed time.
    #[must_use]
    pub const fn observed_unix_seconds(&self) -> u64 {
        self.observed_unix_seconds
    }
}

/// Durable selector input for one account and route band.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SelectorQuotaInput {
    account_id: AccountId,
    account_label: String,
    account_status: AccountStatus,
    active_credential_generation: Option<u64>,
    route_band: String,
    windows: Vec<PersistedSelectorQuotaWindow>,
}

impl SelectorQuotaInput {
    /// Creates selector input.
    #[must_use]
    pub fn new(
        account_id: AccountId,
        account_label: impl Into<String>,
        account_status: AccountStatus,
        active_credential_generation: Option<u64>,
        route_band: impl Into<String>,
        windows: Vec<PersistedSelectorQuotaWindow>,
    ) -> Self {
        Self {
            account_id,
            account_label: account_label.into(),
            account_status,
            active_credential_generation,
            route_band: route_band.into(),
            windows,
        }
    }

    /// Returns account id.
    #[must_use]
    pub const fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    /// Returns account label.
    #[must_use]
    pub fn account_label(&self) -> &str {
        &self.account_label
    }

    /// Returns account status.
    #[must_use]
    pub const fn account_status(&self) -> AccountStatus {
        self.account_status
    }

    /// Returns active credential generation.
    #[must_use]
    pub const fn active_credential_generation(&self) -> Option<u64> {
        self.active_credential_generation
    }

    /// Returns route band.
    #[must_use]
    pub fn route_band(&self) -> &str {
        &self.route_band
    }

    /// Returns selector windows.
    #[must_use]
    pub fn windows(&self) -> &[PersistedSelectorQuotaWindow] {
        &self.windows
    }
}

impl PersistedQuotaSnapshot {
    /// Creates a quota snapshot with conservative defaults.
    #[must_use]
    pub fn new(account_id: AccountId, source: QuotaSnapshotSource) -> Self {
        Self {
            account_id,
            source,
            observed_unix_seconds: 0,
            route_band: String::new(),
            remaining_headroom: 0,
            reset_unix_seconds: None,
            reset_credits_available: None,
            stale_penalty: true,
        }
    }

    /// Sets observed time.
    #[must_use]
    pub const fn with_observed_unix_seconds(mut self, observed_unix_seconds: u64) -> Self {
        self.observed_unix_seconds = observed_unix_seconds;
        self
    }

    /// Sets the route band and remaining headroom.
    #[must_use]
    pub fn with_route_band(
        mut self,
        route_band: impl Into<String>,
        remaining_headroom: u32,
    ) -> Self {
        self.route_band = route_band.into();
        self.remaining_headroom = remaining_headroom;
        self
    }

    /// Sets reset hint.
    #[must_use]
    pub const fn with_reset_unix_seconds(mut self, reset_unix_seconds: u64) -> Self {
        self.reset_unix_seconds = Some(reset_unix_seconds);
        self
    }

    /// Sets provider reset credits available.
    #[must_use]
    pub const fn with_reset_credits_available(mut self, reset_credits_available: u32) -> Self {
        self.reset_credits_available = Some(reset_credits_available);
        self
    }

    /// Sets stale penalty status.
    #[must_use]
    pub const fn with_stale_penalty(mut self, stale_penalty: bool) -> Self {
        self.stale_penalty = stale_penalty;
        self
    }

    /// Returns account id.
    #[must_use]
    pub const fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    /// Returns source.
    #[must_use]
    pub const fn source(&self) -> QuotaSnapshotSource {
        self.source
    }

    /// Returns observed time.
    #[must_use]
    pub const fn observed_unix_seconds(&self) -> u64 {
        self.observed_unix_seconds
    }

    /// Returns route band.
    #[must_use]
    pub fn route_band(&self) -> &str {
        &self.route_band
    }

    /// Returns remaining headroom.
    #[must_use]
    pub const fn remaining_headroom(&self) -> u32 {
        self.remaining_headroom
    }

    /// Returns reset hint.
    #[must_use]
    pub const fn reset_unix_seconds(&self) -> Option<u64> {
        self.reset_unix_seconds
    }

    /// Returns provider reset credits available.
    #[must_use]
    pub const fn reset_credits_available(&self) -> Option<u32> {
        self.reset_credits_available
    }

    /// Returns stale penalty status.
    #[must_use]
    pub const fn stale_penalty(&self) -> bool {
        self.stale_penalty
    }
}

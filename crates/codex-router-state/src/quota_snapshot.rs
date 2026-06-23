//! SQLite quota snapshot DTOs.

use codex_router_core::ids::AccountId;

/// Source that produced a persisted quota snapshot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuotaSnapshotSource {
    /// Deterministic mock endpoint.
    MockEndpoint,
    /// OpenAI quota endpoint.
    OpenAiEndpoint,
}

impl QuotaSnapshotSource {
    /// Serializes source to SQLite.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MockEndpoint => "mock_endpoint",
            Self::OpenAiEndpoint => "openai_endpoint",
        }
    }

    /// Parses source from SQLite.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "mock_endpoint" => Some(Self::MockEndpoint),
            "openai_endpoint" => Some(Self::OpenAiEndpoint),
            _ => None,
        }
    }
}

/// Durable quota status freshness/failure state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuotaStatusState {
    /// No quota refresh has produced usable status yet.
    Unknown,
    /// Row was produced by a fresh refresh.
    Fresh,
    /// Row is stale but still useful for local display.
    Stale,
    /// Row represents a redacted refresh failure.
    Failed,
}

impl QuotaStatusState {
    /// Serializes status to SQLite.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Fresh => "fresh",
            Self::Stale => "stale",
            Self::Failed => "failed",
        }
    }

    /// Parses status from SQLite.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "unknown" => Some(Self::Unknown),
            "fresh" => Some(Self::Fresh),
            "stale" => Some(Self::Stale),
            "failed" => Some(Self::Failed),
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
    stale_penalty: bool,
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

    /// Returns stale penalty status.
    #[must_use]
    pub const fn stale_penalty(&self) -> bool {
        self.stale_penalty
    }
}

/// Durable quota status row for local-only human quota output.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PersistedQuotaStatusRow {
    account_id: AccountId,
    source: QuotaSnapshotSource,
    observed_unix_seconds: u64,
    route_band: String,
    family: String,
    window_label: String,
    status: QuotaStatusState,
    used_percent: Option<u32>,
    remaining_headroom: u32,
    reset_unix_seconds: Option<u64>,
    limit_window_seconds: Option<u64>,
    effective: bool,
    failure_message: Option<String>,
    failure_unix_seconds: Option<u64>,
}

impl PersistedQuotaStatusRow {
    /// Creates a quota status row with conservative display defaults.
    #[must_use]
    pub fn new(
        account_id: AccountId,
        source: QuotaSnapshotSource,
        route_band: impl Into<String>,
        family: impl Into<String>,
        window_label: impl Into<String>,
    ) -> Self {
        Self {
            account_id,
            source,
            observed_unix_seconds: 0,
            route_band: route_band.into(),
            family: family.into(),
            window_label: window_label.into(),
            status: QuotaStatusState::Stale,
            used_percent: None,
            remaining_headroom: 0,
            reset_unix_seconds: None,
            limit_window_seconds: None,
            effective: false,
            failure_message: None,
            failure_unix_seconds: None,
        }
    }

    /// Sets observed time.
    #[must_use]
    pub const fn with_observed_unix_seconds(mut self, observed_unix_seconds: u64) -> Self {
        self.observed_unix_seconds = observed_unix_seconds;
        self
    }

    /// Sets status state.
    #[must_use]
    pub const fn with_status(mut self, status: QuotaStatusState) -> Self {
        self.status = status;
        self
    }

    /// Sets used percent.
    #[must_use]
    pub const fn with_used_percent(mut self, used_percent: u32) -> Self {
        self.used_percent = Some(used_percent);
        self
    }

    /// Sets remaining headroom percent.
    #[must_use]
    pub const fn with_remaining_headroom(mut self, remaining_headroom: u32) -> Self {
        self.remaining_headroom = remaining_headroom;
        self
    }

    /// Sets reset hint.
    #[must_use]
    pub const fn with_reset_unix_seconds(mut self, reset_unix_seconds: u64) -> Self {
        self.reset_unix_seconds = Some(reset_unix_seconds);
        self
    }

    /// Sets provider limit window seconds.
    #[must_use]
    pub const fn with_limit_window_seconds(mut self, limit_window_seconds: u64) -> Self {
        self.limit_window_seconds = Some(limit_window_seconds);
        self
    }

    /// Marks this row as the effective compact display row.
    #[must_use]
    pub const fn with_effective(mut self, effective: bool) -> Self {
        self.effective = effective;
        self
    }

    /// Sets redacted failure metadata.
    #[must_use]
    pub fn with_failure(
        mut self,
        failure_message: impl Into<String>,
        failure_unix_seconds: u64,
    ) -> Self {
        self.failure_message = Some(failure_message.into());
        self.failure_unix_seconds = Some(failure_unix_seconds);
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

    /// Returns quota family.
    #[must_use]
    pub fn family(&self) -> &str {
        &self.family
    }

    /// Returns window label.
    #[must_use]
    pub fn window_label(&self) -> &str {
        &self.window_label
    }

    /// Returns status state.
    #[must_use]
    pub const fn status(&self) -> QuotaStatusState {
        self.status
    }

    /// Returns used percent.
    #[must_use]
    pub const fn used_percent(&self) -> Option<u32> {
        self.used_percent
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

    /// Returns provider limit window seconds.
    #[must_use]
    pub const fn limit_window_seconds(&self) -> Option<u64> {
        self.limit_window_seconds
    }

    /// Returns whether this row is the effective compact display row.
    #[must_use]
    pub const fn effective(&self) -> bool {
        self.effective
    }

    /// Returns redacted failure message.
    #[must_use]
    pub fn failure_message(&self) -> Option<&str> {
        self.failure_message.as_deref()
    }

    /// Returns failure time.
    #[must_use]
    pub const fn failure_unix_seconds(&self) -> Option<u64> {
        self.failure_unix_seconds
    }
}

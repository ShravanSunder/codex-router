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

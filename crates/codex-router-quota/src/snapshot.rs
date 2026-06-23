//! Quota snapshot freshness and headroom model.

use codex_router_core::ids::AccountId;

/// Source that produced a quota snapshot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SnapshotSource {
    /// Mock endpoint used by deterministic integration tests.
    MockEndpoint,
    /// OpenAI quota endpoint.
    OpenAiEndpoint,
}

/// Per-route remaining quota band.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QuotaRouteBand {
    route_name: String,
    remaining_headroom: u32,
}

impl QuotaRouteBand {
    /// Creates a route band.
    #[must_use]
    pub fn new(route_name: impl Into<String>, remaining_headroom: u32) -> Self {
        Self {
            route_name: route_name.into(),
            remaining_headroom,
        }
    }

    /// Returns the route name.
    #[must_use]
    pub fn route_name(&self) -> &str {
        &self.route_name
    }

    /// Returns remaining headroom for this route.
    #[must_use]
    pub const fn remaining_headroom(&self) -> u32 {
        self.remaining_headroom
    }
}

/// Freshness state used by selection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SnapshotFreshness {
    /// Snapshot is available and inside the freshness window.
    Fresh {
        /// Snapshot age in seconds.
        age_seconds: u64,
    },
    /// Snapshot exists but should be penalized by selection.
    StaleWithPenalty {
        /// Snapshot age in seconds.
        age_seconds: u64,
    },
    /// No usable snapshot exists.
    Unknown,
}

/// Quota snapshot as consumed by routing and selection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QuotaSnapshot {
    account_id: AccountId,
    source: SnapshotSource,
    observed_unix_seconds: u64,
    route_bands: Vec<QuotaRouteBand>,
    reset_unix_seconds: Option<u64>,
}

impl QuotaSnapshot {
    /// Creates a quota snapshot.
    #[must_use]
    pub fn new(account_id: AccountId, source: SnapshotSource, observed_unix_seconds: u64) -> Self {
        Self {
            account_id,
            source,
            observed_unix_seconds,
            route_bands: Vec::new(),
            reset_unix_seconds: None,
        }
    }

    /// Adds or replaces a route band.
    #[must_use]
    pub fn with_route_band(mut self, route_band: QuotaRouteBand) -> Self {
        self.route_bands
            .retain(|existing| existing.route_name() != route_band.route_name());
        self.route_bands.push(route_band);
        self
    }

    /// Records the provider reset hint.
    #[must_use]
    pub const fn with_reset_unix_seconds(mut self, reset_unix_seconds: u64) -> Self {
        self.reset_unix_seconds = Some(reset_unix_seconds);
        self
    }

    /// Returns remaining headroom for a route.
    #[must_use]
    pub fn remaining_headroom(&self, route_name: &str) -> Option<u32> {
        self.route_bands
            .iter()
            .find(|route_band| route_band.route_name() == route_name)
            .map(QuotaRouteBand::remaining_headroom)
    }

    /// Classifies this snapshot's freshness.
    #[must_use]
    pub fn freshness(&self, now_unix_seconds: u64, max_age_seconds: u64) -> SnapshotFreshness {
        Self::freshness_for_observed_at(
            Some(self.observed_unix_seconds),
            now_unix_seconds,
            max_age_seconds,
        )
    }

    /// Classifies freshness for persisted observed timestamps.
    #[must_use]
    pub const fn freshness_for_observed_at(
        observed_unix_seconds: Option<u64>,
        now_unix_seconds: u64,
        max_age_seconds: u64,
    ) -> SnapshotFreshness {
        let Some(observed_unix_seconds) = observed_unix_seconds else {
            return SnapshotFreshness::Unknown;
        };
        if observed_unix_seconds > now_unix_seconds {
            return SnapshotFreshness::Unknown;
        }

        let age_seconds = now_unix_seconds - observed_unix_seconds;
        if age_seconds <= max_age_seconds {
            return SnapshotFreshness::Fresh { age_seconds };
        }

        SnapshotFreshness::StaleWithPenalty { age_seconds }
    }

    /// Returns the account id.
    #[must_use]
    pub const fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    /// Returns the source.
    #[must_use]
    pub const fn source(&self) -> SnapshotSource {
        self.source
    }
}

//! SQLite session records responsibilities.
use super::*;
/// Active client count for one account and route band.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActiveClientCount {
    account_id: AccountId,
    active_clients: u32,
    active_pressure: u32,
}

/// Durable active-session event kind.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActiveSessionEventKind {
    /// Session was acquired.
    Acquired,
    /// Session was released.
    Released,
    /// Session was retired.
    Retired,
    /// Session was stale-purged.
    StalePurged,
}

impl ActiveSessionEventKind {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Acquired => "acquired",
            Self::Released => "released",
            Self::Retired => "retired",
            Self::StalePurged => "stale_purged",
        }
    }

    pub(super) fn parse(value: &str) -> Result<Self, StateStoreError> {
        match value {
            "acquired" => Ok(Self::Acquired),
            "released" => Ok(Self::Released),
            "retired" => Ok(Self::Retired),
            "stale_purged" => Ok(Self::StalePurged),
            _ => Err(StateStoreError::Sqlite {
                message: "corrupt active session event kind".to_owned(),
            }),
        }
    }
}

/// Durable active-session event.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActiveSessionEvent {
    account_id: AccountId,
    route_band: String,
    process_run_id: String,
    logical_session_id: String,
    reservation_id: ReservationId,
    event_kind: ActiveSessionEventKind,
    event_unix_seconds: u64,
    session_started_unix_seconds: u64,
    session_ended_unix_seconds: Option<u64>,
    transport_kind: String,
}

impl ActiveSessionEvent {
    /// Creates a durable active-session event.
    #[must_use]
    pub fn new(
        account_id: AccountId,
        route_band: impl Into<String>,
        process_run_id: impl Into<String>,
        reservation_id: ReservationId,
        event_kind: ActiveSessionEventKind,
        event_unix_seconds: u64,
    ) -> Self {
        let session_ended_unix_seconds = match event_kind {
            ActiveSessionEventKind::Acquired => None,
            ActiveSessionEventKind::Released
            | ActiveSessionEventKind::Retired
            | ActiveSessionEventKind::StalePurged => Some(event_unix_seconds),
        };
        Self {
            account_id,
            route_band: route_band.into(),
            process_run_id: process_run_id.into(),
            logical_session_id: reservation_id.as_str().to_owned(),
            reservation_id,
            event_kind,
            event_unix_seconds,
            session_started_unix_seconds: event_unix_seconds,
            session_ended_unix_seconds,
            transport_kind: "unknown".to_owned(),
        }
    }

    /// Sets explicit interval metadata for a durable active-session event.
    #[must_use]
    pub fn with_session_interval(
        mut self,
        session_started_unix_seconds: u64,
        session_ended_unix_seconds: Option<u64>,
        transport_kind: impl Into<String>,
    ) -> Self {
        self.session_started_unix_seconds = session_started_unix_seconds;
        self.session_ended_unix_seconds = session_ended_unix_seconds;
        self.transport_kind = transport_kind.into();
        self
    }

    /// Sets the logical session id used for interval continuity.
    #[must_use]
    pub fn with_logical_session_id(mut self, logical_session_id: impl Into<String>) -> Self {
        self.logical_session_id = logical_session_id.into();
        self
    }

    /// Returns the logical session id used for interval continuity.
    #[must_use]
    pub fn logical_session_id(&self) -> &str {
        &self.logical_session_id
    }

    /// Returns the reservation id associated with this session event.
    #[must_use]
    pub const fn reservation_id(&self) -> &ReservationId {
        &self.reservation_id
    }

    /// Returns the durable event kind.
    #[must_use]
    pub const fn event_kind(&self) -> ActiveSessionEventKind {
        self.event_kind
    }
}

/// Persisted active-session rollup.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActiveSessionRollup {
    pub(super) account_id: AccountId,
    pub(super) route_band: String,
    pub(super) bucket_start_unix_seconds: u64,
    pub(super) bucket_end_unix_seconds: u64,
    pub(super) active_session_seconds: u64,
    pub(super) max_concurrent_sessions: u32,
    pub(super) completed_sessions: u32,
    pub(super) stale_purged_sessions: u32,
}

impl ActiveSessionRollup {
    /// Creates an active-session rollup.
    #[must_use]
    pub fn new(
        account_id: AccountId,
        route_band: impl Into<String>,
        bucket_start_unix_seconds: u64,
        bucket_end_unix_seconds: u64,
        active_session_seconds: u64,
        max_concurrent_sessions: u32,
    ) -> Self {
        Self {
            account_id,
            route_band: route_band.into(),
            bucket_start_unix_seconds,
            bucket_end_unix_seconds,
            active_session_seconds,
            max_concurrent_sessions,
            completed_sessions: 0,
            stale_purged_sessions: 0,
        }
    }

    /// Sets terminal lifecycle counts for this rollup bucket.
    #[must_use]
    pub const fn with_terminal_counts(
        mut self,
        completed_sessions: u32,
        stale_purged_sessions: u32,
    ) -> Self {
        self.completed_sessions = completed_sessions;
        self.stale_purged_sessions = stale_purged_sessions;
        self
    }

    /// Returns the account id.
    #[must_use]
    pub const fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    /// Returns the bucket start timestamp.
    #[must_use]
    pub const fn bucket_start_unix_seconds(&self) -> u64 {
        self.bucket_start_unix_seconds
    }

    /// Returns the bucket end timestamp.
    #[must_use]
    pub const fn bucket_end_unix_seconds(&self) -> u64 {
        self.bucket_end_unix_seconds
    }

    /// Returns active session seconds.
    #[must_use]
    pub const fn active_session_seconds(&self) -> u64 {
        self.active_session_seconds
    }

    /// Returns completed session count in this bucket.
    #[must_use]
    pub const fn completed_sessions(&self) -> u32 {
        self.completed_sessions
    }

    /// Returns stale-purged session count in this bucket.
    #[must_use]
    pub const fn stale_purged_sessions(&self) -> u32 {
        self.stale_purged_sessions
    }
}

impl ActiveClientCount {
    /// Creates an active client count row.
    #[must_use]
    pub const fn new(account_id: AccountId, active_clients: u32, active_pressure: u32) -> Self {
        Self {
            account_id,
            active_clients,
            active_pressure,
        }
    }

    /// Returns the account id.
    #[must_use]
    pub const fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    /// Returns active clients.
    #[must_use]
    pub const fn active_clients(&self) -> u32 {
        self.active_clients
    }

    /// Returns summed active pressure units.
    #[must_use]
    pub const fn active_pressure(&self) -> u32 {
        self.active_pressure
    }
}

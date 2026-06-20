//! Background quota refresh scheduling.

use codex_router_core::ids::AccountId;
use thiserror::Error;

use crate::snapshot::QuotaSnapshot;

/// Read-side repository for existing quota snapshots.
pub trait QuotaSnapshotReader {
    /// Loads the latest snapshot for an account without refreshing it inline.
    fn load_snapshot(&self, account_id: &AccountId) -> Result<Option<QuotaSnapshot>, String>;
}

/// Scheduled refresh unit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum QuotaRefreshSchedule {
    /// Refresh one account in the background.
    Account(AccountId),
    /// Refresh all registered accounts in the background.
    AllAccounts,
}

/// Background scheduler boundary.
pub trait RefreshScheduler {
    /// Schedules refresh work without doing provider I/O inline.
    fn schedule(&self, schedule: QuotaRefreshSchedule) -> Result<(), String>;

    /// Test hook for proving startup did not refresh inline.
    fn refresh_inline_for_test(&self) {}
}

/// Startup result for an account.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QuotaRefreshStartup {
    existing_snapshot: Option<QuotaSnapshot>,
}

impl QuotaRefreshStartup {
    /// Creates a startup result.
    #[must_use]
    pub const fn new(existing_snapshot: Option<QuotaSnapshot>) -> Self {
        Self { existing_snapshot }
    }

    /// Returns the existing snapshot that request-time selection may use now.
    #[must_use]
    pub const fn existing_snapshot(&self) -> Option<&QuotaSnapshot> {
        self.existing_snapshot.as_ref()
    }
}

/// Quota refresh runtime failure.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum QuotaRefreshRuntimeError {
    /// Snapshot read failed.
    #[error("failed to load existing quota snapshot: {message}")]
    SnapshotRead {
        /// Redacted read error.
        message: String,
    },
    /// Refresh scheduling failed.
    #[error("failed to schedule quota refresh: {message}")]
    Schedule {
        /// Redacted scheduler error.
        message: String,
    },
}

/// Runtime facade used by server startup.
#[derive(Clone, Copy, Debug)]
pub struct QuotaRefreshRuntime<'a, R, S>
where
    R: QuotaSnapshotReader,
    S: RefreshScheduler,
{
    reader: &'a R,
    scheduler: &'a S,
}

impl<'a, R, S> QuotaRefreshRuntime<'a, R, S>
where
    R: QuotaSnapshotReader,
    S: RefreshScheduler,
{
    /// Creates a runtime facade.
    #[must_use]
    pub const fn new(reader: &'a R, scheduler: &'a S) -> Self {
        Self { reader, scheduler }
    }

    /// Loads the current snapshot immediately and schedules refresh work.
    pub fn start_for_account(
        &self,
        account_id: &AccountId,
    ) -> Result<QuotaRefreshStartup, QuotaRefreshRuntimeError> {
        let snapshot = self
            .reader
            .load_snapshot(account_id)
            .map_err(|message| QuotaRefreshRuntimeError::SnapshotRead { message })?;
        self.scheduler
            .schedule(QuotaRefreshSchedule::Account(account_id.clone()))
            .map_err(|message| QuotaRefreshRuntimeError::Schedule { message })?;

        Ok(QuotaRefreshStartup::new(snapshot))
    }
}

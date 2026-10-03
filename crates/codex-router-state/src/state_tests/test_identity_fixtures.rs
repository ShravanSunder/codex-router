use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use crate::quota_snapshot::PersistedQuotaHistoryObservation;
use crate::quota_snapshot::QuotaHistoryRefreshOutcome;
use crate::quota_snapshot::QuotaSnapshotSource;
use crate::quota_snapshot::SelectorQuotaWindowStatus;
use codex_router_core::affinity::AffinityKeyHash;
use codex_router_core::ids::AccountId;
use rusqlite::Connection;

pub(super) fn expect_error<T, E>(result: Result<T, E>, context: &'static str) -> E {
    match result {
        Ok(_) => panic!("{context}"),
        Err(error) => error,
    }
}

static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

pub(super) struct TestTempDir {
    path: PathBuf,
}

impl TestTempDir {
    pub(super) fn new(name: &str) -> Self {
        let unique = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = env::temp_dir().join(format!(
            "codex-router-state-{name}-{}-{unique}",
            std::process::id()
        ));
        if let Err(error) = fs::create_dir(&path) {
            panic!(
                "failed to create test directory {}: {error}",
                path.display()
            );
        }

        Self { path }
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestTempDir {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.path) {
            panic!(
                "failed to remove test directory {}: {error}",
                self.path.display()
            );
        }
    }
}

pub(super) fn account_id(value: &str) -> AccountId {
    match AccountId::new(value) {
        Ok(account_id) => account_id,
        Err(error) => panic!("account id should parse: {error}"),
    }
}

pub(super) fn quota_history_observation(
    account_id: AccountId,
    route_band: &str,
    limit_window_seconds: u64,
    observed_unix_seconds: u64,
    remaining_headroom: u32,
    reset_unix_seconds: Option<u64>,
) -> PersistedQuotaHistoryObservation {
    let mut observation = PersistedQuotaHistoryObservation::new(
        account_id,
        "safe-label",
        route_band,
        limit_window_seconds,
        observed_unix_seconds,
        remaining_headroom,
    )
    .with_window_status(SelectorQuotaWindowStatus::Eligible)
    .with_refresh_source(QuotaSnapshotSource::OpenAiEndpoint)
    .with_refresh_outcome(QuotaHistoryRefreshOutcome::Success);
    if let Some(reset_unix_seconds) = reset_unix_seconds {
        observation = observation.with_reset_unix_seconds(reset_unix_seconds);
    }

    observation
}

pub(super) fn affinity_hash(character: char) -> AffinityKeyHash {
    match AffinityKeyHash::new(character.to_string().repeat(64)) {
        Ok(hash) => hash,
        Err(error) => panic!("affinity hash should parse: {error}"),
    }
}

pub(super) fn assert_no_previous_response_id_in_affinity_owner_rows(
    database_path: &Path,
    raw_previous_response_id: &str,
) {
    let connection = match Connection::open(database_path) {
        Ok(connection) => connection,
        Err(error) => panic!("raw sqlite should open: {error}"),
    };
    let count: i64 = match connection.query_row(
        "SELECT COUNT(*)
               FROM previous_response_affinity_owners
              WHERE affinity_key_hash = ?1
                 OR route_band = ?1
                 OR account_id = ?1
                 OR source_transport = ?1",
        [raw_previous_response_id],
        |row| row.get(0),
    ) {
        Ok(count) => count,
        Err(error) => panic!("raw sqlite count should query: {error}"),
    };
    assert_eq!(count, 0);
}

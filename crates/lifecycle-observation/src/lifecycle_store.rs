//! Single storage owner with commit-triggered journal reads and bounded maintenance.
use crate::{JournalError, JournalPage, JournalPosition, JournalRow, ObservationJournal};
use collaboration_protocol::{EndpointRef, LifecycleObservation};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
#[cfg(test)]
use tokio::sync::mpsc;
use tokio::sync::{Mutex, watch};
use tokio_util::sync::CancellationToken;

pub struct LifecycleStore {
    journal: Mutex<ObservationJournal>,
    commits: watch::Sender<u64>,
    coverage: std::sync::Mutex<crate::ObservationCoverage>,
    snapshots: Mutex<crate::AddressSnapshotCache>,
    available: AtomicBool,
}

enum MaintenanceTickSource {
    Interval(tokio::time::Interval),
    #[cfg(test)]
    Test(mpsc::Receiver<()>),
}

impl MaintenanceTickSource {
    async fn wait_for_tick(&mut self) -> bool {
        match self {
            Self::Interval(interval) => {
                interval.tick().await;
                true
            }
            #[cfg(test)]
            Self::Test(receiver) => receiver.recv().await.is_some(),
        }
    }
}

impl LifecycleStore {
    #[must_use]
    pub fn new(journal: ObservationJournal) -> Self {
        let (commits, _) = watch::channel(0);
        Self {
            journal: Mutex::new(journal),
            commits,
            coverage: std::sync::Mutex::new(crate::ObservationCoverage::default()),
            snapshots: Mutex::new(crate::AddressSnapshotCache::default()),
            available: AtomicBool::new(true),
        }
    }
    pub async fn bounds(&self) -> Result<crate::JournalBounds, JournalError> {
        let mut journal = self.journal.lock().await;
        self.require_available()?;
        let result = journal.bounds().await;
        self.observe_storage_result(&result)?;
        result
    }
    pub async fn append(
        &self,
        observation: &LifecycleObservation,
        now: i64,
    ) -> Result<JournalRow, JournalError> {
        observation
            .validate()
            .map_err(|_| JournalError::InvalidRecord)?;
        if now < 0 {
            return Err(JournalError::InvalidRecord);
        }
        let mut journal = self.journal.lock().await;
        self.require_available()?;
        let appended = match journal.append(observation, now).await {
            Err(JournalError::Capacity) => {
                // A failed append rolls back before maintenance. Reclaim only the
                // expired prefix, then make one fresh metadata admission attempt.
                match journal.expire_history(now).await {
                    Ok(_) => journal.append(observation, now).await,
                    Err(error) => Err(error),
                }
            }
            result => result,
        };
        let row = match appended {
            Ok(row) => row,
            Err(error) => {
                self.invalidate_storage()?;
                return Err(error);
            }
        };
        self.coverage
            .lock()
            .map_err(|_| JournalError::InvalidStorage)?
            .apply(observation)?;
        self.signal_commit();
        Ok(row)
    }
    fn signal_commit(&self) {
        self.commits
            .send_modify(|revision| *revision = revision.wrapping_add(1));
    }
    fn require_available(&self) -> Result<(), JournalError> {
        if self.available.load(Ordering::Acquire) {
            Ok(())
        } else {
            Err(JournalError::InvalidStorage)
        }
    }
    fn invalidate_storage(&self) -> Result<(), JournalError> {
        self.available.store(false, Ordering::Release);
        self.coverage
            .lock()
            .map_err(|_| JournalError::InvalidStorage)?
            .storage_unavailable();
        self.signal_commit();
        Ok(())
    }
    fn observe_storage_result<TRecord>(
        &self,
        result: &Result<TRecord, JournalError>,
    ) -> Result<(), JournalError> {
        if matches!(
            result,
            Err(JournalError::Storage(_) | JournalError::InvalidStorage | JournalError::Capacity)
        ) {
            self.invalidate_storage()?;
        }
        Ok(())
    }
    async fn read_page(
        &self,
        endpoint: &EndpointRef,
        after: JournalPosition,
        page_size: u32,
    ) -> Result<JournalPage, JournalError> {
        let mut journal = self.journal.lock().await;
        self.require_available()?;
        let result = journal.read_page(endpoint, after, page_size).await;
        self.observe_storage_result(&result)?;
        result
    }
    pub async fn read_wait(
        &self,
        endpoint: &EndpointRef,
        after: JournalPosition,
        page_size: u32,
        wait_milliseconds: u64,
    ) -> Result<JournalPage, JournalError> {
        if wait_milliseconds > 30000 {
            return Err(JournalError::InvalidRecord);
        }
        let deadline = tokio::time::Instant::now() + Duration::from_millis(wait_milliseconds);
        // Subscribe before the first read to avoid missing a commit between read and wait.
        let mut wake = self.commits.subscribe();
        let mut position = after;
        loop {
            let page = self
                .read_page(endpoint, position.clone(), page_size)
                .await?;
            if !page.records.is_empty()
                || !page.caught_up
                || wait_milliseconds == 0
                || tokio::time::Instant::now() >= deadline
            {
                return Ok(page);
            }
            position = page.next.clone();
            if tokio::time::timeout_at(deadline, wake.changed())
                .await
                .is_err()
            {
                return self.read_page(endpoint, position, page_size).await;
            }
        }
    }
    /// Completes startup maintenance before the owner admits public journal reads.
    pub async fn prepare(&self, now: i64) -> Result<(), JournalError> {
        let mut journal = self.journal.lock().await;
        self.invalidate_storage()?;
        journal.rebuild_addresses().await?;
        journal.expire_history(now).await?;
        *self
            .coverage
            .lock()
            .map_err(|_| JournalError::InvalidStorage)? = crate::ObservationCoverage::default();
        *self.snapshots.lock().await = crate::AddressSnapshotCache::default();
        self.available.store(true, Ordering::Release);
        self.signal_commit();
        Ok(())
    }
    pub async fn maintain(&self, now: i64) -> Result<(), JournalError> {
        let mut journal = self.journal.lock().await;
        let result = journal.expire_history(now).await;
        self.observe_storage_result(&result)?;
        result?;
        self.signal_commit();
        Ok(())
    }
    /// Internal retention maintenance only; this does not schedule agent work.
    pub async fn run_maintenance(&self, shutdown: CancellationToken) -> Result<(), JournalError> {
        let mut interval = tokio::time::interval(Duration::from_secs(3600));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        self.run_maintenance_with_ticks(shutdown, MaintenanceTickSource::Interval(interval))
            .await
    }

    async fn run_maintenance_with_ticks(
        &self,
        shutdown: CancellationToken,
        mut ticks: MaintenanceTickSource,
    ) -> Result<(), JournalError> {
        loop {
            tokio::select! {
                _=shutdown.cancelled()=>return Ok(()),
                tick_available = ticks.wait_for_tick()=>{
                    if !tick_available {
                        return Ok(());
                    }
                    let now=SystemTime::now().duration_since(UNIX_EPOCH).map_err(|_|JournalError::InvalidRecord)?.as_secs();
                    if let Err(error) = self
                        .maintain(i64::try_from(now).map_err(|_| JournalError::InvalidRecord)?)
                        .await
                    {
                        tracing::warn!(
                            component = "lifecycle_store",
                            maintenance_class = "lifecycle_journal_retention",
                            error_class = lifecycle_maintenance_error_class(&error),
                            "lifecycle_observation.maintenance_degraded"
                        );
                    }
                }
            }
        }
    }
    pub async fn close(self) {
        self.journal.into_inner().close().await;
    }
}

const fn lifecycle_maintenance_error_class(error: &JournalError) -> &'static str {
    match error {
        JournalError::SnapshotExpired => "snapshot_expired",
        JournalError::JournalChanged => "journal_changed",
        JournalError::HistoryExpired => "history_expired",
        JournalError::Storage(_) => "storage",
        JournalError::InvalidRecord => "invalid_record",
        JournalError::InvalidStorage => "invalid_storage",
        JournalError::Capacity => "capacity",
    }
}

impl LifecycleStore {
    pub async fn address_snapshot(
        &self,
        endpoint: &EndpointRef,
        page_size: usize,
        cursor: Option<&str>,
        snapshot_id: collaboration_protocol::UuidIdentity,
        captured_at: collaboration_protocol::ObservationTimestamp,
    ) -> Result<crate::AddressPage, JournalError> {
        self.require_available()?;
        if let Some(cursor) = cursor {
            return self.snapshots.lock().await.page(
                endpoint,
                page_size,
                cursor,
                std::time::Instant::now(),
            );
        }
        let mut journal = self.journal.lock().await;
        self.require_available()?;
        let captured = journal.capture_addresses(endpoint).await;
        self.observe_storage_result(&captured)?;
        let captured = captured?;
        let coverage = self
            .coverage
            .lock()
            .map_err(|_| JournalError::InvalidStorage)?
            .view(endpoint, captured_at.clone());
        self.snapshots
            .lock()
            .await
            .insert(crate::AddressSnapshotInputs {
                id: snapshot_id,
                endpoint: endpoint.clone(),
                page_size,
                captured,
                now: std::time::Instant::now(),
                coverage,
                captured_at,
            })
    }
}

#[cfg(test)]
mod tests {
    use collaboration_protocol::LifecycleObservation;
    use sqlx::Connection;
    use sqlx::SqliteConnection;
    use sqlx::sqlite::SqliteConnectOptions;
    use std::env;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    use super::LifecycleStore;
    use super::MaintenanceTickSource;
    use crate::JournalError;
    use crate::ObservationJournal;

    static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    #[tokio::test]
    async fn failed_hourly_maintenance_continues_to_next_tick_without_restoring_availability() {
        let path = test_database_path("hourly_maintenance_failure_continues");
        let journal = ObservationJournal::open(&path, identity())
            .await
            .unwrap_or_else(|error| panic!("journal should open: {error}"));
        let store = Arc::new(LifecycleStore::new(journal));
        store
            .append(&observation(), 1)
            .await
            .unwrap_or_else(|error| panic!("fixture row should append: {error}"));
        let mut fixture =
            SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(&path))
                .await
                .unwrap_or_else(|error| panic!("fixture connection should open: {error}"));
        sqlx::query("DROP TABLE lifecycle_records")
            .execute(&mut fixture)
            .await
            .unwrap_or_else(|error| {
                panic!("fixture should inject one maintenance failure: {error}")
            });

        let shutdown = CancellationToken::new();
        let (tick_sender, tick_receiver) = mpsc::channel(2);
        let task = tokio::spawn({
            let store = Arc::clone(&store);
            let shutdown = shutdown.clone();
            async move {
                store
                    .run_maintenance_with_ticks(
                        shutdown,
                        MaintenanceTickSource::Test(tick_receiver),
                    )
                    .await
            }
        });
        tick_sender
            .send(())
            .await
            .unwrap_or_else(|error| panic!("first maintenance tick should send: {error}"));
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while store.available.load(Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap_or_else(|_elapsed| {
            panic!("first maintenance failure should invalidate journal availability")
        });
        assert!(
            !store.available.load(Ordering::Acquire),
            "failed first hourly maintenance tick should leave the journal unavailable"
        );

        sqlx::query(
            "CREATE TABLE lifecycle_records (sequence INTEGER PRIMARY KEY, retention_at INTEGER NOT NULL, observation_json TEXT NOT NULL)",
        )
        .execute(&mut fixture)
        .await
        .unwrap_or_else(|error| panic!("fixture should restore the one failed storage seam: {error}"));
        let restored_observation = serde_json::to_string(&observation()).unwrap_or_else(|error| {
            panic!("expired fixture observation should serialize: {error}")
        });
        sqlx::query(
            "INSERT INTO lifecycle_records (sequence, retention_at, observation_json) VALUES (1, 1, ?)",
        )
        .bind(restored_observation)
        .execute(&mut fixture)
        .await
        .unwrap_or_else(|error| panic!("fixture should restore one expired record: {error}"));
        let before_second_tick: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM lifecycle_records")
            .fetch_one(&mut fixture)
            .await
            .unwrap_or_else(|error| {
                panic!("fixture should contain the expired record before tick two: {error}")
            });
        assert_eq!(before_second_tick, 1);
        tick_sender
            .send(())
            .await
            .unwrap_or_else(|error| panic!("later maintenance tick should send: {error}"));

        let retained = tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                let retained: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM lifecycle_records")
                    .fetch_one(&mut fixture)
                    .await
                    .unwrap_or_else(|error| {
                        panic!("later maintenance tick should query fixture rows: {error}")
                    });
                if retained == 0 {
                    return retained;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap_or_else(|_elapsed| panic!("later maintenance tick should clean expired rows"));
        assert_eq!(
            retained, 0,
            "later existing hourly tick should clean expired rows"
        );
        assert!(matches!(
            store.bounds().await,
            Err(JournalError::InvalidStorage)
        ));

        shutdown.cancel();
        assert!(
            task.await
                .unwrap_or_else(|error| panic!("maintenance task should join: {error}"))
                .is_ok(),
            "shutdown should end the still-running maintenance task successfully"
        );
        fixture
            .close()
            .await
            .unwrap_or_else(|error| panic!("fixture should close: {error}"));
        Arc::try_unwrap(store)
            .unwrap_or_else(|_| panic!("maintenance task should release its store reference"))
            .close()
            .await;
        std::fs::remove_file(path)
            .unwrap_or_else(|error| panic!("fixture database should be removed: {error}"));
    }

    #[test]
    fn lifecycle_maintenance_error_class_excludes_raw_storage_details() {
        let error = JournalError::Storage(sqlx::Error::Protocol(
            "raw-lifecycle-storage-error-canary".to_owned(),
        ));

        let error_class = super::lifecycle_maintenance_error_class(&error);

        assert_eq!(error_class, "storage");
        assert!(!error_class.contains("raw-lifecycle-storage-error-canary"));
    }

    fn identity() -> collaboration_protocol::UuidIdentity {
        "00000000-0000-4000-8000-000000000001"
            .to_owned()
            .try_into()
            .unwrap_or_else(|error| panic!("test identity should parse: {error}"))
    }

    fn observation() -> LifecycleObservation {
        serde_json::from_value(serde_json::json!({
            "observedAt": "2026-09-05T12:00:00Z",
            "source": "observerLifecycle",
            "scope": {
                "endpoint": {
                    "serviceId": "00000000-0000-4000-8000-000000000001",
                    "endpointId": "codex-local"
                },
                "generation": null,
                "observerId": "00000000-0000-4000-8000-000000000002"
            },
            "subject": {"kind": "backend"},
            "change": {"kind": "coverageLost"}
        }))
        .unwrap_or_else(|error| panic!("test observation should parse: {error}"))
    }

    fn test_database_path(name: &str) -> std::path::PathBuf {
        let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        env::temp_dir().join(format!(
            "lifecycle-observation-{name}-{}-{counter}.sqlite",
            std::process::id()
        ))
    }
}

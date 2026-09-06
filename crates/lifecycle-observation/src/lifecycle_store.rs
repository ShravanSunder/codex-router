//! Single storage owner with commit-triggered journal reads and bounded maintenance.
use crate::{JournalError, JournalPage, JournalPosition, JournalRow, ObservationJournal};
use communication_protocol::{EndpointRef, LifecycleObservation};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{Mutex, watch};
use tokio_util::sync::CancellationToken;

pub struct LifecycleStore {
    journal: Mutex<ObservationJournal>,
    commits: watch::Sender<u64>,
    coverage: std::sync::Mutex<crate::ObservationCoverage>,
    snapshots: Mutex<crate::AddressSnapshotCache>,
    available: AtomicBool,
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
        loop {
            tokio::select! {
                _=shutdown.cancelled()=>return Ok(()),
                _=interval.tick()=>{
                    let now=SystemTime::now().duration_since(UNIX_EPOCH).map_err(|_|JournalError::InvalidRecord)?.as_secs();
                    self.maintain(i64::try_from(now).map_err(|_|JournalError::InvalidRecord)?).await?;
                }
            }
        }
    }
    pub async fn close(self) {
        self.journal.into_inner().close().await;
    }
}

impl LifecycleStore {
    pub async fn address_snapshot(
        &self,
        endpoint: &EndpointRef,
        page_size: usize,
        cursor: Option<&str>,
        snapshot_id: communication_protocol::UuidIdentity,
        captured_at: communication_protocol::ObservationTimestamp,
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

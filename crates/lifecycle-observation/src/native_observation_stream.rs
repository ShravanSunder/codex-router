//! Generation-scoped read-only native observation and lifecycle persistence.
use crate::{
    InventoryReconciliation, JournalError, LifecycleStore, ReadDisposition, map_native_lifecycle,
};
use codex_native_integration::{NativeOperation, NativePayloadSchemas, NativeProtocolConnection};
use communication_protocol::{
    LifecycleChange, LifecycleObservation, LifecycleSubject, ObservationOrdering, ObservationScope,
    ObservationSource, SessionId, ThreadAddress,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};
use tokio_util::sync::CancellationToken;

pub struct NativeObservationStream {
    schemas: Arc<NativePayloadSchemas>,
    connection: NativeProtocolConnection,
    store: Arc<LifecycleStore>,
    scope: ObservationScope,
    reconciliation: InventoryReconciliation,
    pending_reads: BTreeSet<SessionId>,
    last_status_observations: BTreeMap<
        SessionId,
        (
            communication_protocol::NativeThreadStatus,
            ObservationOrdering,
        ),
    >,
}
pub struct NativeObservationInputs {
    pub connection: NativeProtocolConnection,
    pub store: Arc<LifecycleStore>,
    pub scope: ObservationScope,
    pub schemas: Arc<NativePayloadSchemas>,
}
impl NativeObservationStream {
    pub fn new(inputs: NativeObservationInputs) -> Result<Self, JournalError> {
        let NativeObservationInputs {
            connection,
            store,
            scope,
            schemas,
        } = inputs;
        let reconciliation = InventoryReconciliation::new(scope.clone())?;
        Ok(Self {
            schemas,
            connection,
            store,
            scope,
            reconciliation,
            pending_reads: BTreeSet::new(),
            last_status_observations: BTreeMap::new(),
        })
    }
    /// Caller admits the native schema and supplies the generation retirement signal.
    pub async fn run(mut self, retired: CancellationToken) -> Result<(), JournalError> {
        let result = tokio::select! {
            _=retired.cancelled()=>Ok(()),
            result=self.observe()=>result,
        };
        self.reconciliation.disconnect();
        let loss = self
            .record(
                LifecycleSubject::Backend,
                LifecycleChange::CoverageLost,
                ObservationSource::ObserverLifecycle,
            )
            .await;
        result.and(loss)
    }
    async fn observe(&mut self) -> Result<(), JournalError> {
        let mut cursor = None::<String>;
        let mut cursors = BTreeSet::new();
        let mut threads = BTreeSet::new();
        loop {
            let result = self
                .connection
                .request_validated(
                    &self.schemas,
                    NativeOperation::ListLoadedThreads,
                    json!({"limit":100,"cursor":cursor}),
                )
                .await
                .map_err(|_| JournalError::InvalidStorage)?;
            let page: LoadedPage =
                serde_json::from_value(result).map_err(|_| JournalError::InvalidRecord)?;
            if page.data.len() > 100 {
                return Err(JournalError::Capacity);
            }
            for id in page.data {
                threads.insert(id);
                if threads.len() > 16384 {
                    return Err(JournalError::Capacity);
                }
            }
            self.drain_events().await?;
            cursor = page.next_cursor;
            let Some(next) = &cursor else {
                break;
            };
            if !cursors.insert(next.clone()) || cursors.len() > 1024 {
                return Err(JournalError::Capacity);
            }
        }
        for thread in threads {
            self.read_thread(thread).await?;
        }
        self.reconcile_pending().await?;
        self.record(
            LifecycleSubject::Backend,
            LifecycleChange::CoverageRestored,
            ObservationSource::ObserverLifecycle,
        )
        .await?;
        loop {
            let message = self
                .connection
                .next_message()
                .await
                .map_err(|_| JournalError::InvalidStorage)?;
            if let Some(thread) = self.ingest_event(message).await? {
                self.pending_reads.insert(thread);
                self.reconcile_pending().await?;
            }
        }
    }
    async fn read_thread(&mut self, thread: SessionId) -> Result<(), JournalError> {
        for _ in 0..3 {
            let ticket = self.reconciliation.begin_read(thread.clone())?;
            let result = self
                .connection
                .request_validated(
                    &self.schemas,
                    NativeOperation::ReadThread,
                    json!({"threadId":String::from(thread.clone()),"includeTurns":false}),
                )
                .await;
            self.drain_events().await?;
            let disposition = self.reconciliation.finish_read(ticket);
            let response = result.map_err(|_| JournalError::InvalidStorage)?;
            let value = response.get("thread").ok_or(JournalError::InvalidRecord)?;
            if value.get("id").and_then(Value::as_str)
                != Some(String::from(thread.clone()).as_str())
            {
                return Err(JournalError::InvalidRecord);
            }
            let status = serde_json::from_value(
                value
                    .get("status")
                    .cloned()
                    .ok_or(JournalError::InvalidRecord)?,
            )
            .map_err(|_| JournalError::InvalidRecord)?;
            let (ordering, retry) = match disposition {
                ReadDisposition::Established => (ObservationOrdering::Established, false),
                ReadDisposition::Ambiguous { retry } => (ObservationOrdering::Ambiguous, retry),
                ReadDisposition::Discard => return Ok(()),
            };
            let subject = LifecycleSubject::Thread {
                address: ThreadAddress {
                    endpoint: self.scope.endpoint.clone(),
                    native_thread_id: thread.clone(),
                },
            };
            self.record(
                subject.clone(),
                LifecycleChange::ThreadDiscovered,
                ObservationSource::InventoryRead,
            )
            .await?;
            self.record(
                subject,
                LifecycleChange::ThreadStatus { status, ordering },
                ObservationSource::InventoryRead,
            )
            .await?;
            if !retry {
                break;
            }
        }
        // Conflicts on this thread were handled by the bounded loop above. Retain other
        // threads queued by interleaved events; they have not yet had their scoped reread.
        self.pending_reads.remove(&thread);
        Ok(())
    }
    async fn reconcile_pending(&mut self) -> Result<(), JournalError> {
        // Bound cross-thread churn as well as the three same-thread conflicts. A busy
        // nonconverging stream loses coverage rather than publishing false readiness.
        for _ in 0..(16384 * 3) {
            let Some(thread) = self.pending_reads.pop_first() else {
                return Ok(());
            };
            self.read_thread(thread).await?;
        }
        if self.pending_reads.is_empty() {
            Ok(())
        } else {
            Err(JournalError::Capacity)
        }
    }
    async fn drain_events(&mut self) -> Result<(), JournalError> {
        while let Some(message) = self.connection.take_buffered_message() {
            if let Some(thread) = self.ingest_event(message).await? {
                self.pending_reads.insert(thread);
            }
        }
        Ok(())
    }
    async fn ingest_event(&mut self, message: Value) -> Result<Option<SessionId>, JournalError> {
        let at = timestamp()?;
        let mut records =
            map_native_lifecycle(&message, &self.scope, at, ObservationOrdering::Ambiguous)?;
        let Some(LifecycleSubject::Thread { address }) =
            records.first().map(|record| &record.subject)
        else {
            return Ok(None);
        };
        let thread = address.native_thread_id.clone();
        let ordering = self.reconciliation.notification(&thread)?;
        for record in &mut records {
            if let LifecycleChange::ThreadStatus {
                ordering: current, ..
            } = &mut record.change
            {
                *current = ordering;
            }
            self.append_observation(record).await?;
        }
        Ok((ordering == ObservationOrdering::Ambiguous).then_some(thread))
    }
    async fn record(
        &mut self,
        subject: LifecycleSubject,
        change: LifecycleChange,
        source: ObservationSource,
    ) -> Result<(), JournalError> {
        self.append_observation(&LifecycleObservation {
            observed_at: timestamp()?,
            source,
            scope: self.scope.clone(),
            subject,
            change,
        })
        .await?;
        Ok(())
    }
    async fn append_observation(
        &mut self,
        record: &LifecycleObservation,
    ) -> Result<(), JournalError> {
        let thread = match &record.subject {
            LifecycleSubject::Thread { address } => Some(&address.native_thread_id),
            LifecycleSubject::Backend => None,
        };
        if let (Some(thread), LifecycleChange::ThreadStatus { status, ordering }) =
            (thread, &record.change)
            && self.last_status_observations.get(thread).is_some_and(
                |(previous, previous_ordering)| previous == status && previous_ordering == ordering,
            )
        {
            // The fact is already durable in this stream's scope. Still notice
            // storage failure rather than letting the cache conceal lost coverage.
            self.store.bounds().await?;
            return Ok(());
        }
        self.store
            .append(record, chrono::Utc::now().timestamp())
            .await?;
        if let Some(thread) = thread {
            match &record.change {
                LifecycleChange::ThreadStatus { status, ordering } => {
                    // The reconciliation owner already bounds thread membership.
                    self.last_status_observations
                        .insert(thread.clone(), (status.clone(), *ordering));
                }
                LifecycleChange::ThreadClosed | LifecycleChange::ThreadDeleted => {
                    self.last_status_observations.remove(thread);
                }
                _ => {}
            }
        }
        Ok(())
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LoadedPage {
    data: Vec<SessionId>,
    next_cursor: Option<String>,
}
fn timestamp() -> Result<communication_protocol::ObservationTimestamp, JournalError> {
    chrono::Utc::now()
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
        .try_into()
        .map_err(|_| JournalError::InvalidRecord)
}

//! Host-owned timer task turns due wake-ups into durable obligations, independently of native I/O.
use automation_storage::{AutomationStore, StorageError};
use communication_protocol::SavedMessage;
use std::{sync::Arc, time::Duration};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

pub struct WakeTimingWorker {
    store: Arc<Mutex<AutomationStore>>,
    native: crate::wakeup_native_sender::WakeNativeSender,
}
impl WakeTimingWorker {
    pub(crate) fn new(
        store: Arc<Mutex<AutomationStore>>,
        native: crate::wakeup_native_sender::WakeNativeSender,
    ) -> Self {
        Self { store, native }
    }
    /// Start only after the Host owns its listeners and previous workers have retired.
    pub async fn run(self, shutdown: CancellationToken) {
        let mut interval = tokio::time::interval(Duration::from_millis(250));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut dispatches = tokio::task::JoinSet::<Result<(), StorageError>>::new();
        let mut recovered = false;
        let mut reported_failure = false;
        loop {
            tokio::select! {
                biased;
                _=shutdown.cancelled()=>{
                    dispatches.abort_all();
                    while dispatches.join_next().await.is_some() {}
                    return;
                },
                completed=dispatches.join_next(), if !dispatches.is_empty()=>{
                    if !matches!(completed,Some(Ok(Ok(())))) {tracing::warn!("automation delivery task stopped; persisted attempt must be inspected before replay");}
                    continue;
                }
                _=interval.tick()=>{}
            }
            let now = chrono::Utc::now().timestamp_millis();
            let result = if recovered {
                match self.evaluate_due(now).await {
                    Ok(()) => self.dispatch_pending(&mut dispatches, now).await,
                    Err(error) => Err(error),
                }
            } else {
                match self
                    .store
                    .lock()
                    .await
                    .recover_interrupted_deliveries(now)
                    .await
                {
                    Ok(_) => {
                        recovered = true;
                        Ok(())
                    }
                    Err(error) => Err(error),
                }
            };
            match result {
                Ok(()) => reported_failure = false,
                Err(_) => {
                    if !reported_failure {
                        tracing::warn!(
                            "automation timing storage unavailable; no new delivery is dispatched by this pass"
                        );
                    }
                    reported_failure = true;
                }
            }
        }
    }
    async fn dispatch_pending(
        &self,
        tasks: &mut tokio::task::JoinSet<Result<(), StorageError>>,
        now_ms: i64,
    ) -> Result<(), StorageError> {
        let slots = 16_usize.saturating_sub(tasks.len());
        if slots == 0 {
            return Ok(());
        }
        let ids = self
            .store
            .lock()
            .await
            .eligible_delivery_ids(
                now_ms,
                u32::try_from(slots).map_err(|_| StorageError::InvalidRecord)?,
            )
            .await?;
        for id in ids {
            let store = Arc::clone(&self.store);
            let native = self.native.clone();
            tasks.spawn(async move { native.dispatch(store, id).await });
        }
        Ok(())
    }
    async fn evaluate_due(&self, now_ms: i64) -> Result<(), StorageError> {
        let ids = self.store.lock().await.due_wakeup_ids(now_ms, 100).await?;
        for id in ids {
            self.store
                .lock()
                .await
                .evaluate_wakeup::<SavedMessage>(&id, now_ms)
                .await?;
        }
        Ok(())
    }
}

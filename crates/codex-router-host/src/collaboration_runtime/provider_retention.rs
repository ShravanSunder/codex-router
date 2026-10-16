//! Host-owned provider operation retention and its shutdown-bound maintenance task.
use super::*;

impl CollaborationRuntime {
    pub async fn configure_provider_operation_retention(
        &mut self,
        retention_days: std::num::NonZeroU32,
    ) -> io::Result<u64> {
        let cutoff_ms = unix_seconds()?
            .saturating_mul(1_000)
            .saturating_sub(i64::from(retention_days.get()).saturating_mul(24 * 60 * 60 * 1_000));
        let protected = self
            .external_provider_supervisor
            .as_ref()
            .map(|supervisor| supervisor.live_operation_ids())
            .unwrap_or_default();
        let Some(store) = &self.provider_store else {
            return Ok(0);
        };
        let pruned = store
            .lock()
            .await
            .prune_terminal_before(cutoff_ms, &protected)
            .await
            .map_err(io::Error::other)?;
        let store = std::sync::Arc::clone(store);
        let supervisor = self.external_provider_supervisor.clone();
        let shutdown = self.shutdown.clone();
        self.provider_retention = Some(tokio::spawn(async move {
            let period = std::time::Duration::from_secs(24 * 60 * 60);
            let mut interval =
                tokio::time::interval_at(tokio::time::Instant::now() + period, period);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    () = shutdown.cancelled() => break,
                    _ = interval.tick() => {
                        let cutoff_ms = chrono::Utc::now().timestamp_millis().saturating_sub(
                            i64::from(retention_days.get()).saturating_mul(24 * 60 * 60 * 1_000),
                        );
                        let protected = supervisor.as_ref().map(|value| value.live_operation_ids()).unwrap_or_default();
                        let _ = store.lock().await.prune_terminal_before(cutoff_ms, &protected).await;
                    }
                }
            }
        }));
        Ok(pruned)
    }
}

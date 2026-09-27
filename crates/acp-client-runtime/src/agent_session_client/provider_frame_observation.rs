//! Bounded ACP frame-limit observation shared by connection and Session actors.

use super::*;

#[derive(Debug, Default)]
pub(crate) struct ProviderFrameObservation {
    limit_exceeded: AtomicBool,
    limit_notification: tokio::sync::Notify,
}

impl ProviderFrameObservation {
    pub(super) fn record_limit_exceeded(&self) {
        self.limit_exceeded.store(true, Ordering::Relaxed);
        self.limit_notification.notify_one();
    }

    pub(crate) fn limit_was_exceeded(&self) -> bool {
        self.limit_exceeded.load(Ordering::Relaxed)
    }

    pub(crate) async fn wait_for_limit_exceeded(&self) -> bool {
        tokio::time::timeout(
            Duration::from_millis(100),
            self.limit_notification.notified(),
        )
        .await
        .is_ok()
    }
}

use chrono::{DateTime, Utc};
use tokio::time::Instant;

/// Wall time carries durable deadlines; monotonic time drives the live owner.
pub trait SubscriptionClock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
    fn monotonic_now(&self) -> Instant;
    fn sleep_until(
        &self,
        deadline: Instant,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        Box::pin(tokio::time::sleep_until(deadline))
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemSubscriptionClock;

impl SubscriptionClock for SystemSubscriptionClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
    fn monotonic_now(&self) -> Instant {
        Instant::now()
    }
}

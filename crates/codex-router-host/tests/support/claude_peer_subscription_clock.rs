//! Deterministic owner deadlines through the existing subscription clock seam.
use super::{OWNER_EVENT_TIMEOUT, PRESENCE_RECHECK, ProofResult};
use chrono::{DateTime, Utc};
use collaboration_service::SubscriptionClock;
use std::{sync::Mutex, time::Duration};
use tokio::{
    sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel},
    time::Instant,
};

pub(super) struct ManualSubscriptionClock {
    wall: Mutex<DateTime<Utc>>,
    monotonic: Mutex<Instant>,
    advanced: tokio::sync::Notify,
    sleep_deadlines: UnboundedSender<Instant>,
}

impl ManualSubscriptionClock {
    pub(super) fn new(now: DateTime<Utc>) -> (Self, UnboundedReceiver<Instant>) {
        let (sleep_deadlines, receiver) = unbounded_channel();
        (
            Self {
                wall: Mutex::new(now),
                monotonic: Mutex::new(Instant::now()),
                advanced: tokio::sync::Notify::new(),
                sleep_deadlines,
            },
            receiver,
        )
    }

    pub(super) fn advance(&self, amount: Duration) -> ProofResult {
        let wall_amount = chrono::Duration::from_std(amount)?;
        let mut wall = self
            .wall
            .lock()
            .map_err(|_| std::io::Error::other("manual wall clock lock poisoned"))?;
        *wall = wall
            .checked_add_signed(wall_amount)
            .ok_or_else(|| std::io::Error::other("manual wall clock overflow"))?;
        let mut monotonic = self
            .monotonic
            .lock()
            .map_err(|_| std::io::Error::other("manual monotonic clock lock poisoned"))?;
        *monotonic = monotonic
            .checked_add(amount)
            .ok_or_else(|| std::io::Error::other("manual monotonic clock overflow"))?;
        drop(monotonic);
        drop(wall);
        self.advanced.notify_waiters();
        Ok(())
    }

    pub(super) fn current_monotonic(&self) -> ProofResult<Instant> {
        self.monotonic
            .lock()
            .map(|instant| *instant)
            .map_err(|_| std::io::Error::other("manual monotonic clock lock poisoned").into())
    }

    pub(super) async fn next_sleep_deadline_matching(
        clock: &Self,
        receiver: &mut UnboundedReceiver<Instant>,
        stage: &'static str,
        exact_remaining: Option<Duration>,
    ) -> ProofResult<Instant> {
        tokio::time::timeout(OWNER_EVENT_TIMEOUT, async {
            loop {
                let deadline = receiver.recv().await.ok_or_else(|| {
                    std::io::Error::other("owner sleep observation channel closed")
                })?;
                let remaining = deadline.checked_duration_since(clock.current_monotonic()?);
                let matches_stage = match exact_remaining {
                    Some(expected) => remaining == Some(expected),
                    None => remaining.is_some_and(|value| value > PRESENCE_RECHECK),
                };
                if matches_stage {
                    return Ok(deadline);
                }
            }
        })
        .await
        .map_err(|_| std::io::Error::other(format!("owner did not reach {stage}")))?
    }
}

impl SubscriptionClock for ManualSubscriptionClock {
    fn now(&self) -> DateTime<Utc> {
        *self
            .wall
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn monotonic_now(&self) -> Instant {
        *self
            .monotonic
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn sleep_until(
        &self,
        deadline: Instant,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            let _observed = self.sleep_deadlines.send(deadline);
            loop {
                let advanced = self.advanced.notified();
                tokio::pin!(advanced);
                advanced.as_mut().enable();
                if self.monotonic_now() >= deadline {
                    return;
                }
                advanced.await;
            }
        })
    }
}

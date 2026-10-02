use chrono::{DateTime, Utc};
use message_board::{
    MessageId, SubscriptionMode, SubscriptionScope, ThreadSubscriptionRecord, WhenIdle,
};
use std::{collections::HashMap, time::Duration};
use tokio::time::Instant;

pub(super) const PRESENCE_INTERVAL: Duration = Duration::from_secs(30);

pub(super) struct RootSubscriptionFacts {
    pub mode: SubscriptionMode,
    pub when_idle: WhenIdle,
    pub quiet_seconds: u64,
    pub cap_seconds: u64,
    pub opened_at: DateTime<Utc>,
    pub last_arrival_at: DateTime<Utc>,
    pub held_since: Option<DateTime<Utc>>,
    pub next_retry_at: Option<DateTime<Utc>>,
    pub retry_attempts: u32,
}

pub(super) fn root_facts(
    records: &[ThreadSubscriptionRecord],
) -> HashMap<MessageId, RootSubscriptionFacts> {
    let mut facts = HashMap::new();
    // Storage excludes shadowed Topic roots; a Thread record still takes precedence here.
    for record in records
        .iter()
        .filter(|record| matches!(record.scope(), SubscriptionScope::Thread { .. }))
        .chain(
            records
                .iter()
                .filter(|record| matches!(record.scope(), SubscriptionScope::Topic { .. })),
        )
    {
        for root in record.roots() {
            facts
                .entry(root.root_message_id().clone())
                .or_insert_with(|| RootSubscriptionFacts {
                    mode: record.policy().mode(),
                    when_idle: record.policy().when_idle(),
                    quiet_seconds: record.policy().timing().quiet_seconds(),
                    cap_seconds: record.policy().timing().cap_seconds(),
                    opened_at: root.opened_at(),
                    last_arrival_at: root.last_arrival_at(),
                    held_since: root.held_since(),
                    next_retry_at: root.next_retry_at(),
                    retry_attempts: root.retry_attempts(),
                });
        }
    }
    facts
}

pub(super) fn wall_deadline(
    wall: DateTime<Utc>,
    now: DateTime<Utc>,
    monotonic: Instant,
) -> Instant {
    monotonic
        .checked_add(
            wall.signed_duration_since(now)
                .to_std()
                .unwrap_or(Duration::ZERO),
        )
        .unwrap_or(monotonic)
}

pub(super) fn delivery_retry_delay(attempts: u32) -> Duration {
    Duration::from_secs(30)
        .saturating_mul(1_u32.checked_shl(attempts.min(5)).unwrap_or(32))
        .min(Duration::from_secs(600))
}

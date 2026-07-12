use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

pub(crate) const MODEL_CAPACITY_MAX_RETRIES: usize = 10;
pub(crate) const MODEL_CAPACITY_RETRY_DELAY_SECONDS: u64 = 300;
const TEST_CAPACITY_RETRY_DELAY_ENV: &str = "CODEX_ROUTER_TEST_CAPACITY_RETRY_DELAY_SECONDS";
const MAX_TEST_CAPACITY_RETRY_DELAY_SECONDS: u64 = 300;
const ENTRY_TTL: Duration = Duration::from_secs(60 * 60);
const MAX_TRACKED_THREADS: usize = 1024;
pub(crate) const MAX_THREAD_ID_BYTES: usize = 256;

#[derive(Clone)]
pub(crate) struct CapacityRetryTracker {
    entries: Arc<Mutex<HashMap<String, CapacityRetryEntry>>>,
}

#[derive(Clone, Debug)]
struct CapacityRetryEntry {
    retry_count: usize,
    last_seen: Instant,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CapacityRetryOutcome {
    Retry { retry_after_seconds: u64 },
    Exhausted,
    Full,
}

impl CapacityRetryTracker {
    pub(crate) fn new() -> Self {
        Self {
            entries: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub(crate) fn record_or_exhaust(&self, thread_id: &str) -> CapacityRetryOutcome {
        self.record_or_exhaust_at(thread_id, Instant::now())
    }

    pub(crate) fn clear(&self, thread_id: &str) {
        self.clear_at(thread_id, Instant::now());
    }

    fn record_or_exhaust_at(&self, thread_id: &str, now: Instant) -> CapacityRetryOutcome {
        let Ok(mut entries) = self.entries.lock() else {
            return CapacityRetryOutcome::Full;
        };
        entries.retain(|_, entry| now.duration_since(entry.last_seen) < ENTRY_TTL);
        let entry = if let Some(entry) = entries.get_mut(thread_id) {
            entry
        } else {
            if entries.len() >= MAX_TRACKED_THREADS {
                return CapacityRetryOutcome::Full;
            }
            entries.insert(
                thread_id.to_owned(),
                CapacityRetryEntry {
                    retry_count: 0,
                    last_seen: now,
                },
            );
            let Some(entry) = entries.get_mut(thread_id) else {
                return CapacityRetryOutcome::Full;
            };
            entry
        };
        entry.last_seen = now;
        if entry.retry_count >= MODEL_CAPACITY_MAX_RETRIES {
            return CapacityRetryOutcome::Exhausted;
        }
        entry.retry_count += 1;
        CapacityRetryOutcome::Retry {
            retry_after_seconds: model_capacity_retry_delay_seconds(),
        }
    }

    fn clear_at(&self, thread_id: &str, now: Instant) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.retain(|_, entry| now.duration_since(entry.last_seen) < ENTRY_TTL);
            entries.remove(thread_id);
        }
    }
}

fn model_capacity_retry_delay_seconds() -> u64 {
    #[cfg(debug_assertions)]
    {
        bounded_positive_test_delay(
            std::env::var(TEST_CAPACITY_RETRY_DELAY_ENV).ok().as_deref(),
            MODEL_CAPACITY_RETRY_DELAY_SECONDS,
            MAX_TEST_CAPACITY_RETRY_DELAY_SECONDS,
        )
    }
    #[cfg(not(debug_assertions))]
    MODEL_CAPACITY_RETRY_DELAY_SECONDS
}

fn bounded_positive_test_delay(value: Option<&str>, default: u64, maximum: u64) -> u64 {
    value
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| (1..=maximum).contains(value))
        .unwrap_or(default)
}

impl Default for CapacityRetryTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for CapacityRetryTracker {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CapacityRetryTracker")
            .field(
                "tracked_thread_count",
                &self.entries.lock().map_or(0, |entries| entries.len()),
            )
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::CapacityRetryOutcome;
    use super::CapacityRetryTracker;
    use super::MAX_TRACKED_THREADS;
    use super::MODEL_CAPACITY_RETRY_DELAY_SECONDS;
    use super::bounded_positive_test_delay;
    use std::time::Duration;
    use std::time::Instant;

    #[test]
    fn ten_capacity_retries_then_terminal() {
        let tracker = CapacityRetryTracker::new();
        let now = Instant::now();
        for _ in 0..10 {
            assert_eq!(
                tracker.record_or_exhaust_at("thread_1", now),
                CapacityRetryOutcome::Retry {
                    retry_after_seconds: MODEL_CAPACITY_RETRY_DELAY_SECONDS
                }
            );
        }
        assert_eq!(
            tracker.record_or_exhaust_at("thread_1", now),
            CapacityRetryOutcome::Exhausted
        );
    }

    #[test]
    fn clear_and_expiry_reset_capacity_retries() {
        let tracker = CapacityRetryTracker::new();
        let now = Instant::now();
        for _ in 0..10 {
            let _retry = tracker.record_or_exhaust_at("thread_1", now);
        }
        assert_eq!(
            tracker.record_or_exhaust_at("thread_1", now),
            CapacityRetryOutcome::Exhausted
        );
        tracker.clear_at("thread_1", now);
        assert!(matches!(
            tracker.record_or_exhaust_at("thread_1", now),
            CapacityRetryOutcome::Retry { .. }
        ));
        for _ in 0..9 {
            let _retry = tracker.record_or_exhaust_at("thread_1", now);
        }
        assert!(matches!(
            tracker.record_or_exhaust_at("thread_1", now + Duration::from_secs(3_601)),
            CapacityRetryOutcome::Retry { .. }
        ));
    }

    #[test]
    fn tracker_enforces_hard_cap_and_accepts_after_expiry() {
        let tracker = CapacityRetryTracker::new();
        let now = Instant::now();
        for identity_index in 0..MAX_TRACKED_THREADS {
            assert!(matches!(
                tracker.record_or_exhaust_at(&format!("thread_{identity_index}"), now),
                CapacityRetryOutcome::Retry { .. }
            ));
        }
        assert_eq!(
            tracker.record_or_exhaust_at("rejected_thread", now),
            CapacityRetryOutcome::Full
        );
        assert_eq!(
            tracker.record_or_exhaust_at("rejected_thread", now + Duration::from_secs(3_601)),
            CapacityRetryOutcome::Retry {
                retry_after_seconds: MODEL_CAPACITY_RETRY_DELAY_SECONDS
            }
        );
    }

    #[test]
    fn debug_output_redacts_thread_identities() {
        let tracker = CapacityRetryTracker::new();
        let secret_thread_id = "thread_secret_value";
        let _outcome = tracker.record_or_exhaust(secret_thread_id);
        let debug_output = format!("{tracker:?}");
        assert!(!debug_output.contains(secret_thread_id));
        assert!(debug_output.contains("tracked_thread_count: 1"));
    }

    #[test]
    fn test_delay_override_is_positive_bounded_and_fails_closed() {
        assert_eq!(bounded_positive_test_delay(None, 300, 300), 300);
        assert_eq!(bounded_positive_test_delay(Some("2"), 300, 300), 2);
        for invalid in ["", "0", "301", "not-a-number"] {
            assert_eq!(bounded_positive_test_delay(Some(invalid), 300, 300), 300);
        }
    }
}

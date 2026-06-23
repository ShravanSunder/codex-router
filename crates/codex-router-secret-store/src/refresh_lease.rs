//! Deterministic refresh lease coordination.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

/// Clock used by refresh leases.
pub trait LeaseClock: Clone {
    /// Returns the current logical time.
    fn now(&self) -> u64;
}

/// Manual deterministic test clock.
#[derive(Clone, Debug)]
pub struct ManualClock {
    now: Arc<Mutex<u64>>,
}

impl ManualClock {
    /// Creates a manual clock.
    #[must_use]
    pub fn new(now: u64) -> Self {
        Self {
            now: Arc::new(Mutex::new(now)),
        }
    }

    /// Advances the clock.
    pub fn advance(&self, delta: u64) {
        let mut now = lock_or_recover(&self.now);
        *now += delta;
    }
}

impl LeaseClock for ManualClock {
    fn now(&self) -> u64 {
        *lock_or_recover(&self.now)
    }
}

/// Refresh lease acquisition result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LeaseAcquisition {
    /// Caller owns the refresh.
    Acquired(RefreshLease),
    /// Another caller owns the refresh.
    Follower {
        /// Current owner.
        owner: String,
        /// Logical expiry time.
        expires_at: u64,
    },
}

/// Owned refresh lease.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RefreshLease {
    key: String,
    owner: String,
    acquired_at: u64,
}

/// Refresh lease manager.
#[derive(Debug)]
pub struct RefreshLeaseManager<C>
where
    C: LeaseClock,
{
    clock: C,
    leases: Mutex<HashMap<String, LeaseState>>,
}

impl<C> RefreshLeaseManager<C>
where
    C: LeaseClock,
{
    /// Creates a lease manager.
    #[must_use]
    pub fn new(clock: C) -> Self {
        Self {
            clock,
            leases: Mutex::new(HashMap::new()),
        }
    }

    /// Attempts to acquire a refresh lease for a logical key.
    #[must_use]
    pub fn acquire(&self, key: &str, owner: &str, ttl: u64) -> LeaseAcquisition {
        let now = self.clock.now();
        let mut leases = lock_or_recover(&self.leases);

        if let Some(existing_lease) = leases.get(key)
            && existing_lease.expires_at > now
        {
            return LeaseAcquisition::Follower {
                owner: existing_lease.owner.clone(),
                expires_at: existing_lease.expires_at,
            };
        }

        leases.insert(
            key.to_owned(),
            LeaseState {
                owner: owner.to_owned(),
                acquired_at: now,
                expires_at: now + ttl,
            },
        );

        LeaseAcquisition::Acquired(RefreshLease {
            key: key.to_owned(),
            owner: owner.to_owned(),
            acquired_at: now,
        })
    }

    /// Finishes a lease if it is still owned by the same caller.
    pub fn finish(&self, lease: RefreshLease) {
        let mut leases = lock_or_recover(&self.leases);
        let should_remove = leases
            .get(&lease.key)
            .map(|state| state.owner == lease.owner && state.acquired_at == lease.acquired_at)
            .unwrap_or(false);

        if should_remove {
            leases.remove(&lease.key);
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct LeaseState {
    owner: String,
    acquired_at: u64,
    expires_at: u64,
}

fn lock_or_recover<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

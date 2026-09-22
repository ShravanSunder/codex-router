//! Process-local session affinity ownership and real-activity renewal.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::sync::Mutex;

use codex_router_core::ids::AccountId;
use codex_router_core::routes::RouteBand;
use codex_router_state::session_account_affinity::SessionAccountAffinity;

use crate::db_write_actor::DbWriteActor;
use crate::db_write_actor::DbWriteCommand;

/// Strict soft-affinity idle lifetime.
pub const SESSION_ACCOUNT_AFFINITY_IDLE_TTL_SECONDS: u64 = 7_200;
const CACHE_PRUNE_INTERVAL_SECONDS: u64 = 60;

/// Shared process-local session affinity cache.
pub type SharedSessionAccountAffinityCache = Arc<Mutex<SessionAccountAffinityCache>>;

/// Cache access failed because its mutex was poisoned.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SessionAccountAffinityCacheUnavailable;

#[derive(Debug)]
struct SessionAccountAffinityEntry {
    account_id: AccountId,
    last_seen_unix_seconds: u64,
    owner_token: Arc<()>,
}

/// Process-local current owner for each supplied session identity.
#[derive(Debug, Default)]
pub struct SessionAccountAffinityCache {
    entries: HashMap<String, SessionAccountAffinityEntry>,
    last_pruned_unix_seconds: Option<u64>,
}

impl SessionAccountAffinityCache {
    /// Creates a shared empty cache.
    #[must_use]
    pub fn shared() -> SharedSessionAccountAffinityCache {
        Arc::new(Mutex::new(Self::default()))
    }
}

/// A current cache owner plus the token-bound activity handle for that owner.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionAccountAffinitySelection {
    account_id: AccountId,
    activity_handle: SessionAffinityActivityHandle,
}

impl SessionAccountAffinitySelection {
    /// Returns the selected owner.
    #[must_use]
    pub const fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    /// Returns the token-bound activity handle.
    #[must_use]
    pub const fn activity_handle(&self) -> &SessionAffinityActivityHandle {
        &self.activity_handle
    }
}

/// Renews affinity only while its exact ownership instance is still current.
#[derive(Clone)]
pub struct SessionAffinityActivityHandle {
    cache: SharedSessionAccountAffinityCache,
    session_id: String,
    account_id: AccountId,
    owner_token: Arc<()>,
    route_band: RouteBand,
    writer: Option<DbWriteActor>,
}

impl fmt::Debug for SessionAffinityActivityHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SessionAffinityActivityHandle")
            .field("session_id", &"[redacted]")
            .field("route_band", &self.route_band)
            .finish_non_exhaustive()
    }
}

impl PartialEq for SessionAffinityActivityHandle {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.cache, &other.cache)
            && self.session_id == other.session_id
            && self.account_id == other.account_id
            && Arc::ptr_eq(&self.owner_token, &other.owner_token)
            && self.route_band == other.route_band
    }
}

impl Eq for SessionAffinityActivityHandle {}

impl SessionAffinityActivityHandle {
    /// Renews this ownership instance after successful real request forwarding.
    pub fn touch_if_current(
        &self,
        now_unix_seconds: u64,
    ) -> Result<bool, SessionAccountAffinityCacheUnavailable> {
        let mut cache = self
            .cache
            .lock()
            .map_err(|_error| SessionAccountAffinityCacheUnavailable)?;
        let Some(entry) = cache.entries.get_mut(&self.session_id) else {
            return Ok(false);
        };
        if entry.account_id != self.account_id
            || !Arc::ptr_eq(&entry.owner_token, &self.owner_token)
        {
            return Ok(false);
        }

        entry.last_seen_unix_seconds = entry.last_seen_unix_seconds.max(now_unix_seconds);
        enqueue_affinity_write(
            self.writer.as_ref(),
            &self.session_id,
            &self.account_id,
            self.route_band,
            entry.last_seen_unix_seconds,
        );
        Ok(true)
    }
}

/// Looks up a fresh current owner without renewing it.
pub fn lookup_session_account_affinity(
    cache: &SharedSessionAccountAffinityCache,
    session_id: &str,
    route_band: RouteBand,
    writer: Option<&DbWriteActor>,
    now_unix_seconds: u64,
) -> Result<Option<SessionAccountAffinitySelection>, SessionAccountAffinityCacheUnavailable> {
    let mut cache_guard = cache
        .lock()
        .map_err(|_error| SessionAccountAffinityCacheUnavailable)?;
    prune_if_due(&mut cache_guard, now_unix_seconds);
    let Some(entry) = cache_guard.entries.get(session_id) else {
        return Ok(None);
    };
    if now_unix_seconds.saturating_sub(entry.last_seen_unix_seconds)
        >= SESSION_ACCOUNT_AFFINITY_IDLE_TTL_SECONDS
    {
        return Ok(None);
    }
    Ok(Some(selection_from_entry(
        cache, session_id, route_band, writer, entry,
    )))
}

/// Rechecks live state after a database await and seeds only a fresh persisted row.
pub fn reconcile_persisted_session_account_affinity(
    cache: &SharedSessionAccountAffinityCache,
    session_id: &str,
    persisted: Option<&SessionAccountAffinity>,
    route_band: RouteBand,
    writer: Option<&DbWriteActor>,
    now_unix_seconds: u64,
) -> Result<Option<SessionAccountAffinitySelection>, SessionAccountAffinityCacheUnavailable> {
    let mut cache_guard = cache
        .lock()
        .map_err(|_error| SessionAccountAffinityCacheUnavailable)?;
    prune_if_due(&mut cache_guard, now_unix_seconds);
    if let Some(live_entry) = cache_guard.entries.get(session_id)
        && now_unix_seconds.saturating_sub(live_entry.last_seen_unix_seconds)
            < SESSION_ACCOUNT_AFFINITY_IDLE_TTL_SECONDS
    {
        return Ok(Some(selection_from_entry(
            cache, session_id, route_band, writer, live_entry,
        )));
    }
    let Some(persisted) = persisted.filter(|persisted| {
        persisted.session_id() == session_id
            && now_unix_seconds.saturating_sub(persisted.last_seen_unix_seconds())
                < SESSION_ACCOUNT_AFFINITY_IDLE_TTL_SECONDS
    }) else {
        return Ok(None);
    };

    let owner_token = cache_guard
        .entries
        .get(session_id)
        .filter(|entry| entry.account_id == *persisted.account_id())
        .map_or_else(|| Arc::new(()), |entry| Arc::clone(&entry.owner_token));
    cache_guard.entries.insert(
        session_id.to_owned(),
        SessionAccountAffinityEntry {
            account_id: persisted.account_id().clone(),
            last_seen_unix_seconds: persisted.last_seen_unix_seconds(),
            owner_token: Arc::clone(&owner_token),
        },
    );
    Ok(Some(SessionAccountAffinitySelection {
        account_id: persisted.account_id().clone(),
        activity_handle: SessionAffinityActivityHandle {
            cache: Arc::clone(cache),
            session_id: session_id.to_owned(),
            account_id: persisted.account_id().clone(),
            owner_token,
            route_band,
            writer: writer.cloned(),
        },
    }))
}

/// Publishes the selected owner and queues durability before selector serialization is released.
pub fn publish_session_account_affinity(
    cache: &SharedSessionAccountAffinityCache,
    session_id: &str,
    account_id: &AccountId,
    route_band: RouteBand,
    writer: Option<&DbWriteActor>,
    now_unix_seconds: u64,
) -> Result<SessionAccountAffinitySelection, SessionAccountAffinityCacheUnavailable> {
    let mut cache_guard = cache
        .lock()
        .map_err(|_error| SessionAccountAffinityCacheUnavailable)?;
    prune_if_due(&mut cache_guard, now_unix_seconds);
    let entry = cache_guard
        .entries
        .entry(session_id.to_owned())
        .or_insert_with(|| SessionAccountAffinityEntry {
            account_id: account_id.clone(),
            last_seen_unix_seconds: now_unix_seconds,
            owner_token: Arc::new(()),
        });
    if entry.account_id != *account_id {
        entry.account_id = account_id.clone();
        entry.owner_token = Arc::new(());
    }
    entry.last_seen_unix_seconds = entry.last_seen_unix_seconds.max(now_unix_seconds);
    enqueue_affinity_write(
        writer,
        session_id,
        account_id,
        route_band,
        entry.last_seen_unix_seconds,
    );
    Ok(selection_from_entry(
        cache, session_id, route_band, writer, entry,
    ))
}

fn selection_from_entry(
    cache: &SharedSessionAccountAffinityCache,
    session_id: &str,
    route_band: RouteBand,
    writer: Option<&DbWriteActor>,
    entry: &SessionAccountAffinityEntry,
) -> SessionAccountAffinitySelection {
    SessionAccountAffinitySelection {
        account_id: entry.account_id.clone(),
        activity_handle: SessionAffinityActivityHandle {
            cache: Arc::clone(cache),
            session_id: session_id.to_owned(),
            account_id: entry.account_id.clone(),
            owner_token: Arc::clone(&entry.owner_token),
            route_band,
            writer: writer.cloned(),
        },
    }
}

fn prune_if_due(cache: &mut SessionAccountAffinityCache, now_unix_seconds: u64) {
    if cache.last_pruned_unix_seconds.is_some_and(|last_pruned| {
        now_unix_seconds.saturating_sub(last_pruned) < CACHE_PRUNE_INTERVAL_SECONDS
    }) {
        return;
    }
    cache.last_pruned_unix_seconds = Some(now_unix_seconds);
    cache.entries.retain(|_, entry| {
        now_unix_seconds.saturating_sub(entry.last_seen_unix_seconds)
            < SESSION_ACCOUNT_AFFINITY_IDLE_TTL_SECONDS
            || Arc::strong_count(&entry.owner_token) > 1
    });
}

fn enqueue_affinity_write(
    writer: Option<&DbWriteActor>,
    session_id: &str,
    account_id: &AccountId,
    route_band: RouteBand,
    last_seen_unix_seconds: u64,
) {
    let Some(writer) = writer else {
        return;
    };
    let _enqueue_result = writer.try_enqueue(DbWriteCommand::session_account_affinity(
        route_band,
        SessionAccountAffinity::new(session_id, account_id.clone(), last_seen_unix_seconds),
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account_id(value: &str) -> AccountId {
        AccountId::new(value).unwrap_or_else(|error| panic!("test account id: {error}"))
    }

    #[test]
    fn lookup_is_strict_at_two_hours_and_does_not_renew() {
        for (age, expected_fresh) in [(7_199, true), (7_200, false), (7_201, false)] {
            let cache = SessionAccountAffinityCache::shared();
            let selected = publish_session_account_affinity(
                &cache,
                "session-boundary",
                &account_id("acct-a"),
                RouteBand::Responses,
                None,
                1_000,
            )
            .unwrap_or_else(|_| panic!("publication should succeed"));
            drop(selected);

            let lookup = lookup_session_account_affinity(
                &cache,
                "session-boundary",
                RouteBand::Responses,
                None,
                1_000 + age,
            )
            .unwrap_or_else(|_| panic!("lookup should succeed"));
            assert_eq!(lookup.is_some(), expected_fresh, "age={age}");
        }
    }

    #[test]
    fn stale_handle_cannot_renew_after_a_to_b_to_a() {
        let cache = SessionAccountAffinityCache::shared();
        let original_a = publish_session_account_affinity(
            &cache,
            "session-cycle",
            &account_id("acct-a"),
            RouteBand::Responses,
            None,
            1_000,
        )
        .unwrap_or_else(|_| panic!("A publication should succeed"));
        let _b = publish_session_account_affinity(
            &cache,
            "session-cycle",
            &account_id("acct-b"),
            RouteBand::Responses,
            None,
            1_100,
        )
        .unwrap_or_else(|_| panic!("B publication should succeed"));
        let current_a = publish_session_account_affinity(
            &cache,
            "session-cycle",
            &account_id("acct-a"),
            RouteBand::Responses,
            None,
            1_200,
        )
        .unwrap_or_else(|_| panic!("second A publication should succeed"));

        assert!(
            !original_a
                .activity_handle()
                .touch_if_current(2_000)
                .unwrap()
        );
        assert!(current_a.activity_handle().touch_if_current(2_000).unwrap());
    }

    #[test]
    fn persisted_reconciliation_preserves_last_seen_and_live_touch_wins() {
        let cache = SessionAccountAffinityCache::shared();
        let persisted = SessionAccountAffinity::new("session-db", account_id("acct-a"), 1_000);
        let seeded = reconcile_persisted_session_account_affinity(
            &cache,
            "session-db",
            Some(&persisted),
            RouteBand::Responses,
            None,
            8_199,
        )
        .unwrap_or_else(|_| panic!("reconciliation should succeed"))
        .unwrap_or_else(|| panic!("7,199-second row should seed"));
        assert!(
            lookup_session_account_affinity(
                &cache,
                "session-db",
                RouteBand::Responses,
                None,
                8_200,
            )
            .unwrap_or_else(|_| panic!("lookup-only boundary check should succeed"))
            .is_none(),
            "seeding at age 7,199 must preserve persisted last-seen and expire at age 7,200"
        );
        assert!(seeded.activity_handle().touch_if_current(8_300).unwrap());

        let older = SessionAccountAffinity::new("session-db", account_id("acct-b"), 8_250);
        let reconciled = reconcile_persisted_session_account_affinity(
            &cache,
            "session-db",
            Some(&older),
            RouteBand::Responses,
            None,
            8_301,
        )
        .unwrap_or_else(|_| panic!("second reconciliation should succeed"))
        .unwrap_or_else(|| panic!("live owner should remain"));
        assert_eq!(reconciled.account_id().as_str(), "acct-a");
    }

    #[test]
    fn persisted_reconciliation_is_strict_at_two_hours() {
        for (age, expected_fresh) in [(7_199, true), (7_200, false), (7_201, false)] {
            let cache = SessionAccountAffinityCache::shared();
            let persisted =
                SessionAccountAffinity::new("session-db-boundary", account_id("acct-a"), 1_000);
            let reconciled = reconcile_persisted_session_account_affinity(
                &cache,
                "session-db-boundary",
                Some(&persisted),
                RouteBand::Responses,
                None,
                1_000 + age,
            )
            .unwrap_or_else(|_| panic!("reconciliation should succeed"));
            assert_eq!(reconciled.is_some(), expected_fresh, "age={age}");
        }
    }

    #[test]
    fn expired_entry_with_current_handle_can_renew_after_real_activity() {
        let cache = SessionAccountAffinityCache::shared();
        let published = publish_session_account_affinity(
            &cache,
            "session-idle-activity",
            &account_id("acct-a"),
            RouteBand::Responses,
            None,
            1_000,
        )
        .unwrap_or_else(|_| panic!("publication should succeed"));

        assert!(
            lookup_session_account_affinity(
                &cache,
                "session-idle-activity",
                RouteBand::Responses,
                None,
                8_200,
            )
            .unwrap_or_else(|_| panic!("lookup should succeed"))
            .is_none()
        );
        assert!(
            published
                .activity_handle()
                .touch_if_current(8_300)
                .unwrap_or_else(|_| panic!("touch should succeed"))
        );
        assert!(
            lookup_session_account_affinity(
                &cache,
                "session-idle-activity",
                RouteBand::Responses,
                None,
                8_300,
            )
            .unwrap_or_else(|_| panic!("lookup should succeed"))
            .is_some()
        );
    }
}

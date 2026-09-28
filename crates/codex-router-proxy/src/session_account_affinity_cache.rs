//! Process-local session affinity ownership and real-activity renewal.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use codex_router_core::ids::AccountId;
use codex_router_core::provider::Provider;
use codex_router_core::routes::RouteBand;
use codex_router_state::session_account_affinity::SessionAccountAffinity;

use crate::db_write_actor::DbWriteActor;
use crate::db_write_actor::DbWriteCommand;

/// Default idle lifetime shared by provider session pins.
pub const DEFAULT_SESSION_PIN_IDLE_TTL: Duration = Duration::from_secs(75 * 60);
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

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct SessionAffinityKey {
    provider: Provider,
    session_id: String,
}

/// Process-local current owner for each provider-scoped session identity.
#[derive(Debug)]
pub struct SessionAccountAffinityCache {
    entries: HashMap<SessionAffinityKey, SessionAccountAffinityEntry>,
    last_pruned_unix_seconds: Option<u64>,
    idle_ttl: Duration,
}

impl SessionAccountAffinityCache {
    /// Creates a shared empty cache with the configured pin idle lifetime.
    #[must_use]
    pub fn shared(idle_ttl: Duration) -> SharedSessionAccountAffinityCache {
        Arc::new(Mutex::new(Self {
            entries: HashMap::new(),
            last_pruned_unix_seconds: None,
            idle_ttl,
        }))
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
    provider: Provider,
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
            && self.provider == other.provider
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
        let key = SessionAffinityKey {
            provider: self.provider,
            session_id: self.session_id.clone(),
        };
        let Some(entry) = cache.entries.get_mut(&key) else {
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
            self.provider,
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
    provider: Provider,
    session_id: &str,
    route_band: RouteBand,
    writer: Option<&DbWriteActor>,
    now_unix_seconds: u64,
) -> Result<Option<SessionAccountAffinitySelection>, SessionAccountAffinityCacheUnavailable> {
    let mut cache_guard = cache
        .lock()
        .map_err(|_error| SessionAccountAffinityCacheUnavailable)?;
    prune_if_due(&mut cache_guard, now_unix_seconds);
    let key = SessionAffinityKey {
        provider,
        session_id: session_id.to_owned(),
    };
    let Some(entry) = cache_guard.entries.get(&key) else {
        return Ok(None);
    };
    if now_unix_seconds.saturating_sub(entry.last_seen_unix_seconds)
        >= cache_guard.idle_ttl.as_secs()
    {
        return Ok(None);
    }
    Ok(Some(selection_from_entry(
        cache, provider, session_id, route_band, writer, entry,
    )))
}

/// Rechecks live state after a database await and seeds only a fresh persisted row.
pub fn reconcile_persisted_session_account_affinity(
    cache: &SharedSessionAccountAffinityCache,
    provider: Provider,
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
    let key = SessionAffinityKey {
        provider,
        session_id: session_id.to_owned(),
    };
    if let Some(live_entry) = cache_guard.entries.get(&key)
        && now_unix_seconds.saturating_sub(live_entry.last_seen_unix_seconds)
            < cache_guard.idle_ttl.as_secs()
    {
        return Ok(Some(selection_from_entry(
            cache, provider, session_id, route_band, writer, live_entry,
        )));
    }
    let Some(persisted) = persisted.filter(|persisted| {
        persisted.provider() == provider
            && persisted.session_id() == session_id
            && now_unix_seconds.saturating_sub(persisted.last_seen_unix_seconds())
                < cache_guard.idle_ttl.as_secs()
    }) else {
        return Ok(None);
    };
    let Some(persisted_account_id) = persisted.account_id() else {
        return Ok(None);
    };

    let owner_token = cache_guard
        .entries
        .get(&key)
        .filter(|entry| entry.account_id == *persisted_account_id)
        .map_or_else(|| Arc::new(()), |entry| Arc::clone(&entry.owner_token));
    cache_guard.entries.insert(
        key,
        SessionAccountAffinityEntry {
            account_id: persisted_account_id.clone(),
            last_seen_unix_seconds: persisted.last_seen_unix_seconds(),
            owner_token: Arc::clone(&owner_token),
        },
    );
    Ok(Some(SessionAccountAffinitySelection {
        account_id: persisted_account_id.clone(),
        activity_handle: SessionAffinityActivityHandle {
            cache: Arc::clone(cache),
            provider,
            session_id: session_id.to_owned(),
            account_id: persisted_account_id.clone(),
            owner_token,
            route_band,
            writer: writer.cloned(),
        },
    }))
}

/// Publishes the selected owner and queues durability before selector serialization is released.
pub fn publish_session_account_affinity(
    cache: &SharedSessionAccountAffinityCache,
    provider: Provider,
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
    let key = SessionAffinityKey {
        provider,
        session_id: session_id.to_owned(),
    };
    let entry = cache_guard
        .entries
        .entry(key)
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
        provider,
        session_id,
        account_id,
        route_band,
        entry.last_seen_unix_seconds,
    );
    Ok(selection_from_entry(
        cache, provider, session_id, route_band, writer, entry,
    ))
}

fn selection_from_entry(
    cache: &SharedSessionAccountAffinityCache,
    provider: Provider,
    session_id: &str,
    route_band: RouteBand,
    writer: Option<&DbWriteActor>,
    entry: &SessionAccountAffinityEntry,
) -> SessionAccountAffinitySelection {
    SessionAccountAffinitySelection {
        account_id: entry.account_id.clone(),
        activity_handle: SessionAffinityActivityHandle {
            cache: Arc::clone(cache),
            provider,
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
    let idle_ttl_seconds = cache.idle_ttl.as_secs();
    cache.entries.retain(|_, entry| {
        now_unix_seconds.saturating_sub(entry.last_seen_unix_seconds) < idle_ttl_seconds
            || Arc::strong_count(&entry.owner_token) > 1
    });
}

fn enqueue_affinity_write(
    writer: Option<&DbWriteActor>,
    provider: Provider,
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
        SessionAccountAffinity::with_pin_state(
            provider,
            session_id,
            Some(account_id.clone()),
            0,
            last_seen_unix_seconds,
        ),
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account_id(value: &str) -> AccountId {
        AccountId::new(value).unwrap_or_else(|error| panic!("test account id: {error}"))
    }

    #[test]
    fn default_ttl_expires_after_75_idle_minutes_without_renewal() {
        for (age, expected_fresh) in [(4_499, true), (4_500, false), (4_501, false)] {
            let cache = SessionAccountAffinityCache::shared(DEFAULT_SESSION_PIN_IDLE_TTL);
            let selected = publish_session_account_affinity(
                &cache,
                Provider::Openai,
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
                Provider::Openai,
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
    fn configured_ttl_is_used_for_expiry() {
        let cache = SessionAccountAffinityCache::shared(Duration::from_secs(30 * 60));
        let _selected = publish_session_account_affinity(
            &cache,
            Provider::Openai,
            "session-configured-ttl",
            &account_id("acct-a"),
            RouteBand::Responses,
            None,
            1_000,
        )
        .unwrap_or_else(|_| panic!("publication should succeed"));

        for (age, expected_fresh) in [(1_799, true), (1_800, false)] {
            let lookup = lookup_session_account_affinity(
                &cache,
                Provider::Openai,
                "session-configured-ttl",
                RouteBand::Responses,
                None,
                1_000 + age,
            )
            .unwrap_or_else(|_| panic!("lookup should succeed"));
            assert_eq!(lookup.is_some(), expected_fresh, "age={age}");
        }
    }

    #[test]
    fn same_session_id_has_independent_provider_owners() {
        let cache = SessionAccountAffinityCache::shared(DEFAULT_SESSION_PIN_IDLE_TTL);
        let openai_selection = publish_session_account_affinity(
            &cache,
            Provider::Openai,
            "shared-session-id",
            &account_id("acct-openai"),
            RouteBand::Responses,
            None,
            1_000,
        )
        .unwrap_or_else(|_| panic!("OpenAI publication should succeed"));
        let _claude_selection = publish_session_account_affinity(
            &cache,
            Provider::Claude,
            "shared-session-id",
            &account_id("acct-claude"),
            RouteBand::Responses,
            None,
            1_100,
        )
        .unwrap_or_else(|_| panic!("Claude publication should succeed"));

        let openai_lookup = lookup_session_account_affinity(
            &cache,
            Provider::Openai,
            "shared-session-id",
            RouteBand::Responses,
            None,
            1_101,
        )
        .unwrap_or_else(|_| panic!("OpenAI lookup should succeed"));
        assert_eq!(
            openai_lookup.map(|selection| selection.account_id().clone()),
            Some(openai_selection.account_id().clone())
        );
    }

    #[test]
    fn stale_handle_cannot_renew_after_a_to_b_to_a() {
        let cache = SessionAccountAffinityCache::shared(DEFAULT_SESSION_PIN_IDLE_TTL);
        let original_a = publish_session_account_affinity(
            &cache,
            Provider::Openai,
            "session-cycle",
            &account_id("acct-a"),
            RouteBand::Responses,
            None,
            1_000,
        )
        .unwrap_or_else(|_| panic!("A publication should succeed"));
        let _b = publish_session_account_affinity(
            &cache,
            Provider::Openai,
            "session-cycle",
            &account_id("acct-b"),
            RouteBand::Responses,
            None,
            1_100,
        )
        .unwrap_or_else(|_| panic!("B publication should succeed"));
        let current_a = publish_session_account_affinity(
            &cache,
            Provider::Openai,
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
        let cache = SessionAccountAffinityCache::shared(DEFAULT_SESSION_PIN_IDLE_TTL);
        let persisted = SessionAccountAffinity::new("session-db", account_id("acct-a"), 1_000);
        let seeded = reconcile_persisted_session_account_affinity(
            &cache,
            Provider::Openai,
            "session-db",
            Some(&persisted),
            RouteBand::Responses,
            None,
            5_499,
        )
        .unwrap_or_else(|_| panic!("reconciliation should succeed"))
        .unwrap_or_else(|| panic!("4,499-second row should seed"));
        assert!(
            lookup_session_account_affinity(
                &cache,
                Provider::Openai,
                "session-db",
                RouteBand::Responses,
                None,
                5_500,
            )
            .unwrap_or_else(|_| panic!("lookup-only boundary check should succeed"))
            .is_none(),
            "seeding at age 4,499 must preserve persisted last-seen and expire at age 4,500"
        );
        assert!(seeded.activity_handle().touch_if_current(5_600).unwrap());

        let older = SessionAccountAffinity::new("session-db", account_id("acct-b"), 8_250);
        let reconciled = reconcile_persisted_session_account_affinity(
            &cache,
            Provider::Openai,
            "session-db",
            Some(&older),
            RouteBand::Responses,
            None,
            5_601,
        )
        .unwrap_or_else(|_| panic!("second reconciliation should succeed"))
        .unwrap_or_else(|| panic!("live owner should remain"));
        assert_eq!(reconciled.account_id().as_str(), "acct-a");
    }

    #[test]
    fn persisted_reconciliation_uses_default_75_minute_ttl() {
        for (age, expected_fresh) in [(4_499, true), (4_500, false), (4_501, false)] {
            let cache = SessionAccountAffinityCache::shared(DEFAULT_SESSION_PIN_IDLE_TTL);
            let persisted =
                SessionAccountAffinity::new("session-db-boundary", account_id("acct-a"), 1_000);
            let reconciled = reconcile_persisted_session_account_affinity(
                &cache,
                Provider::Openai,
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
        let cache = SessionAccountAffinityCache::shared(DEFAULT_SESSION_PIN_IDLE_TTL);
        let published = publish_session_account_affinity(
            &cache,
            Provider::Openai,
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
                Provider::Openai,
                "session-idle-activity",
                RouteBand::Responses,
                None,
                5_500,
            )
            .unwrap_or_else(|_| panic!("lookup should succeed"))
            .is_none()
        );
        assert!(
            published
                .activity_handle()
                .touch_if_current(5_600)
                .unwrap_or_else(|_| panic!("touch should succeed"))
        );
        assert!(
            lookup_session_account_affinity(
                &cache,
                Provider::Openai,
                "session-idle-activity",
                RouteBand::Responses,
                None,
                5_600,
            )
            .unwrap_or_else(|_| panic!("lookup should succeed"))
            .is_some()
        );
    }
}

//! Process-local session affinity ownership and real-activity renewal.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use codex_router_core::ids::AccountId;
use codex_router_core::provider::Provider;
use codex_router_core::routes::RouteBand;
use codex_router_state::session_account_affinity::PinObservation;
use codex_router_state::session_account_affinity::SessionAccountAffinity;
use codex_router_state::sqlite::AsyncSessionAccountAffinityRepository;
use codex_router_state::sqlite::StateStoreError;
use thiserror::Error;

use crate::claude_edge::response_completion::ClaudeResponseCompletion;
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

/// Failure while publishing a completed Claude session pin.
#[derive(Debug, Error)]
pub(crate) enum SessionAccountAffinityPublicationError {
    /// The process-local cache could not be accessed.
    #[error("session account affinity cache is unavailable")]
    CacheUnavailable,
    /// The versioned pin write failed.
    #[error("session account affinity persistence failed: {0}")]
    Persistence(#[from] StateStoreError),
}

#[derive(Debug)]
struct SessionAccountAffinityEntry {
    account_id: AccountId,
    pin_version: u64,
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
        if self.provider == Provider::Claude {
            return Ok(false);
        }
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
        .filter(|entry| {
            entry.account_id == *persisted_account_id
                && entry.pin_version == persisted.pin_version()
        })
        .map_or_else(|| Arc::new(()), |entry| Arc::clone(&entry.owner_token));
    cache_guard.entries.insert(
        key,
        SessionAccountAffinityEntry {
            account_id: persisted_account_id.clone(),
            pin_version: persisted.pin_version(),
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

/// Publishes Codex's selected owner immediately; Claude waits for response completion.
pub fn publish_session_account_affinity(
    cache: &SharedSessionAccountAffinityCache,
    provider: Provider,
    session_id: &str,
    account_id: &AccountId,
    route_band: RouteBand,
    writer: Option<&DbWriteActor>,
    now_unix_seconds: u64,
) -> Result<SessionAccountAffinitySelection, SessionAccountAffinityCacheUnavailable> {
    if provider == Provider::Claude {
        return Ok(SessionAccountAffinitySelection {
            account_id: account_id.clone(),
            activity_handle: SessionAffinityActivityHandle {
                cache: Arc::clone(cache),
                provider,
                session_id: session_id.to_owned(),
                account_id: account_id.clone(),
                owner_token: Arc::new(()),
                route_band,
                writer: None,
            },
        });
    }

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
            pin_version: 0,
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

/// Publishes a Claude pin only after its final response completion was successful.
pub(crate) async fn publish_claude_session_account_affinity_on_success<TRepository, TClock>(
    cache: &SharedSessionAccountAffinityCache,
    session_id: &str,
    observation: &PinObservation,
    attempt_account_id: &AccountId,
    completion: tokio::sync::oneshot::Receiver<ClaudeResponseCompletion>,
    repository: &TRepository,
    success_clock: TClock,
) -> Result<bool, SessionAccountAffinityPublicationError>
where
    TRepository: AsyncSessionAccountAffinityRepository + Sync,
    TClock: Fn() -> u64,
{
    if !matches!(completion.await, Ok(ClaudeResponseCompletion::Success)) {
        return Ok(false);
    }

    let pin_version = match observation.active_account() {
        None => {
            let Some(next_version) = observation.version().checked_add(1) else {
                return Ok(false);
            };
            next_version
        }
        Some(active_account) if active_account == attempt_account_id => observation.version(),
        Some(_) => return Ok(false),
    };
    let now_unix_seconds = success_clock();
    let pin_ttl_seconds = cache
        .lock()
        .map_err(|_error| SessionAccountAffinityPublicationError::CacheUnavailable)?
        .idle_ttl
        .as_secs();
    let desired_affinity = SessionAccountAffinity::with_pin_state(
        Provider::Claude,
        session_id,
        Some(attempt_account_id.clone()),
        pin_version,
        now_unix_seconds,
    );
    let compare_and_set_won = repository
        .compare_and_set_session_account_affinity(observation, &desired_affinity, pin_ttl_seconds)
        .await?;
    if !compare_and_set_won {
        return Ok(false);
    }

    cache_successful_claude_pin(
        cache,
        session_id,
        attempt_account_id,
        pin_version,
        now_unix_seconds,
    )?;
    Ok(true)
}

fn cache_successful_claude_pin(
    cache: &SharedSessionAccountAffinityCache,
    session_id: &str,
    account_id: &AccountId,
    pin_version: u64,
    now_unix_seconds: u64,
) -> Result<(), SessionAccountAffinityPublicationError> {
    let mut cache_guard = cache
        .lock()
        .map_err(|_error| SessionAccountAffinityPublicationError::CacheUnavailable)?;
    prune_if_due(&mut cache_guard, now_unix_seconds);
    let key = SessionAffinityKey {
        provider: Provider::Claude,
        session_id: session_id.to_owned(),
    };
    if let Some(entry) = cache_guard.entries.get_mut(&key)
        && entry.pin_version == pin_version
        && entry.account_id == *account_id
    {
        entry.last_seen_unix_seconds = entry.last_seen_unix_seconds.max(now_unix_seconds);
        return Ok(());
    }
    if cache_guard
        .entries
        .get(&key)
        .is_some_and(|entry| entry.pin_version > pin_version)
    {
        return Ok(());
    }

    cache_guard.entries.insert(
        key,
        SessionAccountAffinityEntry {
            account_id: account_id.clone(),
            pin_version,
            last_seen_unix_seconds: now_unix_seconds,
            owner_token: Arc::new(()),
        },
    );
    Ok(())
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
#[path = "session_account_affinity_cache_tests.rs"]
mod tests;

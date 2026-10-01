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
    account_id: Option<AccountId>,
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
        if entry.account_id.as_ref() != Some(&self.account_id)
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
    Ok(selection_from_entry(
        cache, provider, session_id, route_band, writer, entry,
    ))
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
        return Ok(selection_from_entry(
            cache, provider, session_id, route_band, writer, live_entry,
        ));
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
            entry.account_id.as_ref() == Some(persisted_account_id)
                && entry.pin_version == persisted.pin_version()
        })
        .map_or_else(|| Arc::new(()), |entry| Arc::clone(&entry.owner_token));
    cache_guard.entries.insert(
        key,
        SessionAccountAffinityEntry {
            account_id: Some(persisted_account_id.clone()),
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
            account_id: Some(account_id.clone()),
            pin_version: 0,
            last_seen_unix_seconds: now_unix_seconds,
            owner_token: Arc::new(()),
        });
    if entry.account_id.as_ref() != Some(account_id) {
        entry.account_id = Some(account_id.clone());
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
    selection_from_entry(cache, provider, session_id, route_band, writer, entry)
        .ok_or(SessionAccountAffinityCacheUnavailable)
}

/// Reads Claude's versioned authority without renewing its idle clock.
pub(crate) async fn observe_claude_session_account_affinity<TRepository>(
    cache: &SharedSessionAccountAffinityCache,
    session_id: &str,
    repository: &TRepository,
    now_unix_seconds: u64,
) -> Result<PinObservation, SessionAccountAffinityPublicationError>
where
    TRepository: AsyncSessionAccountAffinityRepository + Sync,
{
    let persisted = repository
        .load_session_account_affinity(Provider::Claude, session_id)
        .await?;
    reconcile_claude_pin_observation(cache, session_id, persisted.as_ref(), now_unix_seconds)
}

/// A release winner and the fresh authority for the next attempt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ClaudePinRelease {
    pub(crate) observation: PinObservation,
    pub(crate) released: bool,
}

/// Releases only the observed active pin; a lost race re-reads current authority.
pub(crate) async fn release_claude_session_account_affinity<TReadRepository, TWriteRepository>(
    cache: &SharedSessionAccountAffinityCache,
    session_id: &str,
    observation: &PinObservation,
    read_repository: &TReadRepository,
    write_repository: &TWriteRepository,
    now_unix_seconds: u64,
) -> Result<ClaudePinRelease, SessionAccountAffinityPublicationError>
where
    TReadRepository: AsyncSessionAccountAffinityRepository + Sync,
    TWriteRepository: AsyncSessionAccountAffinityRepository + Sync + ?Sized,
{
    let next_version = observation.version().checked_add(1);
    if observation.active_account().is_some()
        && let Some(next_version) = next_version
    {
        let pin_ttl_seconds = cache
            .lock()
            .map_err(|_error| SessionAccountAffinityPublicationError::CacheUnavailable)?
            .idle_ttl
            .as_secs();
        let released_pin = SessionAccountAffinity::with_pin_state(
            Provider::Claude,
            session_id,
            None,
            next_version,
            now_unix_seconds,
        );
        if write_repository
            .compare_and_set_session_account_affinity(observation, &released_pin, pin_ttl_seconds)
            .await?
        {
            reconcile_claude_pin_observation(
                cache,
                session_id,
                Some(&released_pin),
                now_unix_seconds,
            )?;
            return Ok(ClaudePinRelease {
                observation: PinObservation::new(None, next_version),
                released: true,
            });
        }
    }
    Ok(ClaudePinRelease {
        observation: observe_claude_session_account_affinity(
            cache,
            session_id,
            read_repository,
            now_unix_seconds,
        )
        .await?,
        released: false,
    })
}

fn reconcile_claude_pin_observation(
    cache: &SharedSessionAccountAffinityCache,
    session_id: &str,
    persisted: Option<&SessionAccountAffinity>,
    now_unix_seconds: u64,
) -> Result<PinObservation, SessionAccountAffinityPublicationError> {
    let mut cache_guard = cache
        .lock()
        .map_err(|_error| SessionAccountAffinityPublicationError::CacheUnavailable)?;
    prune_if_due(&mut cache_guard, now_unix_seconds);
    let key = SessionAffinityKey {
        provider: Provider::Claude,
        session_id: session_id.to_owned(),
    };
    if let Some(persisted) = persisted {
        let live_entry = cache_guard.entries.get(&key);
        if !live_entry.is_some_and(|entry| {
            entry.pin_version > persisted.pin_version()
                || (entry.pin_version == persisted.pin_version()
                    && entry.last_seen_unix_seconds >= persisted.last_seen_unix_seconds())
        }) {
            let owner_token = live_entry
                .filter(|entry| {
                    entry.pin_version == persisted.pin_version()
                        && entry.account_id.as_ref() == persisted.account_id()
                })
                .map_or_else(|| Arc::new(()), |entry| Arc::clone(&entry.owner_token));
            cache_guard.entries.insert(
                key.clone(),
                SessionAccountAffinityEntry {
                    account_id: persisted.account_id().cloned(),
                    pin_version: persisted.pin_version(),
                    last_seen_unix_seconds: persisted.last_seen_unix_seconds(),
                    owner_token,
                },
            );
        }
    }
    let Some(entry) = cache_guard.entries.get(&key) else {
        return Ok(PinObservation::new(None, 0));
    };
    let active_account = (now_unix_seconds.saturating_sub(entry.last_seen_unix_seconds)
        < cache_guard.idle_ttl.as_secs())
    .then(|| entry.account_id.clone())
    .flatten();
    Ok(PinObservation::new(active_account, entry.pin_version))
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
    let successful_pin = SessionAccountAffinity::with_pin_state(
        Provider::Claude,
        session_id,
        Some(account_id.clone()),
        pin_version,
        now_unix_seconds,
    );
    reconcile_claude_pin_observation(cache, session_id, Some(&successful_pin), now_unix_seconds)
        .map(|_observation| ())
}

fn selection_from_entry(
    cache: &SharedSessionAccountAffinityCache,
    provider: Provider,
    session_id: &str,
    route_band: RouteBand,
    writer: Option<&DbWriteActor>,
    entry: &SessionAccountAffinityEntry,
) -> Option<SessionAccountAffinitySelection> {
    let account_id = entry.account_id.as_ref()?;
    Some(SessionAccountAffinitySelection {
        account_id: account_id.clone(),
        activity_handle: SessionAffinityActivityHandle {
            cache: Arc::clone(cache),
            provider,
            session_id: session_id.to_owned(),
            account_id: account_id.clone(),
            owner_token: Arc::clone(&entry.owner_token),
            route_band,
            writer: writer.cloned(),
        },
    })
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

#[cfg(test)]
mod pin_authority_tests {
    use super::*;
    use codex_router_state::sqlite::AsyncSqliteStateStore;

    async fn pin_store(temporary_directory: &tempfile::TempDir) -> AsyncSqliteStateStore {
        let database_path = temporary_directory.path().join("router.sqlite");
        AsyncSqliteStateStore::open(&database_path)
            .await
            .unwrap_or_else(|error| panic!("test pin store should open: {error}"))
    }

    fn account_id(value: &str) -> AccountId {
        AccountId::new(value).unwrap_or_else(|error| panic!("test account: {error}"))
    }

    #[tokio::test]
    async fn observation_preserves_stored_version_for_active_expired_released_and_missing_pins() {
        let temporary_directory = tempfile::tempdir()
            .unwrap_or_else(|error| panic!("pin observation temporary directory: {error}"));
        let store = pin_store(&temporary_directory).await;
        let cache = SessionAccountAffinityCache::shared(Duration::from_secs(10));
        let account = account_id("acct_observed");
        let pin = SessionAccountAffinity::with_pin_state(
            Provider::Claude,
            "session",
            Some(account.clone()),
            7,
            1_000,
        );
        store
            .upsert_session_account_affinity(&pin)
            .await
            .unwrap_or_else(|error| panic!("pin should persist: {error}"));
        for (now, expected_account) in [(1_009, Some(account)), (1_010, None), (1_100, None)] {
            assert_eq!(
                observe_claude_session_account_affinity(&cache, "session", &store, now)
                    .await
                    .unwrap_or_else(|error| panic!("pin should observe: {error}")),
                PinObservation::new(expected_account, 7)
            );
        }
        let released =
            SessionAccountAffinity::with_pin_state(Provider::Claude, "released", None, 8, 1_000);
        store
            .upsert_session_account_affinity(&released)
            .await
            .unwrap_or_else(|error| panic!("released pin should persist: {error}"));
        for (session, expected_version) in [("released", 8), ("missing", 0)] {
            assert_eq!(
                observe_claude_session_account_affinity(&cache, session, &store, 1_100)
                    .await
                    .unwrap_or_else(|error| panic!("pin should observe: {error}")),
                PinObservation::new(None, expected_version)
            );
        }
        assert_eq!(
            store
                .load_session_account_affinity(Provider::Claude, "session")
                .await
                .unwrap_or_else(|error| panic!("pin should load: {error}")),
            Some(pin)
        );
    }

    #[tokio::test]
    async fn concurrent_releases_have_one_winner_and_late_success_cannot_resurrect_the_pin() {
        let temporary_directory = tempfile::tempdir()
            .unwrap_or_else(|error| panic!("pin release temporary directory: {error}"));
        let store = pin_store(&temporary_directory).await;
        let cache = SessionAccountAffinityCache::shared(Duration::from_secs(10));
        let account = account_id("acct_released");
        store
            .upsert_session_account_affinity(&SessionAccountAffinity::with_pin_state(
                Provider::Claude,
                "session",
                Some(account.clone()),
                7,
                1_000,
            ))
            .await
            .unwrap_or_else(|error| panic!("pin should persist: {error}"));
        let observation = observe_claude_session_account_affinity(&cache, "session", &store, 1_001)
            .await
            .unwrap_or_else(|error| panic!("pin should observe: {error}"));

        let read_store = AsyncSqliteStateStore::open_read_only(store.database_path())
            .await
            .unwrap_or_else(|error| panic!("read-only pin pool: {error}"));

        let (first, second) = tokio::join!(
            release_claude_session_account_affinity(
                &cache,
                "session",
                &observation,
                &read_store,
                &store,
                1_002
            ),
            release_claude_session_account_affinity(
                &cache,
                "session",
                &observation,
                &read_store,
                &store,
                1_002
            ),
        );
        let first = first.unwrap_or_else(|error| panic!("first release: {error}"));
        let second = second.unwrap_or_else(|error| panic!("second release: {error}"));
        assert_ne!(
            first.released, second.released,
            "exactly one CAS release must win"
        );
        assert_eq!(first.observation, PinObservation::new(None, 8));
        assert_eq!(second.observation, PinObservation::new(None, 8));

        let (completion_sender, completion_receiver) = tokio::sync::oneshot::channel();
        completion_sender
            .send(ClaudeResponseCompletion::Success)
            .unwrap_or_else(|_| panic!("completion receiver should remain open"));
        assert!(
            !publish_claude_session_account_affinity_on_success(
                &cache,
                "session",
                &observation,
                &account,
                completion_receiver,
                &store,
                || 1_003,
            )
            .await
            .unwrap_or_else(|error| panic!("late publication: {error}"))
        );
        assert_eq!(
            store
                .load_session_account_affinity(Provider::Claude, "session")
                .await
                .unwrap_or_else(|error| panic!("released pin should load: {error}")),
            Some(SessionAccountAffinity::with_pin_state(
                Provider::Claude,
                "session",
                None,
                8,
                1_002
            ))
        );
        assert_eq!(
            observe_claude_session_account_affinity(&cache, "session", &store, 1_003)
                .await
                .unwrap_or_else(|error| panic!("released pin should observe: {error}")),
            PinObservation::new(None, 8)
        );
    }

    #[tokio::test]
    async fn losing_release_returns_the_new_active_owner_and_its_version() {
        let temporary_directory = tempfile::tempdir()
            .unwrap_or_else(|error| panic!("pin ownership temporary directory: {error}"));
        let store = pin_store(&temporary_directory).await;
        let cache = SessionAccountAffinityCache::shared(Duration::from_secs(10));
        let old_account = account_id("acct_old");
        let new_account = account_id("acct_new");
        let stale_observation = PinObservation::new(Some(old_account), 7);
        store
            .upsert_session_account_affinity(&SessionAccountAffinity::with_pin_state(
                Provider::Claude,
                "session",
                Some(new_account.clone()),
                9,
                1_000,
            ))
            .await
            .unwrap_or_else(|error| panic!("new owner should persist: {error}"));
        let release = release_claude_session_account_affinity(
            &cache,
            "session",
            &stale_observation,
            &store,
            &store,
            1_001,
        )
        .await
        .unwrap_or_else(|error| panic!("stale release: {error}"));
        assert!(!release.released);
        assert_eq!(
            release.observation,
            PinObservation::new(Some(new_account), 9)
        );
    }
}

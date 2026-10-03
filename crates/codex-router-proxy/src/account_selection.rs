//! Token-free account-selection boundary.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use codex_router_core::affinity::PreviousResponseId;
use codex_router_core::affinity::RouterAffinityHashSecret;
use codex_router_core::affinity::hash_previous_response_id;
use codex_router_core::ids::AccountId;
use codex_router_core::ids::TokenGeneration;
use codex_router_core::provider::Provider;
use codex_router_core::route_profile::CLAUDE_MESSAGES;
use codex_router_core::route_profile::ClaudeFiveHourReservePercent;
use codex_router_core::route_profile::DEFAULT_CLAUDE_FIVE_HOUR_RESERVE_PERCENT;
use codex_router_core::route_profile::RESPONSES_HTTP;
use codex_router_core::route_profile::RESPONSES_WEBSOCKET;
use codex_router_core::route_profile::RouteProfile;
use codex_router_core::routes::RouteBand;
use codex_router_quota::snapshot::SnapshotFreshness;
use codex_router_selection::burn_down::AccountAvailability;
use codex_router_selection::burn_down::BurnDownAccountAssessment;
use codex_router_selection::burn_down::BurnDownAccountInput;
use codex_router_selection::burn_down::BurnDownRouteBandAssessmentInput;
use codex_router_selection::burn_down::BurnDownRouteBandAssessmentResult;
use codex_router_selection::burn_down::CreditBackedEligibility;
use codex_router_selection::burn_down::QuotaEvidenceFreshness;
use codex_router_selection::burn_down::QuotaEvidenceReason;
use codex_router_selection::burn_down::QuotaWindowFact;
use codex_router_selection::burn_down::QuotaWindowStatus;
use codex_router_selection::burn_down::RoutingExclusion;
use codex_router_selection::burn_down::RoutingReason;
use codex_router_selection::burn_down::SelectedPool;
use codex_router_selection::burn_down::V1_SHORT_WINDOW_SECONDS;
use codex_router_selection::burn_down::V1_WEEKLY_WINDOW_SECONDS;
use codex_router_selection::burn_down::assess_route_band;
use codex_router_selection::reservation::ReservationBook;
use codex_router_selection::reservation::ReservationHandle;
use codex_router_selection::selection_outcome::SelectionOutcome;
use codex_router_selection::weighted_deficit::WeightedDeficitSelector;
#[cfg(test)]
use codex_router_state::account::AccountStatus;
use codex_router_state::affinity_owner::PreviousResponseAffinityOwnerLookup;
#[cfg(test)]
use codex_router_state::quota_snapshot::PersistedSelectorQuotaWindow;
#[cfg(test)]
use codex_router_state::quota_snapshot::SelectorQuotaInput;
#[cfg(test)]
use codex_router_state::quota_snapshot::SelectorQuotaWindowStatus;
#[cfg(test)]
use codex_router_state::repositories::AffinityRepository;
#[cfg(test)]
use codex_router_state::repositories::SelectorQuotaRepository;
use codex_router_state::selection_projection::AsyncSelectionProjectionRepository;
use codex_router_state::selection_projection::project_route_band_selection_inputs_with_active_counts_read_only;
use codex_router_state::session_account_affinity::PinObservation;
use codex_router_state::sqlite::AsyncAffinityRepository;
use codex_router_state::sqlite::AsyncSessionAccountAffinityRepository;
use codex_router_state::sqlite::StateStoreError;
use futures_util::future::BoxFuture;
use sha2::Digest;
use sha2::Sha256;
use thiserror::Error;
use tokio::sync::Mutex as AsyncMutex;

use crate::db_write_actor::DbWriteActor;
use crate::db_write_actor::DbWriteCommand;
use crate::http_sse::HttpProxyError;
use crate::http_sse::HttpProxyRequest;
use crate::routes::RouteClass;
use crate::routes::RouteKind;
use crate::routes::classify_route;
use crate::session_account_affinity_cache::DEFAULT_SESSION_PIN_IDLE_TTL;
use crate::session_account_affinity_cache::SessionAccountAffinityCache;
use crate::session_account_affinity_cache::SessionAffinityActivityHandle;
use crate::session_account_affinity_cache::SharedSessionAccountAffinityCache;
use crate::session_account_affinity_cache::lookup_session_account_affinity;
use crate::session_account_affinity_cache::observe_claude_session_account_affinity;
use crate::session_account_affinity_cache::publish_session_account_affinity;
use crate::session_account_affinity_cache::reconcile_persisted_session_account_affinity;
use crate::session_account_affinity_cache::release_claude_session_account_affinity;

#[path = "account_selection/account_admission.rs"]
mod account_admission;
pub(crate) use account_admission::AccountSourceAdmission;
pub(crate) use account_admission::FloorSwitchPeerAssessment;
pub(crate) use account_admission::LiveAccountAdmissionAssessor;
pub(crate) use account_admission::RuntimeAccountAdmissionAssessor;
mod active_reservations;
mod affinity_admission;
mod assessment_selection;
#[cfg(test)]
mod fixture_selection;
mod post_exhaustion;
mod repository_selection;
mod request_metadata;
mod runtime_quarantine;
mod short_quota_wait;

pub use active_reservations::{
    ActiveClientLeaseReporter, SqliteActiveClientLeaseReporter, release_account_reservation,
};
#[cfg(test)]
pub use fixture_selection::RepositoryBackedAccountSelector;
pub use post_exhaustion::{
    RouteBandPostExhaustionOutcomeInput, route_band_post_exhaustion_outcome,
};
pub use runtime_quarantine::{
    clear_route_band_queue_degraded, mark_route_band_queue_degraded, mark_runtime_quota_exhausted,
};
pub(crate) use runtime_quarantine::{
    clear_route_band_queue_degraded_for_queue, mark_route_band_queue_degraded_for_queue,
    route_band_queue_health_allows_selection, route_band_queue_health_key_prefix,
};

use active_reservations::{
    ActiveReservationGuardInner, active_session_counts_by_account, reserve_selected_account,
    telemetry_hash,
};
use affinity_admission::{
    account_id_from_affinity_owner_lookup, assessment_account_is_available,
    assessment_account_must_yield, publish_selected_session_affinity, select_affinity_owner,
    session_affinity_lookup_session_id, session_id_for_route,
};
use assessment_selection::{
    assessment_has_unavailable_quota_authority, empty_assessment_selection_error,
    filter_selector_accounts_for_provider, projected_accounts_excluding_attempted,
    select_from_burn_down_assessment,
};
use request_metadata::{
    previous_response_id, route_kind_for_request, route_profile_for_kind,
    transport_label_for_request,
};
use runtime_quarantine::projected_accounts_excluding_runtime_exhaustions;
use short_quota_wait::{
    exhausted_account_short_quota_wait_delay_seconds, short_quota_wait_delay_seconds,
    short_quota_wait_delay_seconds_with_jitter, short_quota_wait_jitter_seconds,
};

/// Process-lifetime weighted state partitioned by route band.
pub type RouteBandWeightedSelectors = Arc<Mutex<HashMap<String, WeightedDeficitSelector>>>;

/// Process-lifetime account-hold state partitioned by provider and route band.
pub type RouteBandAccountHolds = Arc<Mutex<HashMap<ProviderRouteBand, AccountHold>>>;

/// Process-lifetime active reservation state partitioned by route band.
pub type RouteBandReservationBooks = Arc<Mutex<HashMap<String, ReservationBook>>>;

/// Process-lifetime runtime quota exhaustion state partitioned by route band.
pub type RouteBandRuntimeExhaustions = Arc<Mutex<HashMap<String, Vec<RuntimeQuotaExhaustion>>>>;

/// Process-lifetime route-band queue health state partitioned by route band.
pub type RouteBandQueueHealth = Arc<Mutex<HashMap<String, RouteBandQueueDegradedState>>>;

/// Async critical section for active-count projection and reservation.
pub(crate) type SelectionReservationLock = Arc<AsyncMutex<()>>;

/// Process-local cooldown scope for one provider's route band.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ProviderRouteBand {
    provider: Provider,
    route_band: RouteBand,
}

impl ProviderRouteBand {
    /// Creates a cooldown scope for a provider route.
    #[must_use]
    pub const fn new(provider: Provider, route_band: RouteBand) -> Self {
        Self {
            provider,
            route_band,
        }
    }

    /// Returns the provider scoped by this cooldown key.
    #[must_use]
    pub const fn provider(self) -> Provider {
        self.provider
    }

    /// Returns the route band scoped by this cooldown key.
    #[must_use]
    pub const fn route_band(self) -> RouteBand {
        self.route_band
    }
}

/// Process-local dependencies shared by async account selectors.
#[derive(Clone)]
pub struct AsyncAccountSelectorRuntimeState {
    weighted_selectors: RouteBandWeightedSelectors,
    account_holds: RouteBandAccountHolds,
    active_reservations: RouteBandReservationBooks,
    runtime_exhaustions: RouteBandRuntimeExhaustions,
    route_band_queue_health: RouteBandQueueHealth,
    selection_reservation_lock: SelectionReservationLock,
    session_affinity_cache: SharedSessionAccountAffinityCache,
}

impl AsyncAccountSelectorRuntimeState {
    /// Creates selector runtime dependencies with a fresh selection lock.
    #[must_use]
    pub fn new(
        weighted_selectors: RouteBandWeightedSelectors,
        account_holds: RouteBandAccountHolds,
        active_reservations: RouteBandReservationBooks,
        runtime_exhaustions: RouteBandRuntimeExhaustions,
        route_band_queue_health: RouteBandQueueHealth,
    ) -> Self {
        Self::new_with_selection_lock(
            weighted_selectors,
            account_holds,
            active_reservations,
            runtime_exhaustions,
            route_band_queue_health,
            Arc::new(AsyncMutex::new(())),
        )
    }

    /// Creates selector runtime dependencies with a shared selection lock.
    #[must_use]
    pub(crate) fn new_with_selection_lock(
        weighted_selectors: RouteBandWeightedSelectors,
        account_holds: RouteBandAccountHolds,
        active_reservations: RouteBandReservationBooks,
        runtime_exhaustions: RouteBandRuntimeExhaustions,
        route_band_queue_health: RouteBandQueueHealth,
        selection_reservation_lock: SelectionReservationLock,
    ) -> Self {
        Self {
            weighted_selectors,
            account_holds,
            active_reservations,
            runtime_exhaustions,
            route_band_queue_health,
            selection_reservation_lock,
            session_affinity_cache: SessionAccountAffinityCache::shared(
                DEFAULT_SESSION_PIN_IDLE_TTL,
            ),
        }
    }

    /// Creates runtime dependencies with a caller-owned shared affinity cache.
    #[must_use]
    pub(crate) fn new_with_selection_lock_and_affinity_cache(
        weighted_selectors: RouteBandWeightedSelectors,
        account_holds: RouteBandAccountHolds,
        active_reservations: RouteBandReservationBooks,
        runtime_exhaustions: RouteBandRuntimeExhaustions,
        route_band_queue_health: RouteBandQueueHealth,
        selection_reservation_lock: SelectionReservationLock,
        session_affinity_cache: SharedSessionAccountAffinityCache,
    ) -> Self {
        Self {
            weighted_selectors,
            account_holds,
            active_reservations,
            runtime_exhaustions,
            route_band_queue_health,
            selection_reservation_lock,
            session_affinity_cache,
        }
    }

    #[cfg(test)]
    pub(crate) fn selection_reservation_lock_for_test(&self) -> SelectionReservationLock {
        Arc::clone(&self.selection_reservation_lock)
    }
}

/// Default v1 minimum account reuse period for adjacent normal requests.
pub const DEFAULT_ACCOUNT_HOLD_COOLDOWN_SECONDS: u64 = 120;

/// Idle time after which a Codex session may move to another account.
pub const PROMPT_CACHE_ACCOUNT_AFFINITY_IDLE_TTL_SECONDS: u64 =
    DEFAULT_SESSION_PIN_IDLE_TTL.as_secs();

const ACTIVE_SESSION_RESERVATION_UNITS: u32 = 1;

const ACTIVE_RESERVATION_MAX_AGE_SECONDS: u64 = 7_200;

type UnixClock = Arc<dyn Fn() -> u64 + Send + Sync>;

/// RAII guard that releases active-load accounting when the stream lifecycle ends.
#[derive(Clone)]
pub struct ActiveReservationGuard {
    inner: Arc<ActiveReservationGuardInner>,
}

/// Process-local account hold for one provider route band.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccountHold {
    account_id: AccountId,
    selected_unix_seconds: u64,
}

impl AccountHold {
    fn new(account_id: AccountId, selected_unix_seconds: u64) -> Self {
        Self {
            account_id,
            selected_unix_seconds,
        }
    }
}

/// Runtime-only account exclusion created when a live socket sees quota exhaustion.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeQuotaExhaustion {
    account_id: AccountId,
    expires_unix_seconds: u64,
}

/// Low-cardinality route-band queue degraded reason.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RouteBandQueueDegradedReason {
    /// DB write queue had no capacity.
    DbWriteQueueFull,
    /// DB write queue was closed.
    DbWriteQueueClosed,
    /// DB write actor accepted a command but durable storage failed.
    DbWriteFailed,
}

/// Route-band degraded state for selection admission.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RouteBandQueueDegradedState {
    reason: RouteBandQueueDegradedReason,
    observed_unix_seconds: u64,
}

/// Selected account material needed by the proxy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SelectedAccountDecision {
    account_id: AccountId,
    selection_reason: String,
    /// Private in-memory source context for next-turn credit eligibility checks.
    credit_backed_at_selection: bool,
    active_reservation_guard: Option<ActiveReservationGuard>,
    session_affinity_activity_handle: Option<SessionAffinityActivityHandle>,
    pin_observation: Option<PinObservation>,
}

impl SelectedAccountDecision {
    /// Creates selected account material.
    #[must_use]
    pub fn new(account_id: AccountId, selection_reason: impl Into<String>) -> Self {
        Self {
            account_id,
            selection_reason: selection_reason.into(),
            credit_backed_at_selection: false,
            active_reservation_guard: None,
            session_affinity_activity_handle: None,
            pin_observation: None,
        }
    }

    /// Attaches an active-load reservation handle.
    #[must_use]
    pub fn with_active_reservation_guard(
        mut self,
        active_reservation_guard: ActiveReservationGuard,
    ) -> Self {
        self.active_reservation_guard = Some(active_reservation_guard);
        self
    }

    /// Returns selected account id.
    #[must_use]
    pub const fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    /// Returns a redacted static/audit-safe selection reason.
    #[must_use]
    pub fn selection_reason(&self) -> &str {
        &self.selection_reason
    }

    pub(crate) const fn credit_backed_at_selection(&self) -> bool {
        self.credit_backed_at_selection
    }

    pub(crate) const fn with_credit_backed_at_selection(
        mut self,
        credit_backed_at_selection: bool,
    ) -> Self {
        self.credit_backed_at_selection = credit_backed_at_selection;
        self
    }

    /// Returns the active-load reservation handle, if one was created.
    #[must_use]
    pub fn reservation_handle(&self) -> Option<&ReservationHandle> {
        match &self.active_reservation_guard {
            Some(active_reservation_guard) => Some(active_reservation_guard.reservation_handle()),
            None => None,
        }
    }

    /// Returns the active reservation guard, if one was created.
    #[must_use]
    pub const fn active_reservation_guard(&self) -> Option<&ActiveReservationGuard> {
        self.active_reservation_guard.as_ref()
    }

    /// Attaches the token-bound handle for successful established request activity.
    #[must_use]
    pub fn with_session_affinity_activity_handle(
        mut self,
        session_affinity_activity_handle: SessionAffinityActivityHandle,
    ) -> Self {
        self.session_affinity_activity_handle = Some(session_affinity_activity_handle);
        self
    }

    /// Returns the token-bound session-affinity activity handle.
    #[must_use]
    pub const fn session_affinity_activity_handle(&self) -> Option<&SessionAffinityActivityHandle> {
        self.session_affinity_activity_handle.as_ref()
    }

    /// Carries the admission or release authority through Claude's attempt lifecycle.
    #[must_use]
    pub(crate) fn with_pin_observation(mut self, observation: Option<PinObservation>) -> Self {
        self.pin_observation = observation;
        self
    }

    /// Returns the one versioned observation that may publish this Claude attempt.
    #[must_use]
    pub(crate) const fn pin_observation(&self) -> Option<&PinObservation> {
        self.pin_observation.as_ref()
    }
}

/// Selects an upstream account after local auth succeeds.
pub trait AccountDecisionSelector {
    /// Selects account material for one request.
    fn select_upstream_account(
        &self,
        request: &HttpProxyRequest,
        token_generation: TokenGeneration,
        affinity_secret: Option<&RouterAffinityHashSecret>,
    ) -> Result<SelectedAccountDecision, HttpProxyError>;
}

/// Async account selector boundary for Tokio proxy runtime callers.
pub trait AsyncAccountDecisionSelector {
    /// Selects account material for one request without blocking the async runtime.
    fn select_upstream_account<'a>(
        &'a self,
        request: &'a HttpProxyRequest,
        token_generation: TokenGeneration,
        affinity_secret: Option<&'a RouterAffinityHashSecret>,
    ) -> BoxFuture<'a, Result<SelectedAccountDecision, HttpProxyError>>;
}

/// Account state consumed by the quota-aware proxy selector adapter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QuotaAwareAccountState {
    account_id: AccountId,
    remaining_headroom: u32,
    freshness: SnapshotFreshness,
}

/// Selection adapter failure.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum QuotaAwareAccountSelectorError {
    /// No account has usable headroom.
    #[error("no eligible accounts")]
    NoEligibleAccounts,
    /// All accounts have exhausted only their short quota window until the supplied delay elapses.
    #[error("short quota exhausted; retry after {retry_after_seconds} seconds")]
    ShortQuotaExhausted {
        /// Conservative delay until the earliest verified short-window reset.
        retry_after_seconds: u64,
    },
    /// Weighted selector state was unavailable.
    #[error("selector state unavailable")]
    SelectorStateUnavailable,
    /// State repository could not be read.
    #[error("state repository unavailable")]
    StateUnavailable,
    /// Secret store could not be read.
    #[error("secret store unavailable")]
    SecretUnavailable,
    /// Previous-response affinity key was malformed.
    #[error("malformed affinity key")]
    MalformedAffinityKey,
    /// Previous-response affinity owner was missing.
    #[error("affinity owner missing")]
    AffinityOwnerMissing,
    /// Previous-response affinity owner is not currently routable.
    #[error("affinity owner unavailable")]
    AffinityOwnerUnavailable,
}

/// Safe post-exhaustion routing outcome for one Responses route band.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PostExhaustionRouteBandOutcome {
    /// A fresh alternative account can receive the replay immediately.
    SelectableAlternative,
    /// Every fresh alternative is short-window exhausted until this delay elapses.
    ShortQuotaWait {
        /// Conservative delay until the earliest verified short-window reset.
        retry_after_seconds: u64,
    },
    /// No safe alternative exists and retrying at a short reset is not proven safe.
    NoSelectableAlternative,
}

/// Account selector adapter using quota freshness and weighted deficit state.
#[derive(Debug)]
pub struct QuotaAwareAccountSelector {
    accounts: Vec<QuotaAwareAccountState>,
    weighted_selector: Mutex<WeightedDeficitSelector>,
}

/// Async selector that hydrates account state from repositories at request time.
pub struct AsyncRepositoryBackedAccountSelector<'a, R>
where
    R: AsyncAffinityRepository
        + AsyncSessionAccountAffinityRepository
        + AsyncSelectionProjectionRepository
        + Sync,
{
    state_repository: &'a R,
    claude_affinity_writer: &'a (dyn AsyncSessionAccountAffinityRepository + Sync),
    weighted_selectors: RouteBandWeightedSelectors,
    account_holds: RouteBandAccountHolds,
    active_reservations: RouteBandReservationBooks,
    runtime_exhaustions: RouteBandRuntimeExhaustions,
    route_band_queue_health: RouteBandQueueHealth,
    active_client_leases: Option<Arc<dyn ActiveClientLeaseReporter>>,
    session_affinity_writer: Option<DbWriteActor>,
    session_affinity_cache: SharedSessionAccountAffinityCache,
    selection_reservation_lock: SelectionReservationLock,
    minimum_account_hold_cooldown_seconds: u64,
    clock: UnixClock,
    claude_five_hour_reserve_percent: ClaudeFiveHourReservePercent,
}

fn active_reservation_book_for_route_band(
    active_reservations: &RouteBandReservationBooks,
    route_band: &str,
    now_unix_seconds: u64,
) -> Result<Option<ReservationBook>, HttpProxyError> {
    let mut active_reservations =
        active_reservations
            .lock()
            .map_err(|_error| HttpProxyError::Selection {
                reason: QuotaAwareAccountSelectorError::SelectorStateUnavailable,
            })?;
    if let Some(book) = active_reservations.get_mut(route_band) {
        book.purge_stale(now_unix_seconds, ACTIVE_RESERVATION_MAX_AGE_SECONDS);
    }
    Ok(active_reservations.get(route_band).cloned())
}

fn current_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

#[cfg(test)]
#[path = "account_selection/account_selection_tests.rs"]
mod tests;

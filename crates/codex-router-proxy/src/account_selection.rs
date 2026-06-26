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
use codex_router_core::routes::RouteBand;
use codex_router_quota::snapshot::SnapshotFreshness;
use codex_router_selection::burn_down::AccountAvailability;
use codex_router_selection::burn_down::BurnDownAccountAssessment;
use codex_router_selection::burn_down::BurnDownAccountInput;
use codex_router_selection::burn_down::BurnDownRouteBandAssessmentInput;
use codex_router_selection::burn_down::BurnDownRouteBandAssessmentResult;
use codex_router_selection::burn_down::QuotaWindowFact;
use codex_router_selection::burn_down::QuotaWindowStatus;
use codex_router_selection::burn_down::SelectedPool;
use codex_router_selection::burn_down::V1_SHORT_WINDOW_SECONDS;
use codex_router_selection::burn_down::V1_WEEKLY_WINDOW_SECONDS;
use codex_router_selection::burn_down::assess_route_band;
use codex_router_selection::reservation::ReservationBook;
use codex_router_selection::reservation::ReservationHandle;
use codex_router_selection::run_rate::QuotaRunRateConfidence;
use codex_router_selection::run_rate::QuotaRunRateEstimator;
use codex_router_selection::run_rate::QuotaRunRateObservation;
use codex_router_selection::weighted_deficit::WeightedDeficitSelector;
use codex_router_state::account::AccountStatus;
use codex_router_state::affinity_owner::PreviousResponseAffinityOwnerLookup;
use codex_router_state::quota_snapshot::PersistedSelectorQuotaWindow;
use codex_router_state::quota_snapshot::SelectorQuotaInput;
use codex_router_state::quota_snapshot::SelectorQuotaWindowStatus;
use codex_router_state::repositories::AffinityRepository;
use codex_router_state::repositories::SelectorQuotaRepository;
use codex_router_state::sqlite::AsyncAffinityRepository;
use codex_router_state::sqlite::AsyncQuotaHistoryRepository;
use codex_router_state::sqlite::AsyncSelectorQuotaRepository;
use codex_router_state::sqlite::AsyncSqliteStateStore;
use codex_router_state::sqlite::StateStoreError;
use futures_util::future::BoxFuture;
use thiserror::Error;
use tokio_util::task::TaskTracker;

use crate::http_sse::HttpProxyError;
use crate::http_sse::HttpProxyRequest;
use crate::routes::RouteClass;
use crate::routes::classify_route;

/// Process-lifetime weighted state partitioned by route band.
pub type RouteBandWeightedSelectors = Arc<Mutex<HashMap<String, WeightedDeficitSelector>>>;
/// Process-lifetime account-hold state partitioned by route band.
pub type RouteBandAccountHolds = Arc<Mutex<HashMap<String, AccountHold>>>;
/// Process-lifetime active reservation state partitioned by route band.
pub type RouteBandReservationBooks = Arc<Mutex<HashMap<String, ReservationBook>>>;

const ROUTING_METADATA_SCAN_LIMIT_BYTES: usize = 64 * 1024;
const ROUTING_METADATA_SCAN_MAX_TOP_LEVEL_KEYS: usize = 64;

/// Default v1 minimum account reuse period for adjacent normal requests.
pub const DEFAULT_ACCOUNT_HOLD_COOLDOWN_SECONDS: u64 = 120;
const HTTP_ACTIVE_LOAD_PRESSURE: u32 = 2;
const WEBSOCKET_ACTIVE_LOAD_PRESSURE: u32 = 8;
const ACTIVE_RESERVATION_MAX_AGE_SECONDS: u64 = 7_200;
const QUOTA_HISTORY_LOOKBACK_SECONDS: u64 = 604_800;
const QUOTA_HISTORY_FRESHNESS_SECONDS: u64 = 300;

type UnixClock = Arc<dyn Fn() -> u64 + Send + Sync>;

/// Mirrors active client leases into a process-external status source.
pub trait ActiveClientLeaseReporter: Send + Sync {
    /// Records one acquired client lease.
    fn record_acquired(
        &self,
        route_band: &str,
        reservation_handle: &ReservationHandle,
        acquired_unix_seconds: u64,
    );

    /// Records one released client lease.
    fn record_released(&self, route_band: &str, reservation_handle: &ReservationHandle);
}

/// SQLx-backed active client lease reporter.
#[derive(Clone, Debug)]
pub struct SqliteActiveClientLeaseReporter {
    state: AsyncSqliteStateStore,
    tasks: TaskTracker,
}

impl SqliteActiveClientLeaseReporter {
    /// Creates a SQLx-backed active client lease reporter.
    #[must_use]
    pub const fn new(state: AsyncSqliteStateStore, tasks: TaskTracker) -> Self {
        Self { state, tasks }
    }
}

impl ActiveClientLeaseReporter for SqliteActiveClientLeaseReporter {
    fn record_acquired(
        &self,
        route_band: &str,
        reservation_handle: &ReservationHandle,
        acquired_unix_seconds: u64,
    ) {
        let state = self.state.clone();
        let route_band = route_band.to_owned();
        let reservation_id = reservation_handle.reservation_id().clone();
        let account_id = reservation_handle.account_id().clone();
        self.tasks.spawn(async move {
            let _result = state
                .record_active_client_acquired(
                    &route_band,
                    &reservation_id,
                    &account_id,
                    acquired_unix_seconds,
                )
                .await;
        });
    }

    fn record_released(&self, route_band: &str, reservation_handle: &ReservationHandle) {
        let state = self.state.clone();
        let route_band = route_band.to_owned();
        let reservation_id = reservation_handle.reservation_id().clone();
        self.tasks.spawn(async move {
            let _result = state
                .record_active_client_released(&route_band, &reservation_id)
                .await;
        });
    }
}

/// RAII guard that releases active-load accounting when the stream lifecycle ends.
#[derive(Clone)]
pub struct ActiveReservationGuard {
    inner: Arc<ActiveReservationGuardInner>,
}

impl ActiveReservationGuard {
    pub(crate) fn new(
        active_reservations: RouteBandReservationBooks,
        route_band: String,
        reservation_handle: ReservationHandle,
    ) -> Self {
        Self::new_with_active_client_leases(
            active_reservations,
            route_band,
            reservation_handle,
            None,
        )
    }

    pub(crate) fn new_with_active_client_leases(
        active_reservations: RouteBandReservationBooks,
        route_band: String,
        reservation_handle: ReservationHandle,
        active_client_leases: Option<Arc<dyn ActiveClientLeaseReporter>>,
    ) -> Self {
        Self {
            inner: Arc::new(ActiveReservationGuardInner {
                active_reservations,
                route_band,
                reservation_handle,
                active_client_leases,
                released: AtomicBool::new(false),
            }),
        }
    }

    /// Returns the reservation handle.
    #[must_use]
    pub fn reservation_handle(&self) -> &ReservationHandle {
        &self.inner.reservation_handle
    }

    /// Releases the reservation before the stream object itself closes.
    pub fn release(&self) {
        self.inner.release_once();
    }

    /// Reserves the same account/route/cost again after a completed turn.
    pub fn reserve_again_at(&self, reserved_unix_seconds: u64) -> Option<Self> {
        let mut active_reservations = self.inner.active_reservations.lock().ok()?;
        let reservation_handle = active_reservations
            .entry(self.inner.route_band.clone())
            .or_insert_with(ReservationBook::default)
            .reserve_next_at(
                self.inner.reservation_handle.account_id().clone(),
                self.inner.reservation_handle.headroom_cost(),
                reserved_unix_seconds,
            );
        Some(Self::new(
            Arc::clone(&self.inner.active_reservations),
            self.inner.route_band.clone(),
            reservation_handle,
        ))
    }
}

impl std::fmt::Debug for ActiveReservationGuard {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ActiveReservationGuard")
            .field("route_band", &self.inner.route_band)
            .field("reservation_handle", &self.inner.reservation_handle)
            .finish()
    }
}

impl PartialEq for ActiveReservationGuard {
    fn eq(&self, other: &Self) -> bool {
        self.inner.route_band == other.inner.route_band
            && self.inner.reservation_handle == other.inner.reservation_handle
    }
}

impl Eq for ActiveReservationGuard {}

struct ActiveReservationGuardInner {
    active_reservations: RouteBandReservationBooks,
    route_band: String,
    reservation_handle: ReservationHandle,
    active_client_leases: Option<Arc<dyn ActiveClientLeaseReporter>>,
    released: AtomicBool,
}

impl ActiveReservationGuardInner {
    fn release_once(&self) {
        if self
            .released
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            release_account_reservation(
                &self.active_reservations,
                &self.route_band,
                &self.reservation_handle,
            );
            if let Some(active_client_leases) = self.active_client_leases.as_ref() {
                active_client_leases.record_released(&self.route_band, &self.reservation_handle);
            }
        }
    }
}

impl Drop for ActiveReservationGuardInner {
    fn drop(&mut self) {
        self.release_once();
    }
}

/// Process-local account hold for one route band.
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

/// Selected account material needed by the proxy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SelectedAccountDecision {
    account_id: AccountId,
    selection_reason: String,
    active_reservation_guard: Option<ActiveReservationGuard>,
}

impl SelectedAccountDecision {
    /// Creates selected account material.
    #[must_use]
    pub fn new(account_id: AccountId, selection_reason: impl Into<String>) -> Self {
        Self {
            account_id,
            selection_reason: selection_reason.into(),
            active_reservation_guard: None,
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

impl QuotaAwareAccountState {
    /// Creates account state for selector input.
    #[must_use]
    pub const fn new(
        account_id: AccountId,
        remaining_headroom: u32,
        freshness: SnapshotFreshness,
    ) -> Self {
        Self {
            account_id,
            remaining_headroom,
            freshness,
        }
    }
}

/// Selection adapter failure.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum QuotaAwareAccountSelectorError {
    /// No account has usable headroom.
    #[error("no eligible accounts")]
    NoEligibleAccounts,
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

/// Account selector adapter using quota freshness and weighted deficit state.
#[derive(Debug)]
pub struct QuotaAwareAccountSelector {
    accounts: Vec<QuotaAwareAccountState>,
    weighted_selector: Mutex<WeightedDeficitSelector>,
}

impl QuotaAwareAccountSelector {
    /// Creates a quota-aware selector from account snapshots.
    #[must_use]
    pub fn new(accounts: Vec<QuotaAwareAccountState>) -> Self {
        Self {
            accounts,
            weighted_selector: Mutex::new(WeightedDeficitSelector::default()),
        }
    }
}

impl AccountDecisionSelector for QuotaAwareAccountSelector {
    fn select_upstream_account(
        &self,
        _request: &HttpProxyRequest,
        _token_generation: TokenGeneration,
        _affinity_secret: Option<&RouterAffinityHashSecret>,
    ) -> Result<SelectedAccountDecision, HttpProxyError> {
        select_from_account_states(&self.accounts, &self.weighted_selector)
    }
}

/// Selector that hydrates account state from repositories at request time.
pub struct RepositoryBackedAccountSelector<'a, R>
where
    R: AffinityRepository + SelectorQuotaRepository,
{
    state_repository: &'a R,
    weighted_selectors: RouteBandWeightedSelectors,
    account_holds: RouteBandAccountHolds,
    minimum_account_hold_cooldown_seconds: u64,
    clock: UnixClock,
}

/// Async selector that hydrates account state from repositories at request time.
pub struct AsyncRepositoryBackedAccountSelector<'a, R>
where
    R: AsyncAffinityRepository + AsyncSelectorQuotaRepository + AsyncQuotaHistoryRepository + Sync,
{
    state_repository: &'a R,
    weighted_selectors: RouteBandWeightedSelectors,
    account_holds: RouteBandAccountHolds,
    active_reservations: RouteBandReservationBooks,
    active_client_leases: Option<Arc<dyn ActiveClientLeaseReporter>>,
    minimum_account_hold_cooldown_seconds: u64,
    clock: UnixClock,
}

impl<'a, R> RepositoryBackedAccountSelector<'a, R>
where
    R: AffinityRepository + SelectorQuotaRepository,
{
    /// Creates a repository-backed selector.
    #[must_use]
    pub fn new(state_repository: &'a R) -> Self {
        Self {
            state_repository,
            weighted_selectors: Arc::new(Mutex::new(HashMap::new())),
            account_holds: Arc::new(Mutex::new(HashMap::new())),
            minimum_account_hold_cooldown_seconds: DEFAULT_ACCOUNT_HOLD_COOLDOWN_SECONDS,
            clock: Arc::new(current_unix_seconds),
        }
    }

    /// Creates a repository-backed selector with process-lifetime weighted state.
    #[must_use]
    pub fn new_with_weighted_selector(
        state_repository: &'a R,
        weighted_selectors: RouteBandWeightedSelectors,
        account_holds: RouteBandAccountHolds,
    ) -> Self {
        Self {
            state_repository,
            weighted_selectors,
            account_holds,
            minimum_account_hold_cooldown_seconds: DEFAULT_ACCOUNT_HOLD_COOLDOWN_SECONDS,
            clock: Arc::new(current_unix_seconds),
        }
    }

    /// Creates a repository-backed selector with process-lifetime runtime state.
    #[must_use]
    pub fn new_with_runtime(
        state_repository: &'a R,
        weighted_selectors: RouteBandWeightedSelectors,
        account_holds: RouteBandAccountHolds,
        minimum_account_hold_cooldown_seconds: u64,
        clock: UnixClock,
    ) -> Self {
        Self {
            state_repository,
            weighted_selectors,
            account_holds,
            minimum_account_hold_cooldown_seconds,
            clock,
        }
    }
}

impl<'a, R> AsyncRepositoryBackedAccountSelector<'a, R>
where
    R: AsyncAffinityRepository + AsyncSelectorQuotaRepository + AsyncQuotaHistoryRepository + Sync,
{
    /// Creates an async repository-backed selector.
    #[must_use]
    pub fn new(state_repository: &'a R) -> Self {
        Self {
            state_repository,
            weighted_selectors: Arc::new(Mutex::new(HashMap::new())),
            account_holds: Arc::new(Mutex::new(HashMap::new())),
            active_reservations: Arc::new(Mutex::new(HashMap::new())),
            active_client_leases: None,
            minimum_account_hold_cooldown_seconds: DEFAULT_ACCOUNT_HOLD_COOLDOWN_SECONDS,
            clock: Arc::new(current_unix_seconds),
        }
    }

    /// Creates an async repository-backed selector with process-lifetime weighted state.
    #[must_use]
    pub fn new_with_weighted_selector(
        state_repository: &'a R,
        weighted_selectors: RouteBandWeightedSelectors,
        account_holds: RouteBandAccountHolds,
    ) -> Self {
        Self {
            state_repository,
            weighted_selectors,
            account_holds,
            active_reservations: Arc::new(Mutex::new(HashMap::new())),
            active_client_leases: None,
            minimum_account_hold_cooldown_seconds: DEFAULT_ACCOUNT_HOLD_COOLDOWN_SECONDS,
            clock: Arc::new(current_unix_seconds),
        }
    }

    /// Creates an async repository-backed selector with process-lifetime runtime state.
    #[must_use]
    pub fn new_with_runtime(
        state_repository: &'a R,
        weighted_selectors: RouteBandWeightedSelectors,
        account_holds: RouteBandAccountHolds,
        minimum_account_hold_cooldown_seconds: u64,
        clock: UnixClock,
    ) -> Self {
        Self {
            state_repository,
            weighted_selectors,
            account_holds,
            active_reservations: Arc::new(Mutex::new(HashMap::new())),
            active_client_leases: None,
            minimum_account_hold_cooldown_seconds,
            clock,
        }
    }

    /// Creates an async selector with explicit active reservation state.
    #[must_use]
    pub fn new_with_runtime_and_reservations(
        state_repository: &'a R,
        weighted_selectors: RouteBandWeightedSelectors,
        account_holds: RouteBandAccountHolds,
        active_reservations: RouteBandReservationBooks,
        minimum_account_hold_cooldown_seconds: u64,
        clock: UnixClock,
    ) -> Self {
        Self {
            state_repository,
            weighted_selectors,
            account_holds,
            active_reservations,
            active_client_leases: None,
            minimum_account_hold_cooldown_seconds,
            clock,
        }
    }

    /// Adds process-external active client lease reporting.
    #[must_use]
    pub fn with_active_client_lease_reporter(
        mut self,
        active_client_leases: Arc<dyn ActiveClientLeaseReporter>,
    ) -> Self {
        self.active_client_leases = Some(active_client_leases);
        self
    }
}

impl<R> AccountDecisionSelector for RepositoryBackedAccountSelector<'_, R>
where
    R: AffinityRepository + SelectorQuotaRepository,
{
    fn select_upstream_account(
        &self,
        request: &HttpProxyRequest,
        _token_generation: TokenGeneration,
        affinity_secret: Option<&RouterAffinityHashSecret>,
    ) -> Result<SelectedAccountDecision, HttpProxyError> {
        let route_kind = route_kind_for_request(request)?;
        let route_band = route_kind.route_band();
        let now_unix_seconds = (self.clock)();
        let selector_inputs = self
            .state_repository
            .selector_inputs_for_route_band(route_band.as_str(), now_unix_seconds)
            .map_err(|_error| HttpProxyError::Selection {
                reason: QuotaAwareAccountSelectorError::StateUnavailable,
            })?;
        let selector_accounts = selector_inputs
            .iter()
            .map(account_input_from_selector_input)
            .collect::<Vec<_>>();
        let assessment_input =
            BurnDownRouteBandAssessmentInput::new(route_band, now_unix_seconds, selector_accounts);
        let assessment = assess_route_band(assessment_input);
        if assessment.selected_pool() == SelectedPool::None {
            return Err(HttpProxyError::Selection {
                reason: QuotaAwareAccountSelectorError::NoEligibleAccounts,
            });
        }

        let mut weighted_selectors =
            self.weighted_selectors
                .lock()
                .map_err(|_error| HttpProxyError::Selection {
                    reason: QuotaAwareAccountSelectorError::SelectorStateUnavailable,
                })?;
        let weighted_selector = weighted_selectors
            .entry(route_band.as_str().to_owned())
            .or_insert_with(WeightedDeficitSelector::default);
        let mut account_holds =
            self.account_holds
                .lock()
                .map_err(|_error| HttpProxyError::Selection {
                    reason: QuotaAwareAccountSelectorError::SelectorStateUnavailable,
                })?;
        if route_kind.previous_response_affinity_capable()
            && let Some(previous_response_id) = previous_response_id(request)?
        {
            let affinity_secret = affinity_secret.ok_or(HttpProxyError::Selection {
                reason: QuotaAwareAccountSelectorError::SecretUnavailable,
            })?;
            let affinity_key_hash =
                hash_previous_response_id(affinity_secret, &previous_response_id).map_err(
                    |_error| HttpProxyError::Selection {
                        reason: QuotaAwareAccountSelectorError::MalformedAffinityKey,
                    },
                )?;
            let owner_lookup = self
                .state_repository
                .load_previous_response_owner(&affinity_key_hash, route_band.as_str())
                .map_err(|_error| HttpProxyError::Selection {
                    reason: QuotaAwareAccountSelectorError::StateUnavailable,
                })?;
            let owner = match owner_lookup {
                PreviousResponseAffinityOwnerLookup::Found(owner) => owner,
                PreviousResponseAffinityOwnerLookup::Missing => {
                    return Err(HttpProxyError::Selection {
                        reason: QuotaAwareAccountSelectorError::AffinityOwnerMissing,
                    });
                }
                PreviousResponseAffinityOwnerLookup::Ambiguous => {
                    return Err(HttpProxyError::Selection {
                        reason: QuotaAwareAccountSelectorError::AffinityOwnerUnavailable,
                    });
                }
            };
            return select_affinity_owner(
                route_band,
                owner.account_id(),
                &assessment,
                &mut account_holds,
                now_unix_seconds,
            );
        }

        select_from_burn_down_assessment(
            route_band.as_str(),
            &assessment,
            weighted_selector,
            &mut account_holds,
            self.minimum_account_hold_cooldown_seconds,
            now_unix_seconds,
        )
    }
}

impl<R> AsyncAccountDecisionSelector for AsyncRepositoryBackedAccountSelector<'_, R>
where
    R: AsyncAffinityRepository + AsyncSelectorQuotaRepository + AsyncQuotaHistoryRepository + Sync,
{
    fn select_upstream_account<'a>(
        &'a self,
        request: &'a HttpProxyRequest,
        _token_generation: TokenGeneration,
        affinity_secret: Option<&'a RouterAffinityHashSecret>,
    ) -> BoxFuture<'a, Result<SelectedAccountDecision, HttpProxyError>> {
        Box::pin(async move {
            let route_kind = route_kind_for_request(request)?;
            let route_band = route_kind.route_band();
            let now_unix_seconds = (self.clock)();
            let selector_inputs = AsyncSelectorQuotaRepository::selector_inputs_for_route_band(
                self.state_repository,
                route_band.as_str(),
                now_unix_seconds,
            )
            .await
            .map_err(|_error| HttpProxyError::Selection {
                reason: QuotaAwareAccountSelectorError::StateUnavailable,
            })?;
            let active_reservation_book = active_reservation_book_for_route_band(
                &self.active_reservations,
                route_band.as_str(),
                now_unix_seconds,
            )?;
            let selector_accounts = account_inputs_from_selector_inputs_with_history(
                self.state_repository,
                route_band.as_str(),
                now_unix_seconds,
                &selector_inputs,
                active_reservation_book.as_ref(),
            )
            .await
            .map_err(|_error| HttpProxyError::Selection {
                reason: QuotaAwareAccountSelectorError::StateUnavailable,
            })?;
            let assessment_input = BurnDownRouteBandAssessmentInput::new(
                route_band,
                now_unix_seconds,
                selector_accounts,
            );
            let assessment = assess_route_band(assessment_input);
            if assessment.selected_pool() == SelectedPool::None {
                return Err(HttpProxyError::Selection {
                    reason: QuotaAwareAccountSelectorError::NoEligibleAccounts,
                });
            }

            let affinity_owner_account_id = if route_kind.previous_response_affinity_capable() {
                match previous_response_id(request)? {
                    Some(previous_response_id) => {
                        let affinity_secret = affinity_secret.ok_or(HttpProxyError::Selection {
                            reason: QuotaAwareAccountSelectorError::SecretUnavailable,
                        })?;
                        let affinity_key_hash =
                            hash_previous_response_id(affinity_secret, &previous_response_id)
                                .map_err(|_error| HttpProxyError::Selection {
                                    reason: QuotaAwareAccountSelectorError::MalformedAffinityKey,
                                })?;
                        let owner_lookup = AsyncAffinityRepository::load_previous_response_owner(
                            self.state_repository,
                            &affinity_key_hash,
                            route_band.as_str(),
                        )
                        .await
                        .map_err(|_error| HttpProxyError::Selection {
                            reason: QuotaAwareAccountSelectorError::StateUnavailable,
                        })?;
                        Some(account_id_from_affinity_owner_lookup(owner_lookup)?)
                    }
                    None => None,
                }
            } else {
                None
            };

            let mut weighted_selectors =
                self.weighted_selectors
                    .lock()
                    .map_err(|_error| HttpProxyError::Selection {
                        reason: QuotaAwareAccountSelectorError::SelectorStateUnavailable,
                    })?;
            let weighted_selector = weighted_selectors
                .entry(route_band.as_str().to_owned())
                .or_insert_with(WeightedDeficitSelector::default);
            let mut account_holds =
                self.account_holds
                    .lock()
                    .map_err(|_error| HttpProxyError::Selection {
                        reason: QuotaAwareAccountSelectorError::SelectorStateUnavailable,
                    })?;

            if let Some(owner_account_id) = affinity_owner_account_id {
                let selected = select_affinity_owner(
                    route_band,
                    &owner_account_id,
                    &assessment,
                    &mut account_holds,
                    now_unix_seconds,
                )?;
                return reserve_selected_account(
                    selected,
                    &self.active_reservations,
                    self.active_client_leases.as_ref(),
                    route_band.as_str(),
                    active_load_pressure_for_request(request),
                    now_unix_seconds,
                );
            }

            let selected = select_from_burn_down_assessment(
                route_band.as_str(),
                &assessment,
                weighted_selector,
                &mut account_holds,
                self.minimum_account_hold_cooldown_seconds,
                now_unix_seconds,
            )?;
            reserve_selected_account(
                selected,
                &self.active_reservations,
                self.active_client_leases.as_ref(),
                route_band.as_str(),
                active_load_pressure_for_request(request),
                now_unix_seconds,
            )
        })
    }
}

fn account_id_from_affinity_owner_lookup(
    owner_lookup: PreviousResponseAffinityOwnerLookup,
) -> Result<AccountId, HttpProxyError> {
    match owner_lookup {
        PreviousResponseAffinityOwnerLookup::Found(owner) => Ok(owner.account_id().clone()),
        PreviousResponseAffinityOwnerLookup::Missing => Err(HttpProxyError::Selection {
            reason: QuotaAwareAccountSelectorError::AffinityOwnerMissing,
        }),
        PreviousResponseAffinityOwnerLookup::Ambiguous => Err(HttpProxyError::Selection {
            reason: QuotaAwareAccountSelectorError::AffinityOwnerUnavailable,
        }),
    }
}

fn select_affinity_owner(
    route_band: RouteBand,
    owner_account_id: &AccountId,
    assessment: &BurnDownRouteBandAssessmentResult,
    account_holds: &mut HashMap<String, AccountHold>,
    now_unix_seconds: u64,
) -> Result<SelectedAccountDecision, HttpProxyError> {
    if !assessment.accounts().iter().any(|account| {
        account.account_id() == owner_account_id
            && matches!(
                account.availability(),
                AccountAvailability::Usable | AccountAvailability::Reserve
            )
    }) {
        account_holds.remove(route_band.as_str());
        return Err(HttpProxyError::Selection {
            reason: QuotaAwareAccountSelectorError::AffinityOwnerUnavailable,
        });
    }

    account_holds.insert(
        route_band.as_str().to_owned(),
        AccountHold::new(owner_account_id.clone(), now_unix_seconds),
    );
    Ok(SelectedAccountDecision::new(
        owner_account_id.clone(),
        "previous_response_affinity",
    ))
}

fn select_from_account_states(
    accounts: &[QuotaAwareAccountState],
    weighted_selector: &Mutex<WeightedDeficitSelector>,
) -> Result<SelectedAccountDecision, HttpProxyError> {
    let mut weighted_selector =
        weighted_selector
            .lock()
            .map_err(|_error| HttpProxyError::Selection {
                reason: QuotaAwareAccountSelectorError::SelectorStateUnavailable,
            })?;
    select_from_account_states_with_selector(accounts, &mut weighted_selector)
}

fn select_from_account_states_with_selector(
    accounts: &[QuotaAwareAccountState],
    weighted_selector: &mut WeightedDeficitSelector,
) -> Result<SelectedAccountDecision, HttpProxyError> {
    let account_inputs = accounts
        .iter()
        .map(account_input_from_quota_state)
        .collect::<Vec<_>>();
    let assessment_input = BurnDownRouteBandAssessmentInput::new(
        RouteBand::Responses,
        current_unix_seconds(),
        account_inputs,
    );
    let assessment = assess_route_band(assessment_input);
    select_from_burn_down_assessment_without_hold(&assessment, weighted_selector)
}

fn select_from_burn_down_assessment_without_hold(
    assessment: &BurnDownRouteBandAssessmentResult,
    weighted_selector: &mut WeightedDeficitSelector,
) -> Result<SelectedAccountDecision, HttpProxyError> {
    if assessment.selected_pool() == SelectedPool::None {
        return Err(HttpProxyError::Selection {
            reason: QuotaAwareAccountSelectorError::NoEligibleAccounts,
        });
    }
    let weighted_candidates = assessment.weighted_candidates();
    let selected_account_id =
        weighted_selector
            .select(weighted_candidates, 1)
            .ok_or(HttpProxyError::Selection {
                reason: QuotaAwareAccountSelectorError::NoEligibleAccounts,
            })?;
    let selected_assessment = assessment
        .accounts()
        .iter()
        .find(|account| account.account_id() == &selected_account_id)
        .ok_or(HttpProxyError::Selection {
            reason: QuotaAwareAccountSelectorError::NoEligibleAccounts,
        })?;

    Ok(SelectedAccountDecision::new(
        selected_account_id,
        selection_reason_for_assessment(selected_assessment),
    ))
}

fn select_from_burn_down_assessment(
    route_band: &str,
    assessment: &BurnDownRouteBandAssessmentResult,
    weighted_selector: &mut WeightedDeficitSelector,
    account_holds: &mut HashMap<String, AccountHold>,
    minimum_account_hold_cooldown_seconds: u64,
    now_unix_seconds: u64,
) -> Result<SelectedAccountDecision, HttpProxyError> {
    let weighted_candidates = assessment.weighted_candidates();
    if weighted_candidates.is_empty() {
        account_holds.remove(route_band);
        return Err(HttpProxyError::Selection {
            reason: QuotaAwareAccountSelectorError::NoEligibleAccounts,
        });
    }

    if let Some(held_account_id) = reusable_held_account_id(
        route_band,
        account_holds,
        weighted_candidates,
        minimum_account_hold_cooldown_seconds,
        now_unix_seconds,
    ) && held_account_allowed_by_weighted_choice(
        weighted_selector,
        weighted_candidates,
        &held_account_id,
    ) && weighted_selector.record_selection(weighted_candidates, &held_account_id)
    {
        return Ok(SelectedAccountDecision::new(
            held_account_id,
            "account_hold_cooldown",
        ));
    }

    let selected_account_id =
        weighted_selector
            .select(weighted_candidates, 1)
            .ok_or(HttpProxyError::Selection {
                reason: QuotaAwareAccountSelectorError::NoEligibleAccounts,
            })?;
    let selected_assessment = assessment
        .accounts()
        .iter()
        .find(|account| account.account_id() == &selected_account_id)
        .ok_or(HttpProxyError::Selection {
            reason: QuotaAwareAccountSelectorError::NoEligibleAccounts,
        })?;
    account_holds.insert(
        route_band.to_owned(),
        AccountHold::new(selected_account_id.clone(), now_unix_seconds),
    );

    Ok(SelectedAccountDecision::new(
        selected_account_id,
        selection_reason_for_assessment(selected_assessment),
    ))
}

fn held_account_allowed_by_weighted_choice(
    weighted_selector: &WeightedDeficitSelector,
    weighted_candidates: &[(AccountId, u32)],
    held_account_id: &AccountId,
) -> bool {
    let mut projected_selector = weighted_selector.clone();
    if projected_selector.select(weighted_candidates, 1).as_ref() == Some(held_account_id) {
        return true;
    }

    let held_weight = weighted_candidates
        .iter()
        .find_map(|(account_id, weight)| (account_id == held_account_id).then_some(*weight));
    let Some(held_weight) = held_weight else {
        return false;
    };
    let best_weight = weighted_candidates
        .iter()
        .map(|(_account_id, weight)| *weight)
        .max()
        .unwrap_or(0);

    best_weight.saturating_sub(held_weight) < WEBSOCKET_ACTIVE_LOAD_PRESSURE
}

fn reusable_held_account_id(
    route_band: &str,
    account_holds: &mut HashMap<String, AccountHold>,
    weighted_candidates: &[(AccountId, u32)],
    minimum_account_hold_cooldown_seconds: u64,
    now_unix_seconds: u64,
) -> Option<AccountId> {
    let hold = account_holds.get(route_band)?;
    let hold_age_seconds = now_unix_seconds.saturating_sub(hold.selected_unix_seconds);
    let reusable = hold_age_seconds < minimum_account_hold_cooldown_seconds
        && weighted_candidates
            .iter()
            .any(|(account_id, _weight)| account_id == &hold.account_id);
    if reusable {
        Some(hold.account_id.clone())
    } else {
        account_holds.remove(route_band);
        None
    }
}

fn account_input_from_selector_input(input: &SelectorQuotaInput) -> BurnDownAccountInput {
    let windows = input
        .windows()
        .iter()
        .map(quota_window_fact_from_selector_window)
        .collect::<Vec<_>>();

    BurnDownAccountInput::new(input.account_id().clone(), input.account_label(), windows)
        .with_account_enabled(input.account_status() == AccountStatus::Enabled)
        .with_active_credential(input.active_credential_generation().is_some())
}

async fn account_inputs_from_selector_inputs_with_history<R>(
    state_repository: &R,
    route_band: &str,
    now_unix_seconds: u64,
    selector_inputs: &[SelectorQuotaInput],
    active_reservation_book: Option<&ReservationBook>,
) -> Result<Vec<BurnDownAccountInput>, StateStoreError>
where
    R: AsyncQuotaHistoryRepository + Sync,
{
    let mut account_inputs = Vec::new();
    for input in selector_inputs {
        let mut windows = Vec::new();
        for window in input.windows() {
            let mut fact = quota_window_fact_from_selector_window(window);
            if let Some(projected_exhaustion_unix_seconds) = projected_exhaustion_from_history(
                state_repository,
                input,
                route_band,
                window,
                now_unix_seconds,
            )
            .await?
            {
                fact =
                    fact.with_projected_exhaustion_unix_seconds(projected_exhaustion_unix_seconds);
            }
            windows.push(fact);
        }
        let active_load_pressure = active_reservation_book
            .map(|book| book.active_load_pressure(input.account_id()))
            .unwrap_or(0);
        account_inputs.push(
            BurnDownAccountInput::new(input.account_id().clone(), input.account_label(), windows)
                .with_account_enabled(input.account_status() == AccountStatus::Enabled)
                .with_active_credential(input.active_credential_generation().is_some())
                .with_active_load_pressure(active_load_pressure),
        );
    }

    Ok(account_inputs)
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

fn reserve_selected_account(
    selected: SelectedAccountDecision,
    active_reservations: &RouteBandReservationBooks,
    active_client_leases: Option<&Arc<dyn ActiveClientLeaseReporter>>,
    route_band: &str,
    headroom_cost: u32,
    now_unix_seconds: u64,
) -> Result<SelectedAccountDecision, HttpProxyError> {
    if headroom_cost == 0 {
        return Ok(selected);
    }
    let active_reservations_guard_source = Arc::clone(active_reservations);
    let mut active_reservations =
        active_reservations
            .lock()
            .map_err(|_error| HttpProxyError::Selection {
                reason: QuotaAwareAccountSelectorError::SelectorStateUnavailable,
            })?;
    let reservation_handle = active_reservations
        .entry(route_band.to_owned())
        .or_insert_with(ReservationBook::default)
        .reserve_next_at(
            selected.account_id().clone(),
            headroom_cost,
            now_unix_seconds,
        );
    if let Some(active_client_leases) = active_client_leases {
        active_client_leases.record_acquired(route_band, &reservation_handle, now_unix_seconds);
    }
    Ok(selected.with_active_reservation_guard(
        ActiveReservationGuard::new_with_active_client_leases(
            active_reservations_guard_source,
            route_band.to_owned(),
            reservation_handle,
            active_client_leases.cloned(),
        ),
    ))
}

/// Releases a selection reservation from route-band active load accounting.
pub fn release_account_reservation(
    active_reservations: &RouteBandReservationBooks,
    route_band: &str,
    reservation_handle: &ReservationHandle,
) {
    let Ok(mut active_reservations) = active_reservations.lock() else {
        return;
    };
    if let Some(book) = active_reservations.get_mut(route_band) {
        book.release_handle(reservation_handle);
    }
}

fn quota_window_fact_from_selector_window(
    window: &PersistedSelectorQuotaWindow,
) -> QuotaWindowFact {
    let mut fact = QuotaWindowFact::new(
        window.limit_window_seconds(),
        quota_window_status_from_selector_status(window.status()),
    )
    .with_remaining_headroom(window.remaining_headroom())
    .with_observed_unix_seconds(window.observed_unix_seconds())
    .with_effective(window.effective());
    if let Some(reset_unix_seconds) = window.reset_unix_seconds() {
        fact = fact.with_reset_unix_seconds(reset_unix_seconds);
    }

    fact
}

async fn projected_exhaustion_from_history<R>(
    state_repository: &R,
    input: &SelectorQuotaInput,
    route_band: &str,
    window: &PersistedSelectorQuotaWindow,
    now_unix_seconds: u64,
) -> Result<Option<u64>, StateStoreError>
where
    R: AsyncQuotaHistoryRepository + Sync,
{
    let Some(reset_unix_seconds) = window.reset_unix_seconds() else {
        return Ok(None);
    };
    let history = AsyncQuotaHistoryRepository::quota_history_observations_for_window(
        state_repository,
        input.account_id(),
        route_band,
        window.limit_window_seconds(),
        now_unix_seconds.saturating_sub(QUOTA_HISTORY_LOOKBACK_SECONDS),
        now_unix_seconds,
    )
    .await?;
    let observations = history
        .iter()
        .filter_map(|observation| {
            observation
                .reset_unix_seconds()
                .map(|history_reset_unix_seconds| {
                    QuotaRunRateObservation::new(
                        observation.observed_unix_seconds(),
                        history_reset_unix_seconds,
                        observation.remaining_headroom(),
                    )
                })
        })
        .collect::<Vec<_>>();
    let estimate = QuotaRunRateEstimator::new(QUOTA_HISTORY_FRESHNESS_SECONDS).estimate(
        now_unix_seconds,
        reset_unix_seconds,
        &observations,
    );
    if !matches!(
        estimate.confidence(),
        QuotaRunRateConfidence::Low | QuotaRunRateConfidence::Normal
    ) {
        return Ok(None);
    }

    Ok(estimate.projected_exhaustion_unix_seconds(now_unix_seconds))
}

fn route_kind_for_request(
    request: &HttpProxyRequest,
) -> Result<crate::routes::RouteKind, HttpProxyError> {
    let classification_path = path_without_query(request.path());
    match classify_route(
        request.method(),
        classification_path,
        request.websocket_upgrade(),
    ) {
        RouteClass::Supported(route_kind) => Ok(route_kind),
        RouteClass::Rejected { reason } => Err(HttpProxyError::Rejected { reason }),
    }
}

const fn active_load_pressure_for_request(request: &HttpProxyRequest) -> u32 {
    if request.websocket_upgrade() {
        WEBSOCKET_ACTIVE_LOAD_PRESSURE
    } else {
        HTTP_ACTIVE_LOAD_PRESSURE
    }
}

fn path_without_query(path: &str) -> &str {
    path.split_once('?')
        .map_or(path, |(path_without_query, _query)| path_without_query)
}

fn previous_response_id(
    request: &HttpProxyRequest,
) -> Result<Option<PreviousResponseId>, HttpProxyError> {
    let Some(previous_response_id) =
        top_level_json_string_field(request.body(), b"previous_response_id")?
    else {
        return Ok(None);
    };
    if previous_response_id.is_empty() {
        return Ok(None);
    }

    Ok(PreviousResponseId::new(previous_response_id).ok())
}

fn top_level_json_string_field(
    body: &[u8],
    field_name: &[u8],
) -> Result<Option<String>, HttpProxyError> {
    let mut cursor = skip_json_whitespace(body, 0);
    if body.get(cursor) != Some(&b'{') {
        return Ok(None);
    }
    cursor += 1;
    let mut depth = 1_u32;
    let scan_end = body.len().min(ROUTING_METADATA_SCAN_LIMIT_BYTES);
    let mut top_level_keys = 0_usize;

    while cursor < scan_end {
        cursor = skip_json_whitespace(body, cursor);
        let Some(byte) = body.get(cursor).copied() else {
            return Ok(None);
        };
        match byte {
            b'"' => {
                let Some((string_slice, after_string)) = json_string_slice(body, cursor) else {
                    return Ok(None);
                };
                if after_string > scan_end {
                    return Ok(None);
                }
                let after_key = skip_json_whitespace(body, after_string);
                if depth == 1 && body.get(after_key) == Some(&b':') {
                    top_level_keys += 1;
                    if top_level_keys > ROUTING_METADATA_SCAN_MAX_TOP_LEVEL_KEYS {
                        return Ok(None);
                    }
                    let Some(key) = serde_json::from_slice::<String>(string_slice).ok() else {
                        return Ok(None);
                    };
                    if key.as_bytes() == field_name {
                        let value_start = skip_json_whitespace(body, after_key + 1);
                        let Some((value_slice, _after_value)) =
                            json_string_slice(body, value_start)
                        else {
                            return Ok(None);
                        };
                        let Some(value) = serde_json::from_slice::<String>(value_slice).ok() else {
                            return Ok(None);
                        };
                        return Ok(Some(value));
                    }
                    cursor = after_key + 1;
                } else {
                    cursor = after_string;
                }
            }
            b'{' | b'[' => {
                depth = depth.saturating_add(1);
                cursor += 1;
            }
            b'}' | b']' => {
                depth = depth.saturating_sub(1);
                cursor += 1;
                if depth == 0 {
                    return Ok(None);
                }
            }
            _ => {
                cursor += 1;
            }
        }
    }

    Ok(None)
}

fn json_string_slice(body: &[u8], start: usize) -> Option<(&[u8], usize)> {
    if body.get(start) != Some(&b'"') {
        return None;
    }
    let mut cursor = start + 1;
    while cursor < body.len() {
        match body[cursor] {
            b'\\' => {
                cursor = cursor.saturating_add(2);
            }
            b'"' => {
                let end = cursor + 1;
                return Some((&body[start..end], end));
            }
            _ => {
                cursor += 1;
            }
        }
    }
    None
}

fn skip_json_whitespace(body: &[u8], mut cursor: usize) -> usize {
    while body
        .get(cursor)
        .is_some_and(|byte| matches!(byte, b' ' | b'\n' | b'\r' | b'\t'))
    {
        cursor += 1;
    }
    cursor
}

fn account_input_from_quota_state(account: &QuotaAwareAccountState) -> BurnDownAccountInput {
    let status = quota_window_status_from_freshness(account.freshness);
    let reset_base = current_unix_seconds();
    let short_window = QuotaWindowFact::new(V1_SHORT_WINDOW_SECONDS, status)
        .with_remaining_headroom(account.remaining_headroom)
        .with_reset_unix_seconds(reset_base)
        .with_observed_unix_seconds(reset_base)
        .with_effective(true);
    let weekly_window = QuotaWindowFact::new(V1_WEEKLY_WINDOW_SECONDS, status)
        .with_remaining_headroom(account.remaining_headroom)
        .with_reset_unix_seconds(reset_base)
        .with_observed_unix_seconds(reset_base)
        .with_effective(false);
    BurnDownAccountInput::new(
        account.account_id.clone(),
        account.account_id.as_str(),
        vec![short_window, weekly_window],
    )
}

const fn quota_window_status_from_freshness(freshness: SnapshotFreshness) -> QuotaWindowStatus {
    match freshness {
        SnapshotFreshness::Fresh { .. } => QuotaWindowStatus::Eligible,
        SnapshotFreshness::StaleWithPenalty { .. } => QuotaWindowStatus::Stale,
        SnapshotFreshness::Unknown => QuotaWindowStatus::Unknown,
    }
}

const fn quota_window_status_from_selector_status(
    status: SelectorQuotaWindowStatus,
) -> QuotaWindowStatus {
    match status {
        SelectorQuotaWindowStatus::Eligible => QuotaWindowStatus::Eligible,
        SelectorQuotaWindowStatus::Stale => QuotaWindowStatus::Stale,
        SelectorQuotaWindowStatus::Unknown => QuotaWindowStatus::Unknown,
        SelectorQuotaWindowStatus::Ineligible => QuotaWindowStatus::Ineligible,
    }
}

fn selection_reason_for_assessment(assessment: &BurnDownAccountAssessment) -> String {
    assessment.routing_reason().as_str().to_owned()
}

fn current_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

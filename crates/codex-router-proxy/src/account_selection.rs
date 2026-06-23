//! Token-free account-selection boundary.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use codex_router_core::affinity::PreviousResponseId;
use codex_router_core::affinity::RouterAffinityHashSecret;
use codex_router_core::affinity::hash_previous_response_id;
use codex_router_core::ids::AccountId;
use codex_router_core::ids::TokenGeneration;
use codex_router_core::routes::RouteBand;
use codex_router_quota::snapshot::SnapshotFreshness;
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
use codex_router_selection::weighted_deficit::WeightedDeficitSelector;
use codex_router_state::account::AccountStatus;
use codex_router_state::affinity_owner::PreviousResponseAffinityOwnerLookup;
use codex_router_state::quota_snapshot::SelectorQuotaInput;
use codex_router_state::quota_snapshot::SelectorQuotaWindowStatus;
use codex_router_state::repositories::AffinityRepository;
use codex_router_state::repositories::SelectorQuotaRepository;
use thiserror::Error;

use crate::http_sse::HttpProxyError;
use crate::http_sse::HttpProxyRequest;
use crate::routes::RouteClass;
use crate::routes::classify_route;

/// Process-lifetime weighted state partitioned by route band.
pub type RouteBandWeightedSelectors = Arc<Mutex<HashMap<String, WeightedDeficitSelector>>>;
/// Process-lifetime account-hold state partitioned by route band.
pub type RouteBandAccountHolds = Arc<Mutex<HashMap<String, AccountHold>>>;

/// Default v1 minimum account reuse period for adjacent normal requests.
pub const DEFAULT_ACCOUNT_HOLD_COOLDOWN_SECONDS: u64 = 120;

type UnixClock = Arc<dyn Fn() -> u64 + Send + Sync>;

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
}

impl SelectedAccountDecision {
    /// Creates selected account material.
    #[must_use]
    pub fn new(account_id: AccountId, selection_reason: impl Into<String>) -> Self {
        Self {
            account_id,
            selection_reason: selection_reason.into(),
        }
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
        let route_band = route_band_for_request(request)?;
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
        if let Some(previous_response_id) = previous_response_id(request)? {
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

fn select_affinity_owner(
    route_band: RouteBand,
    owner_account_id: &AccountId,
    assessment: &BurnDownRouteBandAssessmentResult,
    account_holds: &mut HashMap<String, AccountHold>,
    now_unix_seconds: u64,
) -> Result<SelectedAccountDecision, HttpProxyError> {
    let weighted_candidates = assessment.weighted_candidates();
    if !weighted_candidates
        .iter()
        .any(|(account_id, _weight)| account_id == owner_account_id)
    {
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
        .map(|account| account_input_from_quota_state(account))
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
    ) {
        if weighted_selector.record_selection(weighted_candidates, &held_account_id) {
            return Ok(SelectedAccountDecision::new(
                held_account_id,
                "account_hold_cooldown",
            ));
        }
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
        .map(|window| {
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
        })
        .collect::<Vec<_>>();

    BurnDownAccountInput::new(input.account_id().clone(), input.account_label(), windows)
        .with_account_enabled(input.account_status() == AccountStatus::Enabled)
        .with_active_credential(input.active_credential_generation().is_some())
}

fn route_band_for_request(request: &HttpProxyRequest) -> Result<RouteBand, HttpProxyError> {
    let classification_path = path_without_query(request.path());
    match classify_route(
        request.method(),
        classification_path,
        request.websocket_upgrade(),
    ) {
        RouteClass::Supported(route_kind) => Ok(route_kind.route_band()),
        RouteClass::Rejected { reason } => Err(HttpProxyError::Rejected { reason }),
    }
}

fn path_without_query(path: &str) -> &str {
    path.split_once('?')
        .map_or(path, |(path_without_query, _query)| path_without_query)
}

fn previous_response_id(
    request: &HttpProxyRequest,
) -> Result<Option<PreviousResponseId>, HttpProxyError> {
    if !body_mentions_previous_response_id(request.body()) {
        return Ok(None);
    }

    let value = serde_json::from_slice::<serde_json::Value>(request.body()).map_err(|_error| {
        HttpProxyError::Selection {
            reason: QuotaAwareAccountSelectorError::MalformedAffinityKey,
        }
    })?;
    let Some(previous_response_id) = value.get("previous_response_id") else {
        return Ok(None);
    };
    let previous_response_id = previous_response_id
        .as_str()
        .ok_or(HttpProxyError::Selection {
            reason: QuotaAwareAccountSelectorError::MalformedAffinityKey,
        })?;
    if previous_response_id.is_empty() {
        return Err(HttpProxyError::Selection {
            reason: QuotaAwareAccountSelectorError::MalformedAffinityKey,
        });
    }

    PreviousResponseId::new(previous_response_id.to_owned())
        .map(Some)
        .map_err(|_error| HttpProxyError::Selection {
            reason: QuotaAwareAccountSelectorError::MalformedAffinityKey,
        })
}

fn body_mentions_previous_response_id(body: &[u8]) -> bool {
    body.windows(b"previous_response_id".len())
        .any(|window| window == b"previous_response_id")
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

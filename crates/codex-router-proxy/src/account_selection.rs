//! Token-free account-selection boundary.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use codex_router_core::ids::AccountId;
use codex_router_core::ids::TokenGeneration;
use codex_router_quota::snapshot::SnapshotFreshness;
use codex_router_selection::eligibility::Eligibility;
use codex_router_selection::eligibility::SelectionCandidate;
use codex_router_selection::weighted_deficit::WeightedDeficitSelector;
use codex_router_state::account::AccountStatus;
use codex_router_state::quota_snapshot::SelectorQuotaInput;
use codex_router_state::quota_snapshot::SelectorQuotaWindowStatus;
use codex_router_state::repositories::SelectorQuotaRepository;
use thiserror::Error;

use crate::http_sse::HttpProxyError;
use crate::http_sse::HttpProxyRequest;
use crate::routes::RouteClass;
use crate::routes::RouteKind;
use crate::routes::classify_route;

/// Process-lifetime weighted state partitioned by route band.
pub type RouteBandWeightedSelectors = Arc<Mutex<HashMap<String, WeightedDeficitSelector>>>;

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
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct WeightedAccountCandidate {
    account_id: AccountId,
    effective_headroom: u32,
    selection_reason: &'static str,
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
    ) -> Result<SelectedAccountDecision, HttpProxyError> {
        select_from_account_states(&self.accounts, &self.weighted_selector)
    }
}

/// Selector that hydrates account state from repositories at request time.
#[derive(Debug)]
pub struct RepositoryBackedAccountSelector<'a, R>
where
    R: SelectorQuotaRepository,
{
    state_repository: &'a R,
    weighted_selectors: RouteBandWeightedSelectors,
}

impl<'a, R> RepositoryBackedAccountSelector<'a, R>
where
    R: SelectorQuotaRepository,
{
    /// Creates a repository-backed selector.
    #[must_use]
    pub fn new(state_repository: &'a R) -> Self {
        Self {
            state_repository,
            weighted_selectors: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Creates a repository-backed selector with process-lifetime weighted state.
    #[must_use]
    pub fn new_with_weighted_selector(
        state_repository: &'a R,
        weighted_selectors: RouteBandWeightedSelectors,
    ) -> Self {
        Self {
            state_repository,
            weighted_selectors,
        }
    }
}

impl<R> AccountDecisionSelector for RepositoryBackedAccountSelector<'_, R>
where
    R: SelectorQuotaRepository,
{
    fn select_upstream_account(
        &self,
        request: &HttpProxyRequest,
        _token_generation: TokenGeneration,
    ) -> Result<SelectedAccountDecision, HttpProxyError> {
        let route_band = route_band_for_request(request)?;
        let selector_inputs = self
            .state_repository
            .selector_inputs_for_route_band(route_band)
            .map_err(|_error| HttpProxyError::Selection {
                reason: QuotaAwareAccountSelectorError::StateUnavailable,
            })?;
        let selector_accounts = selector_inputs
            .iter()
            .filter_map(account_state_from_selector_input)
            .collect::<Vec<_>>();

        let mut weighted_selectors =
            self.weighted_selectors
                .lock()
                .map_err(|_error| HttpProxyError::Selection {
                    reason: QuotaAwareAccountSelectorError::SelectorStateUnavailable,
                })?;
        let weighted_selector = weighted_selectors
            .entry(route_band.to_owned())
            .or_insert_with(WeightedDeficitSelector::default);
        select_from_account_states_with_selector(&selector_accounts, weighted_selector)
    }
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
    let known_fresh_account_exists = accounts.iter().any(|account| {
        account.remaining_headroom > 0
            && matches!(account.freshness, SnapshotFreshness::Fresh { .. })
    });
    let weighted_candidates = accounts
        .iter()
        .filter_map(|account| weighted_candidate_for_account(account, known_fresh_account_exists))
        .collect::<Vec<_>>();
    let selector_input = weighted_candidates
        .iter()
        .map(|candidate| (candidate.account_id.clone(), candidate.effective_headroom))
        .collect::<Vec<_>>();
    let selected_account_id =
        weighted_selector
            .select(&selector_input, 1)
            .ok_or(HttpProxyError::Selection {
                reason: QuotaAwareAccountSelectorError::NoEligibleAccounts,
            })?;
    let selected_candidate = weighted_candidates
        .iter()
        .find(|candidate| candidate.account_id == selected_account_id)
        .ok_or(HttpProxyError::Selection {
            reason: QuotaAwareAccountSelectorError::NoEligibleAccounts,
        })?;

    Ok(SelectedAccountDecision::new(
        selected_account_id,
        selected_candidate.selection_reason,
    ))
}

fn account_state_from_selector_input(input: &SelectorQuotaInput) -> Option<QuotaAwareAccountState> {
    if input.account_status() != AccountStatus::Enabled {
        return None;
    }
    input.active_credential_generation()?;
    let effective_window = input.windows().iter().find(|window| window.effective())?;
    let freshness = match effective_window.status() {
        SelectorQuotaWindowStatus::Eligible => SnapshotFreshness::Fresh { age_seconds: 0 },
        SelectorQuotaWindowStatus::Stale => SnapshotFreshness::StaleWithPenalty { age_seconds: 0 },
        SelectorQuotaWindowStatus::Unknown => SnapshotFreshness::Unknown,
        SelectorQuotaWindowStatus::Ineligible => return None,
    };

    Some(QuotaAwareAccountState::new(
        input.account_id().clone(),
        effective_window.remaining_headroom(),
        freshness,
    ))
}

fn route_band_for_request(request: &HttpProxyRequest) -> Result<&'static str, HttpProxyError> {
    let classification_path = path_without_query(request.path());
    match classify_route(
        request.method(),
        classification_path,
        request.websocket_upgrade(),
    ) {
        RouteClass::Supported(RouteKind::Responses | RouteKind::ResponsesWebSocket) => {
            Ok("responses")
        }
        RouteClass::Supported(RouteKind::Models) => Ok("models"),
        RouteClass::Supported(RouteKind::MemoriesTraceSummarize) => Ok("memories_trace_summarize"),
        RouteClass::Supported(RouteKind::ResponsesCompact) => Ok("responses_compact"),
        RouteClass::Rejected { reason } => Err(HttpProxyError::Rejected { reason }),
    }
}

fn path_without_query(path: &str) -> &str {
    path.split_once('?')
        .map_or(path, |(path_without_query, _query)| path_without_query)
}

fn weighted_candidate_for_account(
    account: &QuotaAwareAccountState,
    known_fresh_account_exists: bool,
) -> Option<WeightedAccountCandidate> {
    let candidate = SelectionCandidate::new(
        account.account_id.clone(),
        account.remaining_headroom,
        account.freshness,
    );
    match candidate.eligibility(known_fresh_account_exists) {
        Eligibility::Eligible { headroom } => Some(WeightedAccountCandidate {
            account_id: account.account_id.clone(),
            effective_headroom: headroom,
            selection_reason: selection_reason_for_freshness(account.freshness),
        }),
        Eligibility::Penalized { headroom, reason } => Some(WeightedAccountCandidate {
            account_id: account.account_id.clone(),
            effective_headroom: headroom,
            selection_reason: reason,
        }),
        Eligibility::Ineligible { .. } => None,
    }
}

const fn selection_reason_for_freshness(freshness: SnapshotFreshness) -> &'static str {
    match freshness {
        SnapshotFreshness::Fresh { .. } => "fresh_quota",
        SnapshotFreshness::StaleWithPenalty { .. } => "stale_quota_fallback",
        SnapshotFreshness::Unknown => "unknown_quota_fallback",
    }
}

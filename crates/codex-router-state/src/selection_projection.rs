//! Shared SQLx-to-selector projection.

use std::collections::HashMap;

use codex_router_core::ids::AccountId;
use codex_router_core::provider::Provider;
use codex_router_core::route_profile::WindowKind;
use codex_router_core::routes::RouteBand;
use codex_router_selection::burn_down::ACTIVE_SESSION_ROLLUP_BUCKET_SECONDS;
use codex_router_selection::burn_down::BurnDownAccountInput;
use codex_router_selection::burn_down::CreditBackedEligibility;
use codex_router_selection::burn_down::QuotaEvidenceFreshness;
use codex_router_selection::burn_down::QuotaWindowFact;
use codex_router_selection::burn_down::QuotaWindowRejectionFact;
use codex_router_selection::burn_down::QuotaWindowStatus;
use codex_router_selection::burn_down::V1_SHORT_WINDOW_SECONDS;
use codex_router_selection::burn_down::V1_WEEKLY_WINDOW_SECONDS;
use codex_router_selection::run_rate::NORMAL_CONFIDENCE_MIN_SPAN_SECONDS;
use codex_router_selection::run_rate::QuotaRunRateConfidence;
use codex_router_selection::selection_outcome::HeadroomTimestamp;
use codex_router_selection::selection_outcome::SelectionAccountRestriction;
use codex_router_selection::selection_outcome::SelectionAccountState;
use codex_router_selection::selection_outcome::SelectionHoldReason;
use codex_router_selection::selection_outcome::SelectionWindowObservation;
use codex_router_selection::selection_outcome::SelectionWindowRejection;
use futures_util::future::BoxFuture;

use crate::account::AccountStatus;
use crate::account_routing_policy::AccountRoutingPolicy;
use crate::credential_maintenance::CredentialMaintenanceState;
use crate::quota_snapshot::PersistedQuotaHistoryObservation;
use crate::quota_snapshot::PersistedSelectorQuotaWindow;
use crate::quota_snapshot::SelectorQuotaInput;
use crate::quota_snapshot::SelectorQuotaWindowStatus;
use crate::sqlite::ActiveClientCount;
use crate::sqlite::ActiveSessionRollup;
use crate::sqlite::AsyncSqliteStateStore;
use crate::sqlite::StateStoreError;
use crate::window_observation::LEGACY_QUOTA_EVIDENCE_FRESHNESS_SECONDS;

const QUOTA_HISTORY_LOOKBACK_SECONDS: u64 = 14 * 24 * 60 * 60;

/// Projected selector inputs for one route band.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RouteBandSelectionProjection {
    accounts: Vec<BurnDownAccountInput>,
    account_states: Vec<SelectionAccountState>,
}

impl RouteBandSelectionProjection {
    /// Creates a route-band selection projection.
    #[must_use]
    pub fn new(
        accounts: Vec<BurnDownAccountInput>,
        account_states: Vec<SelectionAccountState>,
    ) -> Self {
        Self {
            accounts,
            account_states,
        }
    }

    /// Returns projected account selector inputs.
    #[must_use]
    pub fn accounts(&self) -> &[BurnDownAccountInput] {
        &self.accounts
    }

    /// Returns provider restrictions and D9 window state for R12 classification.
    #[must_use]
    pub fn account_states(&self) -> &[SelectionAccountState] {
        &self.account_states
    }
}

/// Async state operations required to project selector inputs.
pub trait AsyncSelectionProjectionRepository {
    /// Bulk-loads all enabled per-account routing policies.
    fn list_account_routing_policies(
        &self,
    ) -> BoxFuture<'_, Result<Vec<AccountRoutingPolicy>, StateStoreError>>;

    /// Loads selector inputs for one route band.
    fn selector_inputs_for_route_band<'a>(
        &'a self,
        route_band: &'a str,
        now_unix_seconds: u64,
    ) -> BoxFuture<'a, Result<Vec<SelectorQuotaInput>, StateStoreError>>;

    /// Loads current active client counts for one route band.
    fn active_client_counts_for_route_band<'a>(
        &'a self,
        route_band: &'a str,
        now_unix_seconds: u64,
        max_age_seconds: u64,
    ) -> BoxFuture<'a, Result<Vec<ActiveClientCount>, StateStoreError>>;

    /// Loads current active client counts for one route band without stale-lease cleanup.
    fn active_client_counts_for_route_band_read_only<'a>(
        &'a self,
        route_band: &'a str,
        now_unix_seconds: u64,
        max_age_seconds: u64,
    ) -> BoxFuture<'a, Result<Vec<ActiveClientCount>, StateStoreError>>;

    /// Loads quota history observations for one account/window.
    fn quota_history_observations_for_window<'a>(
        &'a self,
        account_id: &'a AccountId,
        route_band: &'a str,
        limit_window_seconds: u64,
        observed_from_unix_seconds: u64,
        observed_to_unix_seconds: u64,
    ) -> BoxFuture<'a, Result<Vec<PersistedQuotaHistoryObservation>, StateStoreError>>;

    /// Loads active-session rollups for one route band and interval.
    fn active_session_rollups_for_route_band<'a>(
        &'a self,
        route_band: &'a str,
        interval_start_unix_seconds: u64,
        interval_end_unix_seconds: u64,
    ) -> BoxFuture<'a, Result<Vec<ActiveSessionRollup>, StateStoreError>>;

    /// Refreshes active-session rollups for one route band and interval.
    fn refresh_active_session_rollups_for_interval<'a>(
        &'a self,
        route_band: &'a str,
        interval_start_unix_seconds: u64,
        interval_end_unix_seconds: u64,
        bucket_seconds: u64,
    ) -> BoxFuture<'a, Result<(), StateStoreError>>;
}

impl AsyncSelectionProjectionRepository for AsyncSqliteStateStore {
    fn list_account_routing_policies(
        &self,
    ) -> BoxFuture<'_, Result<Vec<AccountRoutingPolicy>, StateStoreError>> {
        Box::pin(async move { self.list_account_routing_policies().await })
    }

    fn selector_inputs_for_route_band<'a>(
        &'a self,
        route_band: &'a str,
        now_unix_seconds: u64,
    ) -> BoxFuture<'a, Result<Vec<SelectorQuotaInput>, StateStoreError>> {
        Box::pin(async move {
            self.selector_inputs_for_route_band(route_band, now_unix_seconds)
                .await
        })
    }

    fn active_client_counts_for_route_band<'a>(
        &'a self,
        route_band: &'a str,
        now_unix_seconds: u64,
        max_age_seconds: u64,
    ) -> BoxFuture<'a, Result<Vec<ActiveClientCount>, StateStoreError>> {
        Box::pin(async move {
            AsyncSqliteStateStore::active_client_counts_for_route_band(
                self,
                route_band,
                now_unix_seconds,
                max_age_seconds,
            )
            .await
        })
    }

    fn active_client_counts_for_route_band_read_only<'a>(
        &'a self,
        route_band: &'a str,
        now_unix_seconds: u64,
        max_age_seconds: u64,
    ) -> BoxFuture<'a, Result<Vec<ActiveClientCount>, StateStoreError>> {
        Box::pin(async move {
            AsyncSqliteStateStore::active_client_counts_for_route_band_read_only(
                self,
                route_band,
                now_unix_seconds,
                max_age_seconds,
            )
            .await
        })
    }

    fn quota_history_observations_for_window<'a>(
        &'a self,
        account_id: &'a AccountId,
        route_band: &'a str,
        limit_window_seconds: u64,
        observed_from_unix_seconds: u64,
        observed_to_unix_seconds: u64,
    ) -> BoxFuture<'a, Result<Vec<PersistedQuotaHistoryObservation>, StateStoreError>> {
        Box::pin(async move {
            self.quota_history_observations_for_window(
                account_id,
                route_band,
                limit_window_seconds,
                observed_from_unix_seconds,
                observed_to_unix_seconds,
            )
            .await
        })
    }

    fn active_session_rollups_for_route_band<'a>(
        &'a self,
        route_band: &'a str,
        interval_start_unix_seconds: u64,
        interval_end_unix_seconds: u64,
    ) -> BoxFuture<'a, Result<Vec<ActiveSessionRollup>, StateStoreError>> {
        Box::pin(async move {
            self.active_session_rollups_for_route_band(
                route_band,
                interval_start_unix_seconds,
                interval_end_unix_seconds,
            )
            .await
        })
    }

    fn refresh_active_session_rollups_for_interval<'a>(
        &'a self,
        route_band: &'a str,
        interval_start_unix_seconds: u64,
        interval_end_unix_seconds: u64,
        bucket_seconds: u64,
    ) -> BoxFuture<'a, Result<(), StateStoreError>> {
        Box::pin(async move {
            self.refresh_active_session_rollups_for_interval(
                route_band,
                interval_start_unix_seconds,
                interval_end_unix_seconds,
                bucket_seconds,
            )
            .await
        })
    }
}

/// Projects persisted state into pure selector inputs for one route band.
pub async fn project_route_band_selection_inputs<R>(
    state: &R,
    route_band: &str,
    now_unix_seconds: u64,
    active_client_max_age_seconds: u64,
) -> Result<RouteBandSelectionProjection, StateStoreError>
where
    R: AsyncSelectionProjectionRepository + Sync,
{
    project_route_band_selection_inputs_with_active_counts_internal(
        state,
        route_band,
        now_unix_seconds,
        active_client_max_age_seconds,
        None,
        true,
    )
    .await
}

/// Projects persisted state for read-only observer surfaces.
pub async fn project_route_band_selection_inputs_read_only<R>(
    state: &R,
    route_band: &str,
    now_unix_seconds: u64,
    active_client_max_age_seconds: u64,
) -> Result<RouteBandSelectionProjection, StateStoreError>
where
    R: AsyncSelectionProjectionRepository + Sync,
{
    project_route_band_selection_inputs_with_active_counts_internal(
        state,
        route_band,
        now_unix_seconds,
        active_client_max_age_seconds,
        None,
        false,
    )
    .await
}

/// Projects persisted state with caller-owned current active-session overrides.
pub async fn project_route_band_selection_inputs_with_active_counts<R>(
    state: &R,
    route_band: &str,
    now_unix_seconds: u64,
    active_client_max_age_seconds: u64,
    active_session_overrides: Option<&HashMap<AccountId, u32>>,
) -> Result<RouteBandSelectionProjection, StateStoreError>
where
    R: AsyncSelectionProjectionRepository + Sync,
{
    project_route_band_selection_inputs_with_active_counts_internal(
        state,
        route_band,
        now_unix_seconds,
        active_client_max_age_seconds,
        active_session_overrides,
        true,
    )
    .await
}

/// Projects persisted state with caller-owned current active-session overrides for read-only observer paths.
pub async fn project_route_band_selection_inputs_with_active_counts_read_only<R>(
    state: &R,
    route_band: &str,
    now_unix_seconds: u64,
    active_client_max_age_seconds: u64,
    active_session_overrides: Option<&HashMap<AccountId, u32>>,
) -> Result<RouteBandSelectionProjection, StateStoreError>
where
    R: AsyncSelectionProjectionRepository + Sync,
{
    project_route_band_selection_inputs_with_active_counts_internal(
        state,
        route_band,
        now_unix_seconds,
        active_client_max_age_seconds,
        active_session_overrides,
        false,
    )
    .await
}

async fn project_route_band_selection_inputs_with_active_counts_internal<R>(
    state: &R,
    route_band: &str,
    now_unix_seconds: u64,
    active_client_max_age_seconds: u64,
    active_session_overrides: Option<&HashMap<AccountId, u32>>,
    refresh_rollups: bool,
) -> Result<RouteBandSelectionProjection, StateStoreError>
where
    R: AsyncSelectionProjectionRepository + Sync,
{
    let account_routing_policies = state.list_account_routing_policies().await?;
    let weekly_quota_floors = account_routing_policies
        .into_iter()
        .map(|policy| {
            (
                policy.account_id().clone(),
                u32::from(policy.weekly_quota_floor_basis_points().basis_points()),
            )
        })
        .collect::<HashMap<_, _>>();
    let selector_inputs = state
        .selector_inputs_for_route_band(route_band, now_unix_seconds)
        .await?;
    let active_counts = if refresh_rollups {
        state
            .active_client_counts_for_route_band(
                route_band,
                now_unix_seconds,
                active_client_max_age_seconds,
            )
            .await
    } else {
        state
            .active_client_counts_for_route_band_read_only(
                route_band,
                now_unix_seconds,
                active_client_max_age_seconds,
            )
            .await
    }?;
    let mut projected_accounts = Vec::with_capacity(selector_inputs.len());
    let mut account_states = Vec::with_capacity(selector_inputs.len());

    for input in selector_inputs {
        let current_active_sessions = active_counts
            .iter()
            .find(|count| count.account_id() == input.account_id())
            .map_or(0, |count| count.active_clients());
        let current_active_sessions = active_session_overrides
            .and_then(|overrides| overrides.get(input.account_id()).copied())
            .unwrap_or(current_active_sessions);
        let weekly_floor_basis_points = weekly_quota_floors.get(input.account_id()).copied();
        account_states.push(selection_account_state_from_selector_input(
            &input,
            weekly_floor_basis_points,
            now_unix_seconds,
        ));

        let mut windows = if input.provider() == Provider::Claude {
            claude_window_facts_from_observations(&input, now_unix_seconds)
        } else {
            Vec::with_capacity(input.windows().len())
        };
        if input.provider() == Provider::Openai {
            for window in input.windows() {
                let mut fact = quota_window_fact_from_selector_window(window);
                let projected_active_sessions = current_active_sessions.saturating_add(1);
                let estimate = estimate_window_burn_rate(
                    state,
                    input.account_id(),
                    route_band,
                    window,
                    now_unix_seconds,
                    refresh_rollups,
                )
                .await?;
                if let Some(per_connection_burn_basis_points_per_hour) =
                    estimate.per_connection_burn_basis_points_per_hour
                {
                    let projected_candidate_burn_basis_points_per_hour =
                        per_connection_burn_basis_points_per_hour
                            .saturating_mul(projected_active_sessions.max(1));
                    fact = fact
                        .with_per_connection_burn_basis_points_per_hour(
                            per_connection_burn_basis_points_per_hour,
                        )
                        .with_projected_candidate_burn_basis_points_per_hour(
                            projected_candidate_burn_basis_points_per_hour,
                        );
                    if let Some(projected_exhaustion_unix_seconds) =
                        projected_exhaustion_unix_seconds(
                            now_unix_seconds,
                            window.remaining_headroom(),
                            projected_candidate_burn_basis_points_per_hour,
                        )
                    {
                        fact = fact.with_projected_exhaustion_unix_seconds(
                            projected_exhaustion_unix_seconds,
                        );
                    }
                } else if let Some(aggregate_burn_basis_points_per_hour) =
                    estimate.aggregate_burn_basis_points_per_hour
                {
                    fact = fact
                        .with_aggregate_burn_basis_points_per_hour(
                            aggregate_burn_basis_points_per_hour,
                        )
                        .with_projected_candidate_burn_basis_points_per_hour(
                            aggregate_burn_basis_points_per_hour,
                        );
                    if let Some(projected_exhaustion_unix_seconds) =
                        projected_exhaustion_unix_seconds(
                            now_unix_seconds,
                            window.remaining_headroom(),
                            aggregate_burn_basis_points_per_hour,
                        )
                    {
                        fact = fact.with_projected_exhaustion_unix_seconds(
                            projected_exhaustion_unix_seconds,
                        );
                    }
                }
                fact = fact.with_burn_rate_confidence(estimate.confidence);
                windows.push(fact);
            }
        }

        let rejected_windows = input
            .window_rejections()
            .iter()
            .map(|rejection| {
                QuotaWindowRejectionFact::new(
                    rejection.window_kind(),
                    rejection.rejected_at(),
                    rejection.reported_reset(),
                )
            })
            .collect();
        let canonical_responses_windows = input.canonical_responses_windows().map(|windows| {
            windows
                .iter()
                .map(quota_window_fact_from_selector_window)
                .collect::<Vec<_>>()
        });
        let has_allow_compact_canonical_responses_windows = route_band
            == RouteBand::ResponsesCompact.as_str()
            && canonical_responses_windows.is_some();
        let mut projected_account = BurnDownAccountInput::new(
            input.account_id().clone(),
            input.account_label(),
            input.provider(),
            windows,
        )
        .with_rejected_windows(rejected_windows)
        .with_account_enabled(input.account_status() == AccountStatus::Enabled)
        .with_active_credential(active_credential_is_routable(&input))
        .with_current_active_sessions(current_active_sessions)
        .with_canonical_responses_windows(canonical_responses_windows);
        let credit_backed_eligibility = if (route_band == RouteBand::Responses.as_str()
            || has_allow_compact_canonical_responses_windows)
            && input.provider() == Provider::Openai
            && weekly_floor_basis_points.is_none()
            && input.has_current_credit_authority(now_unix_seconds)
        {
            CreditBackedEligibility::Eligible
        } else {
            CreditBackedEligibility::Ineligible
        };
        projected_account =
            projected_account.with_credit_backed_eligibility(credit_backed_eligibility);
        if let Some(floor_basis_points) = weekly_floor_basis_points {
            projected_account =
                projected_account.with_weekly_quota_floor_basis_points(floor_basis_points);
        }
        projected_accounts.push(projected_account);
    }

    Ok(RouteBandSelectionProjection::new(
        projected_accounts,
        account_states,
    ))
}

fn claude_window_facts_from_observations(
    input: &SelectorQuotaInput,
    now_unix_seconds: u64,
) -> Vec<QuotaWindowFact> {
    input
        .window_observations()
        .iter()
        .map(|observation| {
            let freshness = observation.freshness_at(now_unix_seconds);
            let status = match freshness {
                QuotaEvidenceFreshness::Fresh => QuotaWindowStatus::Eligible,
                QuotaEvidenceFreshness::Stale => QuotaWindowStatus::Stale,
                QuotaEvidenceFreshness::Unknown => QuotaWindowStatus::Unknown,
            };
            let mut window =
                QuotaWindowFact::new(window_seconds_for_kind(observation.window_kind()), status)
                    .with_remaining_basis_points(observation.remaining_basis_points())
                    .with_observed_unix_seconds(observation.observation_started_at())
                    .with_effective(true);
            if let Some(reset_unix_seconds) = observation.reset_unix_seconds() {
                window = window.with_reset_unix_seconds(reset_unix_seconds);
            }
            window
        })
        .collect()
}

fn selection_account_state_from_selector_input(
    input: &SelectorQuotaInput,
    weekly_floor_basis_points: Option<u32>,
    now_unix_seconds: u64,
) -> SelectionAccountState {
    if input.account_status() != AccountStatus::Enabled {
        return SelectionAccountState::disabled(input.account_id().clone(), input.provider());
    }

    let window_observations = input
        .window_observations()
        .iter()
        .map(|observation| {
            SelectionWindowObservation::new(
                observation.window_kind(),
                observation.remaining_basis_points(),
                observation
                    .reset_unix_seconds()
                    .map(HeadroomTimestamp::from_unix_seconds),
                observation.observation_started_at(),
                observation.freshness_at(now_unix_seconds),
            )
        })
        .collect();
    let window_rejections = input
        .window_rejections()
        .iter()
        .map(|rejection| {
            SelectionWindowRejection::new(
                rejection.window_kind(),
                rejection.rejected_at(),
                rejection
                    .reported_reset()
                    .map(HeadroomTimestamp::from_unix_seconds),
            )
        })
        .collect::<Vec<_>>();

    let active_credential_needs_login = active_credential_requires_login(input);
    let restriction =
        if input.active_credential_generation().is_none() || active_credential_needs_login {
            SelectionAccountRestriction::NeedsLogin
        } else if input.provider() == Provider::Claude && !window_rejections.is_empty() {
            SelectionAccountRestriction::Exhausted
        } else if let Some(floor_basis_points) = weekly_floor_basis_points {
            match weekly_floor_hold_reason(input, floor_basis_points, now_unix_seconds) {
                Some(reason) => SelectionAccountRestriction::HeldByFloor { reason },
                None => SelectionAccountRestriction::Available,
            }
        } else {
            SelectionAccountRestriction::Available
        };

    SelectionAccountState::enabled(
        input.account_id().clone(),
        input.provider(),
        restriction,
        window_observations,
        window_rejections,
    )
}

fn active_credential_requires_login(input: &SelectorQuotaInput) -> bool {
    input.provider() == Provider::Claude
        && matches!(
            input.active_credential_maintenance_state(),
            Some(
                CredentialMaintenanceState::ReauthRequired
                    | CredentialMaintenanceState::Unrefreshable
            )
        )
}

fn active_credential_is_routable(input: &SelectorQuotaInput) -> bool {
    input.active_credential_generation().is_some() && !active_credential_requires_login(input)
}

fn weekly_floor_hold_reason(
    input: &SelectorQuotaInput,
    floor_basis_points: u32,
    now_unix_seconds: u64,
) -> Option<SelectionHoldReason> {
    let current_weekly_basis_points = if input.provider() == Provider::Claude {
        let Some(weekly_observation) = input
            .window_observations()
            .iter()
            .find(|observation| observation.window_kind() == WindowKind::Weekly)
        else {
            return Some(SelectionHoldReason::WaitingForFreshWeeklyObservation);
        };
        if weekly_observation.freshness_at(now_unix_seconds) != QuotaEvidenceFreshness::Fresh {
            return Some(SelectionHoldReason::WaitingForFreshWeeklyObservation);
        }
        weekly_observation.remaining_basis_points()
    } else {
        let Some(weekly_window) = input
            .windows()
            .iter()
            .find(|window| window.limit_window_seconds() == V1_WEEKLY_WINDOW_SECONDS)
        else {
            return Some(SelectionHoldReason::WaitingForFreshWeeklyObservation);
        };
        if weekly_window.status() != SelectorQuotaWindowStatus::Eligible {
            return Some(SelectionHoldReason::WaitingForFreshWeeklyObservation);
        }
        weekly_window.remaining_headroom().saturating_mul(100)
    };

    (current_weekly_basis_points <= floor_basis_points).then_some(SelectionHoldReason::HardFloor)
}

const fn window_seconds_for_kind(window_kind: WindowKind) -> u64 {
    match window_kind {
        WindowKind::FiveHour => V1_SHORT_WINDOW_SECONDS,
        WindowKind::Weekly => V1_WEEKLY_WINDOW_SECONDS,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ProjectedBurnRateEstimate {
    confidence: QuotaRunRateConfidence,
    per_connection_burn_basis_points_per_hour: Option<u32>,
    aggregate_burn_basis_points_per_hour: Option<u32>,
}

async fn estimate_window_burn_rate(
    state: &(impl AsyncSelectionProjectionRepository + Sync),
    account_id: &AccountId,
    route_band: &str,
    window: &PersistedSelectorQuotaWindow,
    now_unix_seconds: u64,
    refresh_rollups: bool,
) -> Result<ProjectedBurnRateEstimate, StateStoreError> {
    let Some(reset_unix_seconds) = window.reset_unix_seconds() else {
        return Ok(ProjectedBurnRateEstimate {
            confidence: QuotaRunRateConfidence::Unknown,
            per_connection_burn_basis_points_per_hour: None,
            aggregate_burn_basis_points_per_hour: None,
        });
    };
    let mut observations = state
        .quota_history_observations_for_window(
            account_id,
            route_band,
            window.limit_window_seconds(),
            now_unix_seconds.saturating_sub(QUOTA_HISTORY_LOOKBACK_SECONDS),
            now_unix_seconds,
        )
        .await?
        .into_iter()
        .filter(|observation| observation.reset_unix_seconds() == Some(reset_unix_seconds))
        .collect::<Vec<_>>();
    observations.sort_by_key(PersistedQuotaHistoryObservation::observed_unix_seconds);

    let Some(latest_observation) = observations.last() else {
        return Ok(ProjectedBurnRateEstimate {
            confidence: QuotaRunRateConfidence::Unknown,
            per_connection_burn_basis_points_per_hour: None,
            aggregate_burn_basis_points_per_hour: None,
        });
    };
    if now_unix_seconds.saturating_sub(latest_observation.observed_unix_seconds())
        > LEGACY_QUOTA_EVIDENCE_FRESHNESS_SECONDS
    {
        return Ok(ProjectedBurnRateEstimate {
            confidence: QuotaRunRateConfidence::Stale,
            per_connection_burn_basis_points_per_hour: None,
            aggregate_burn_basis_points_per_hour: None,
        });
    }
    if observations.len() == 1 {
        return Ok(ProjectedBurnRateEstimate {
            confidence: QuotaRunRateConfidence::Insufficient,
            per_connection_burn_basis_points_per_hour: None,
            aggregate_burn_basis_points_per_hour: None,
        });
    }

    let Some(first_observation) = observations.first() else {
        return Ok(ProjectedBurnRateEstimate {
            confidence: QuotaRunRateConfidence::Unknown,
            per_connection_burn_basis_points_per_hour: None,
            aggregate_burn_basis_points_per_hour: None,
        });
    };
    let elapsed_seconds = latest_observation
        .observed_unix_seconds()
        .saturating_sub(first_observation.observed_unix_seconds());
    let burned_basis_points = first_observation
        .remaining_headroom()
        .saturating_sub(latest_observation.remaining_headroom())
        .saturating_mul(100);
    let confidence =
        if observations.len() >= 3 && elapsed_seconds >= NORMAL_CONFIDENCE_MIN_SPAN_SECONDS {
            QuotaRunRateConfidence::Normal
        } else {
            QuotaRunRateConfidence::Low
        };
    if elapsed_seconds == 0 {
        return Ok(ProjectedBurnRateEstimate {
            confidence,
            per_connection_burn_basis_points_per_hour: None,
            aggregate_burn_basis_points_per_hour: None,
        });
    }

    if refresh_rollups {
        state
            .refresh_active_session_rollups_for_interval(
                route_band,
                first_observation.observed_unix_seconds(),
                latest_observation.observed_unix_seconds(),
                ACTIVE_SESSION_ROLLUP_BUCKET_SECONDS,
            )
            .await?;
    }
    let rollups = state
        .active_session_rollups_for_route_band(
            route_band,
            first_observation.observed_unix_seconds(),
            latest_observation.observed_unix_seconds(),
        )
        .await?;
    let active_session_seconds = rollups
        .iter()
        .filter(|rollup| rollup.account_id() == account_id)
        .map(|rollup| rollup.active_session_seconds())
        .sum::<u64>();
    let active_session_history_covers_interval = active_session_rollups_cover_interval(
        &rollups,
        account_id,
        first_observation.observed_unix_seconds(),
        latest_observation.observed_unix_seconds(),
    );
    let aggregate_burn_basis_points_per_hour = ceil_div_u128(
        u128::from(burned_basis_points).saturating_mul(3_600),
        u128::from(elapsed_seconds),
    );
    let (confidence, per_connection_burn_basis_points_per_hour) =
        if active_session_seconds > 0 && active_session_history_covers_interval {
            (
                confidence,
                Some(ceil_div_u128(
                    u128::from(burned_basis_points).saturating_mul(3_600),
                    u128::from(active_session_seconds),
                )),
            )
        } else {
            (
                downgrade_confidence_for_missing_active_sessions(confidence),
                None,
            )
        };

    Ok(ProjectedBurnRateEstimate {
        confidence,
        per_connection_burn_basis_points_per_hour: per_connection_burn_basis_points_per_hour
            .map(clamp_u128_to_u32),
        aggregate_burn_basis_points_per_hour: Some(clamp_u128_to_u32(
            aggregate_burn_basis_points_per_hour,
        )),
    })
}

fn active_session_rollups_cover_interval(
    rollups: &[ActiveSessionRollup],
    account_id: &AccountId,
    interval_start_unix_seconds: u64,
    interval_end_unix_seconds: u64,
) -> bool {
    if interval_start_unix_seconds >= interval_end_unix_seconds {
        return true;
    }

    let mut account_rollups = rollups
        .iter()
        .filter(|rollup| rollup.account_id() == account_id)
        .collect::<Vec<_>>();
    account_rollups.sort_by_key(|rollup| {
        (
            rollup.bucket_start_unix_seconds(),
            rollup.bucket_end_unix_seconds(),
        )
    });

    let mut covered_until = interval_start_unix_seconds;
    for rollup in account_rollups {
        if rollup.bucket_end_unix_seconds() <= covered_until {
            continue;
        }
        if rollup.bucket_start_unix_seconds() > covered_until {
            return false;
        }
        covered_until = covered_until.max(rollup.bucket_end_unix_seconds());
        if covered_until >= interval_end_unix_seconds {
            return true;
        }
    }

    false
}

fn downgrade_confidence_for_missing_active_sessions(
    confidence: QuotaRunRateConfidence,
) -> QuotaRunRateConfidence {
    match confidence {
        QuotaRunRateConfidence::Normal => QuotaRunRateConfidence::Low,
        other => other,
    }
}

fn quota_window_fact_from_selector_window(
    window: &PersistedSelectorQuotaWindow,
) -> QuotaWindowFact {
    let status = match window.status() {
        SelectorQuotaWindowStatus::Eligible => QuotaWindowStatus::Eligible,
        SelectorQuotaWindowStatus::Stale => QuotaWindowStatus::Stale,
        SelectorQuotaWindowStatus::Unknown => QuotaWindowStatus::Unknown,
        SelectorQuotaWindowStatus::Ineligible => QuotaWindowStatus::Ineligible,
    };
    let mut fact = QuotaWindowFact::new(window.limit_window_seconds(), status)
        .with_remaining_headroom(window.remaining_headroom())
        .with_observed_unix_seconds(window.observed_unix_seconds())
        .with_effective(window.effective());
    if let Some(reset_unix_seconds) = window.reset_unix_seconds() {
        fact = fact.with_reset_unix_seconds(reset_unix_seconds);
    }

    fact
}

fn ceil_div_u128(numerator: u128, denominator: u128) -> u128 {
    if denominator == 0 {
        return 0;
    }
    numerator.div_ceil(denominator)
}

fn clamp_u128_to_u32(value: u128) -> u32 {
    value.min(u128::from(u32::MAX)) as u32
}

fn projected_exhaustion_unix_seconds(
    now_unix_seconds: u64,
    remaining_headroom_percent: u32,
    burn_rate_basis_points_per_hour: u32,
) -> Option<u64> {
    if burn_rate_basis_points_per_hour == 0 {
        return None;
    }
    let remaining_basis_points = u128::from(remaining_headroom_percent).saturating_mul(100);
    let seconds_until_exhaustion = remaining_basis_points
        .saturating_mul(3_600)
        .checked_div(u128::from(burn_rate_basis_points_per_hour))?;
    Some(now_unix_seconds.saturating_add(seconds_until_exhaustion.min(u128::from(u64::MAX)) as u64))
}

#[cfg(test)]
#[path = "selection_projection/selection_projection_tests.rs"]
mod tests;

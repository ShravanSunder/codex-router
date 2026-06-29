//! Shared SQLx-to-selector projection.

use std::collections::HashMap;

use codex_router_core::ids::AccountId;
use codex_router_selection::burn_down::ACTIVE_SESSION_ROLLUP_BUCKET_SECONDS;
use codex_router_selection::burn_down::BurnDownAccountInput;
use codex_router_selection::burn_down::QuotaWindowFact;
use codex_router_selection::burn_down::QuotaWindowStatus;
use codex_router_selection::run_rate::NORMAL_CONFIDENCE_MIN_SPAN_SECONDS;
use codex_router_selection::run_rate::QuotaRunRateConfidence;
use futures_util::future::BoxFuture;

use crate::account::AccountStatus;
use crate::quota_snapshot::PersistedQuotaHistoryObservation;
use crate::quota_snapshot::PersistedSelectorQuotaWindow;
use crate::quota_snapshot::SelectorQuotaInput;
use crate::quota_snapshot::SelectorQuotaWindowStatus;
use crate::sqlite::ActiveClientCount;
use crate::sqlite::ActiveSessionRollup;
use crate::sqlite::AsyncSqliteStateStore;
use crate::sqlite::StateStoreError;

const QUOTA_HISTORY_LOOKBACK_SECONDS: u64 = 604_800;
const QUOTA_HISTORY_FRESHNESS_SECONDS: u64 = 300;

/// Projected selector inputs for one route band.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RouteBandSelectionProjection {
    accounts: Vec<BurnDownAccountInput>,
}

impl RouteBandSelectionProjection {
    /// Creates a route-band selection projection.
    #[must_use]
    pub fn new(accounts: Vec<BurnDownAccountInput>) -> Self {
        Self { accounts }
    }

    /// Returns projected account selector inputs.
    #[must_use]
    pub fn accounts(&self) -> &[BurnDownAccountInput] {
        &self.accounts
    }
}

/// Async state operations required to project selector inputs.
pub trait AsyncSelectionProjectionRepository {
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
            self.active_client_counts_for_route_band(route_band, now_unix_seconds, max_age_seconds)
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
    project_route_band_selection_inputs_with_active_counts(
        state,
        route_band,
        now_unix_seconds,
        active_client_max_age_seconds,
        None,
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
    let selector_inputs = state
        .selector_inputs_for_route_band(route_band, now_unix_seconds)
        .await?;
    let active_counts = state
        .active_client_counts_for_route_band(
            route_band,
            now_unix_seconds,
            active_client_max_age_seconds,
        )
        .await
        .unwrap_or_default();
    let mut projected_accounts = Vec::with_capacity(selector_inputs.len());

    for input in selector_inputs {
        let current_active_sessions = active_counts
            .iter()
            .find(|count| count.account_id() == input.account_id())
            .map_or(0, |count| count.active_clients());
        let current_active_sessions = active_session_overrides
            .and_then(|overrides| overrides.get(input.account_id()).copied())
            .unwrap_or(current_active_sessions);
        let mut windows = Vec::with_capacity(input.windows().len());
        for window in input.windows() {
            let mut fact = quota_window_fact_from_selector_window(window);
            let projected_active_sessions = current_active_sessions.saturating_add(1);
            let estimate = estimate_window_burn_rate(
                state,
                input.account_id(),
                route_band,
                window,
                now_unix_seconds,
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
                if let Some(projected_exhaustion_unix_seconds) = projected_exhaustion_unix_seconds(
                    now_unix_seconds,
                    window.remaining_headroom(),
                    projected_candidate_burn_basis_points_per_hour,
                ) {
                    fact = fact
                        .with_projected_exhaustion_unix_seconds(projected_exhaustion_unix_seconds);
                }
            } else if let Some(aggregate_burn_basis_points_per_hour) =
                estimate.aggregate_burn_basis_points_per_hour
            {
                fact = fact
                    .with_aggregate_burn_basis_points_per_hour(aggregate_burn_basis_points_per_hour)
                    .with_projected_candidate_burn_basis_points_per_hour(
                        aggregate_burn_basis_points_per_hour,
                    );
                if let Some(projected_exhaustion_unix_seconds) = projected_exhaustion_unix_seconds(
                    now_unix_seconds,
                    window.remaining_headroom(),
                    aggregate_burn_basis_points_per_hour,
                ) {
                    fact = fact
                        .with_projected_exhaustion_unix_seconds(projected_exhaustion_unix_seconds);
                }
            }
            fact = fact.with_burn_rate_confidence(estimate.confidence);
            windows.push(fact);
        }

        projected_accounts.push(
            BurnDownAccountInput::new(input.account_id().clone(), input.account_label(), windows)
                .with_account_enabled(input.account_status() == AccountStatus::Enabled)
                .with_active_credential(input.active_credential_generation().is_some())
                .with_current_active_sessions(current_active_sessions),
        );
    }

    Ok(RouteBandSelectionProjection::new(projected_accounts))
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
        > QUOTA_HISTORY_FRESHNESS_SECONDS
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

    let first_observation = &observations[0];
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

    state
        .refresh_active_session_rollups_for_interval(
            route_band,
            first_observation.observed_unix_seconds(),
            latest_observation.observed_unix_seconds(),
            ACTIVE_SESSION_ROLLUP_BUCKET_SECONDS,
        )
        .await?;
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
mod tests {
    use codex_router_core::ids::AccountId;

    use super::*;

    #[test]
    fn projection_rejects_rollup_gap_inside_quota_interval() {
        let account = AccountId::new("acct_rollup_gap").unwrap_or_else(|error| panic!("{error}"));
        let rollups = vec![
            ActiveSessionRollup::new(account.clone(), "responses", 0, 300, 300, 1),
            ActiveSessionRollup::new(account.clone(), "responses", 600, 900, 300, 1),
        ];

        assert!(
            !active_session_rollups_cover_interval(&rollups, &account, 0, 900),
            "a missing middle rollup bucket must downgrade active-session history"
        );
    }
}

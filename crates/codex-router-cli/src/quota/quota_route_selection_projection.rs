use super::*;

pub(super) fn display_windows_from_selector_input(
    input: &SelectorQuotaInput,
) -> Vec<DisplayQuotaWindow> {
    input
        .windows()
        .iter()
        .map(DisplayQuotaWindow::from_selector_window)
        .collect()
}

pub(super) async fn attach_history_estimates_to_display_windows(
    quota_history_state: &AsyncSqliteStateStore,
    account_id: &AccountId,
    route_band: &str,
    now_unix_seconds: u64,
    windows: &mut [DisplayQuotaWindow],
) -> Result<(), QuotaCommandError> {
    for window in windows {
        let Some(reset_unix_seconds) = window.reset_unix_seconds else {
            continue;
        };
        let observed_from_unix_seconds = now_unix_seconds.saturating_sub(
            quota_status_display_burn_lookback_seconds(window.window_seconds),
        );
        let observations = quota_history_state
            .quota_history_observations_for_window(
                account_id,
                route_band,
                window.window_seconds,
                observed_from_unix_seconds,
                now_unix_seconds,
            )
            .await?;
        let observations = observations
            .iter()
            .filter_map(quota_run_rate_observation_from_history)
            .collect::<Vec<_>>();
        window.run_rate_estimate = display_quota_run_rate_estimate(
            window.window_seconds,
            now_unix_seconds,
            reset_unix_seconds,
            &observations,
        );
    }

    Ok(())
}

pub(super) fn display_quota_run_rate_estimate(
    window_seconds: u64,
    now_unix_seconds: u64,
    reset_unix_seconds: u64,
    observations: &[QuotaRunRateObservation],
) -> QuotaRunRateEstimate {
    let observed_from_unix_seconds =
        now_unix_seconds.saturating_sub(quota_status_display_burn_lookback_seconds(window_seconds));
    let recent_observations = observations
        .iter()
        .copied()
        .filter(|observation| observation.observed_unix_seconds() >= observed_from_unix_seconds)
        .collect::<Vec<_>>();
    if recent_observations.len() < QUOTA_STATUS_DISPLAY_MIN_RATE_SAMPLES {
        return QuotaRunRateEstimate::insufficient();
    }
    let estimate = display_quota_run_rate_estimator().estimate(
        now_unix_seconds,
        reset_unix_seconds,
        &recent_observations,
    );
    if recent_observations.len() < QUOTA_STATUS_DISPLAY_NORMAL_CONFIDENCE_SAMPLES
        && estimate.confidence() == QuotaRunRateConfidence::Normal
        && let (Some(rate), Some(headroom)) = (
            estimate.burn_rate_basis_points_per_hour(),
            estimate.latest_remaining_headroom_percent(),
        )
    {
        return QuotaRunRateEstimate::with_rate_basis_points_per_hour(
            QuotaRunRateConfidence::Low,
            rate,
            headroom,
        );
    }
    estimate
}

pub(super) const fn quota_status_display_burn_lookback_seconds(window_seconds: u64) -> u64 {
    if window_seconds == V1_SHORT_WINDOW_SECONDS {
        QUOTA_STATUS_SHORT_BURN_LOOKBACK_SECONDS
    } else if window_seconds == V1_WEEKLY_WINDOW_SECONDS {
        QUOTA_STATUS_WEEKLY_BURN_LOOKBACK_SECONDS
    } else {
        QUOTA_STATUS_SAMPLE_FRESH_SECONDS
    }
}

pub(super) fn display_quota_run_rate_estimator() -> QuotaRunRateEstimator {
    QuotaRunRateEstimator::new(QUOTA_STATUS_SAMPLE_FRESH_SECONDS)
}

pub(super) fn quota_run_rate_observation_from_history(
    observation: &PersistedQuotaHistoryObservation,
) -> Option<QuotaRunRateObservation> {
    if observation.refresh_outcome() != QuotaHistoryRefreshOutcome::Success {
        return None;
    }
    let reset_unix_seconds = observation.reset_unix_seconds()?;
    Some(QuotaRunRateObservation::new(
        observation.observed_unix_seconds(),
        reset_unix_seconds,
        observation.remaining_headroom(),
    ))
}

pub(super) fn burn_down_input_from_display_windows(
    account: &AccountRecord,
    windows: &[DisplayQuotaWindow],
    now_unix_seconds: u64,
) -> BurnDownAccountInput {
    let facts = windows
        .iter()
        .map(|window| {
            let mut fact = QuotaWindowFact::new(window.window_seconds, window.status)
                .with_remaining_headroom(window.remaining_headroom)
                .with_observed_unix_seconds(window.observed_unix_seconds)
                .with_effective(window.effective);
            if let Some(reset_unix_seconds) = window.reset_unix_seconds {
                fact = fact.with_reset_unix_seconds(reset_unix_seconds);
            }
            if matches!(
                window.run_rate_estimate.confidence(),
                QuotaRunRateConfidence::Low | QuotaRunRateConfidence::Normal
            ) && let Some(projected_exhaustion_unix_seconds) = window
                .run_rate_estimate
                .projected_exhaustion_unix_seconds(now_unix_seconds)
            {
                fact =
                    fact.with_projected_exhaustion_unix_seconds(projected_exhaustion_unix_seconds);
            }
            fact
        })
        .collect::<Vec<_>>();

    BurnDownAccountInput::new(account.account_id().clone(), account.label(), facts)
        .with_account_enabled(account.status() == AccountStatus::Enabled)
        .with_active_credential(account.active_credential_generation().is_some())
}

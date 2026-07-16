use super::*;

pub(super) fn append_success_quota_history_observation(
    runtime: &tokio::runtime::Runtime,
    state: &AsyncSqliteStateStore,
    account: &AccountRecord,
    route_band: &str,
    window: &QuotaRefreshProviderWindow,
    observed_unix_seconds: u64,
    reset_credits_available: Option<u32>,
) -> Result<(), QuotaCommandError> {
    let status = if window.remaining_headroom == 0 {
        SelectorQuotaWindowStatus::Ineligible
    } else {
        SelectorQuotaWindowStatus::Eligible
    };
    let mut observation = PersistedQuotaHistoryObservation::new(
        account.account_id().clone(),
        account.label(),
        route_band,
        window.limit_window_seconds,
        observed_unix_seconds,
        window.remaining_headroom,
    )
    .with_effective(window.effective)
    .with_window_status(status)
    .with_refresh_source(QuotaSnapshotSource::OpenAiEndpoint)
    .with_refresh_outcome(QuotaHistoryRefreshOutcome::Success);
    if let Some(reset_unix_seconds) = window.reset_unix_seconds {
        observation = observation.with_reset_unix_seconds(reset_unix_seconds);
    }
    if let Some(reset_credits_available) = reset_credits_available {
        observation = observation.with_reset_credits_available(reset_credits_available);
    }
    runtime
        .block_on(state.append_quota_history_observation(&observation))
        .map_err(QuotaCommandError::StateStore)
}

pub(super) fn append_failure_quota_history_observations(
    runtime: &tokio::runtime::Runtime,
    state: &AsyncSqliteStateStore,
    account: &AccountRecord,
    route_band: &str,
    observed_unix_seconds: u64,
    error_class: QuotaRefreshErrorClass,
) -> Result<(), QuotaCommandError> {
    for limit_window_seconds in [V1_SHORT_WINDOW_SECONDS, V1_WEEKLY_WINDOW_SECONDS] {
        let observation = PersistedQuotaHistoryObservation::new(
            account.account_id().clone(),
            account.label(),
            route_band,
            limit_window_seconds,
            observed_unix_seconds,
            0,
        )
        .with_window_status(SelectorQuotaWindowStatus::Unknown)
        .with_refresh_source(QuotaSnapshotSource::OpenAiEndpoint)
        .with_refresh_outcome(QuotaHistoryRefreshOutcome::Failure { error_class });
        runtime
            .block_on(state.append_quota_history_observation(&observation))
            .map_err(QuotaCommandError::StateStore)?;
    }
    Ok(())
}

pub(super) fn purge_old_quota_history(
    runtime: &tokio::runtime::Runtime,
    state: &AsyncSqliteStateStore,
    observed_unix_seconds: u64,
) -> Result<(), QuotaCommandError> {
    let retention_floor = observed_unix_seconds.saturating_sub(V1_WEEKLY_WINDOW_SECONDS);
    runtime
        .block_on(state.purge_quota_history_before(retention_floor))
        .map_err(QuotaCommandError::StateStore)
}

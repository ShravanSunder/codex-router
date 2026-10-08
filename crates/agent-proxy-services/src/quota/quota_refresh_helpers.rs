use super::*;
use codex_router_core::provider::Provider;

pub(super) struct SupersededResponsesFloorRead {
    pub(super) attempt_sequence: u64,
    pub(super) weekly_remaining_basis_points: Option<u32>,
}

pub(super) async fn notify_weekly_floor_from_latest_committed_responses(
    state: &AsyncSqliteStateStore,
    account_id: &AccountId,
    expected_credential_generation: u64,
    floor: Option<u32>,
    now_unix_seconds: u64,
    current_read: SupersededResponsesFloorRead,
    observer: &dyn WeeklyQuotaFloorIntentObserver,
) -> Result<(), QuotaRefreshError> {
    let selector_inputs = state
        .selector_inputs_for_route_band(USER_QUOTA_ROUTE_BAND, now_unix_seconds)
        .await?;
    let Some(selector_input) = selector_inputs
        .iter()
        .find(|input| input.account_id() == account_id)
    else {
        return Ok(());
    };
    if selector_input.provider() != Provider::Openai
        || selector_input.account_status() != AccountStatus::Enabled
        || selector_input.active_credential_generation() != Some(expected_credential_generation)
    {
        return Ok(());
    }

    let Some(observation) = selector_input.credit_observation() else {
        return Ok(());
    };
    if observation.credential_generation() != expected_credential_generation {
        return Ok(());
    }
    if observation.committed_attempt() != Some(observation.latest_started_attempt()) {
        if observation.latest_started_attempt() <= current_read.attempt_sequence {
            return Ok(());
        }
        // The newer attempt has no committed selector snapshot yet. Preserve this read's
        // fresh floor signal without writing its quota or credit observations.
        if let Some(intent) =
            weekly_quota_floor_intent(floor, current_read.weekly_remaining_basis_points)
        {
            observer.weekly_quota_floor_intent(account_id, intent);
        }
        return Ok(());
    }

    let Some(observed_unix_seconds) = observation.observed_unix_seconds() else {
        return Ok(());
    };
    let Some(stale_after_unix_seconds) = observation.stale_after_unix_seconds() else {
        return Ok(());
    };
    // A newer manual commit can land while this older refresh cycle waits on provider IO.
    let evaluation_now_unix_seconds = now_unix_seconds.max(observed_unix_seconds);
    if evaluation_now_unix_seconds >= stale_after_unix_seconds {
        return Ok(());
    }

    let latest_successful_windows = selector_input
        .windows()
        .iter()
        .filter(|window| {
            window.observed_unix_seconds() == observed_unix_seconds
                && window.observed_unix_seconds() <= evaluation_now_unix_seconds
                && matches!(
                    window.status(),
                    SelectorQuotaWindowStatus::Eligible | SelectorQuotaWindowStatus::Ineligible
                )
        })
        .collect::<Vec<_>>();
    if latest_successful_windows.is_empty() {
        return Ok(());
    }

    let weekly_remaining_basis_points = latest_successful_windows
        .iter()
        .find(|window| window.limit_window_seconds() == V1_WEEKLY_WINDOW_SECONDS)
        .map(|window| window.remaining_headroom().saturating_mul(100));
    if let Some(intent) = weekly_quota_floor_intent(floor, weekly_remaining_basis_points) {
        observer.weekly_quota_floor_intent(account_id, intent);
    }
    Ok(())
}

pub(super) fn weekly_quota_floor_intent(
    floor: Option<u32>,
    weekly_remaining_basis_points: Option<u32>,
) -> Option<WeeklyQuotaFloorIntent> {
    match (floor, weekly_remaining_basis_points) {
        (None, _) => Some(WeeklyQuotaFloorIntent::Clear),
        (Some(floor), Some(remaining)) if remaining <= floor => {
            Some(WeeklyQuotaFloorIntent::HardStop)
        }
        (Some(floor), Some(remaining))
            if remaining <= weekly_quota_switch_at_basis_points(Some(floor)).unwrap_or(floor) =>
        {
            Some(WeeklyQuotaFloorIntent::GracefulSwitch)
        }
        (Some(_), Some(_)) => Some(WeeklyQuotaFloorIntent::Clear),
        (Some(_), None) => None,
    }
}

pub(super) async fn begin_credit_refresh_attempt_for_current_generation(
    state: &AsyncSqliteStateStore,
    account_id: &AccountId,
    credential_generation: u64,
) -> Result<Option<CreditRefreshAttempt>, QuotaRefreshError> {
    match state
        .begin_credit_refresh_attempt(account_id, credential_generation)
        .await
    {
        Ok(attempt) => Ok(Some(attempt)),
        Err(StateStoreError::AccountConcurrentModification { .. }) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub(super) fn record_superseded_account_refresh(
    stdout: &mut impl Write,
    account: &AccountRecord,
    failed_count: &mut u64,
) -> Result<(), QuotaRefreshError> {
    *failed_count = failed_count.saturating_add(DEFAULT_ROUTE_BANDS.len() as u64);
    let diagnostic_account = quota_refresh_diagnostic_account_label(account);
    writeln!(
        stdout,
        "refresh skipped: account={diagnostic_account} error=credential generation changed during refresh",
    )
    .map_err(QuotaRefreshError::Stdout)
}

pub(super) fn quota_refresh_diagnostic_account_label(account: &AccountRecord) -> String {
    safe_account_label(account.label(), account.account_id())
        .as_str()
        .to_owned()
}

pub(super) fn quota_refresh_error_class(error: &QuotaRefreshError) -> QuotaRefreshErrorClass {
    match error {
        QuotaRefreshError::CredentialResolver(_) => QuotaRefreshErrorClass::AuthError,
        QuotaRefreshError::ProviderRequest { .. } => QuotaRefreshErrorClass::NetworkError,
        QuotaRefreshError::ProviderStatus { status } if *status == 401 || *status == 403 => {
            QuotaRefreshErrorClass::AuthError
        }
        QuotaRefreshError::ProviderStatus { status } if *status == 429 => {
            QuotaRefreshErrorClass::RateLimited
        }
        QuotaRefreshError::ProviderStatus { .. } => QuotaRefreshErrorClass::ProviderError,
        QuotaRefreshError::ProviderResponse { .. } => QuotaRefreshErrorClass::ParseError,
        QuotaRefreshError::CredentialResolverOpen(_)
        | QuotaRefreshError::StateStore(_)
        | QuotaRefreshError::BackgroundWorkerInitialization(_)
        | QuotaRefreshError::Stdout(_) => QuotaRefreshErrorClass::ProviderError,
    }
}

#[cfg(test)]
mod freshness_tests {

    #[test]
    fn active_refresh_freshness_deadline_uses_the_shared_window_policy() {
        let deadline = codex_router_state::window_observation::calculate_window_observation_fresh_until_unix_seconds(100, 400)
            .expect("active refresh deadline should fit timestamp range");

        assert_eq!(deadline, 620);
        assert_eq!(deadline - 100, 520);
    }
}

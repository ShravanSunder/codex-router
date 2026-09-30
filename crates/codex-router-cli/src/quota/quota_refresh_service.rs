use super::*;
use codex_router_core::provider::Provider;
use codex_router_core::route_profile::WindowKind;
use codex_router_state::window_observation::WindowObservation;
use codex_router_state::window_observation::WindowObservationProps;

const DEFAULT_QUOTA_REFRESH_INTERVAL_SECONDS: u64 = 180;
const QUOTA_OBSERVATION_FRESHNESS_MARGIN_SECONDS: u64 = 120;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WeeklyQuotaFloorIntent {
    GracefulSwitch,
    HardStop,
    Clear,
}

pub(crate) trait WeeklyQuotaFloorIntentObserver: Send + Sync {
    fn weekly_quota_floor_intent(&self, account_id: &AccountId, intent: WeeklyQuotaFloorIntent);
}

pub(crate) struct QuotaRefreshObservationContext<'a> {
    pub(crate) observed_unix_seconds: u64,
    pub(crate) schedule: QuotaRefreshSchedule,
    pub(crate) weekly_floor_observer: Option<&'a dyn WeeklyQuotaFloorIntentObserver>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum QuotaRefreshSchedule {
    Manual,
    Background { interval_seconds: u64 },
}

impl QuotaRefreshSchedule {
    const fn interval_seconds(self) -> u64 {
        match self {
            Self::Manual => DEFAULT_QUOTA_REFRESH_INTERVAL_SECONDS,
            Self::Background { interval_seconds } => interval_seconds,
        }
    }

    const fn polls_only_idle_claude_accounts(self) -> bool {
        matches!(self, Self::Background { .. })
    }
}

impl WeeklyQuotaFloorIntentObserver for WebSocketQuotaFloorNotifier {
    fn weekly_quota_floor_intent(&self, account_id: &AccountId, intent: WeeklyQuotaFloorIntent) {
        match intent {
            WeeklyQuotaFloorIntent::GracefulSwitch => {
                self.request_weekly_quota_floor_switch(account_id);
            }
            WeeklyQuotaFloorIntent::HardStop => {
                self.signal_weekly_quota_floor_reached(account_id);
            }
            WeeklyQuotaFloorIntent::Clear => {
                self.clear_weekly_quota_floor_switch(account_id);
            }
        }
    }
}

pub(crate) async fn refresh_quota_with_dependencies<R, P>(
    stdout: &mut impl Write,
    router_root: PathBuf,
    base_url: String,
    credential_resolver: &R,
    quota_provider: &P,
    observed_unix_seconds: u64,
) -> Result<(), QuotaCommandError>
where
    R: AsyncProviderCredentialResolver,
    P: QuotaRefreshProvider,
{
    refresh_quota_store_paths_with_dependencies(
        stdout,
        &router_root.join("state.sqlite"),
        &router_root.join("secrets"),
        base_url,
        credential_resolver,
        quota_provider,
        observed_unix_seconds,
    )
    .await
}

pub(crate) async fn refresh_quota_store_paths_with_dependencies<R, P>(
    stdout: &mut impl Write,
    state_db: &Path,
    _secret_root: &Path,
    base_url: String,
    credential_resolver: &R,
    quota_provider: &P,
    observed_unix_seconds: u64,
) -> Result<(), QuotaCommandError>
where
    R: AsyncProviderCredentialResolver,
    P: QuotaRefreshProvider,
{
    refresh_quota_store_paths_with_dependencies_and_floor_notifier(
        stdout,
        state_db,
        _secret_root,
        base_url,
        credential_resolver,
        quota_provider,
        QuotaRefreshObservationContext {
            observed_unix_seconds,
            schedule: QuotaRefreshSchedule::Manual,
            weekly_floor_observer: None,
        },
    )
    .await
}

pub(crate) async fn refresh_quota_store_paths_with_dependencies_and_floor_notifier<R, P>(
    stdout: &mut impl Write,
    state_db: &Path,
    _secret_root: &Path,
    base_url: String,
    credential_resolver: &R,
    quota_provider: &P,
    observation_context: QuotaRefreshObservationContext<'_>,
) -> Result<(), QuotaCommandError>
where
    R: AsyncProviderCredentialResolver,
    P: QuotaRefreshProvider,
{
    let QuotaRefreshObservationContext {
        observed_unix_seconds,
        schedule,
        weekly_floor_observer,
    } = observation_context;
    let refresh_interval_seconds = schedule.interval_seconds();
    let quota_history_state = AsyncSqliteStateStore::open(state_db).await?;
    let accounts = quota_history_state.list_accounts().await?;
    let active_claude_accounts = if schedule.polls_only_idle_claude_accounts() {
        quota_history_state
            .active_client_counts_for_route_band_read_only(
                "claude_messages",
                observed_unix_seconds,
                ACTIVE_CLIENT_LEASE_MAX_AGE_SECONDS,
            )
            .await?
            .into_iter()
            .filter(|count| count.active_clients() > 0)
            .map(|count| count.account_id().clone())
            .collect::<std::collections::HashSet<_>>()
    } else {
        std::collections::HashSet::new()
    };
    let weekly_quota_floors = quota_history_state
        .list_account_routing_policies()
        .await?
        .into_iter()
        .map(|policy| {
            (
                policy.account_id().clone(),
                u32::from(policy.weekly_quota_floor_basis_points().basis_points()),
            )
        })
        .collect::<HashMap<_, _>>();
    let mut refreshed_count = 0_u64;
    let mut failed_count = 0_u64;
    for account in accounts
        .iter()
        .filter(|account| account.status() == AccountStatus::Enabled)
        .filter(|account| account.active_credential_generation().is_some())
    {
        if schedule.polls_only_idle_claude_accounts() && account.provider() == Provider::Claude {
            if active_claude_accounts.contains(account.account_id()) {
                continue;
            }
            let latest_activity = quota_history_state
                .latest_active_session_activity_unix_seconds(
                    account.account_id(),
                    "claude_messages",
                )
                .await?;
            let account_has_been_idle_long_enough = match latest_activity {
                None => true,
                Some(last_activity_unix_seconds) => {
                    observed_unix_seconds >= last_activity_unix_seconds
                        && observed_unix_seconds - last_activity_unix_seconds
                            > refresh_interval_seconds
                }
            };
            if !account_has_been_idle_long_enough {
                continue;
            }
        }
        let route_bands: &[&str] = if account.provider() == Provider::Claude {
            &["claude_messages"]
        } else {
            DEFAULT_ROUTE_BANDS
        };
        let mut resolved = match credential_resolver
            .resolve_provider_credentials_async(account.account_id(), account.provider())
            .await
        {
            Ok(resolved) => resolved,
            Err(error) => {
                failed_count = failed_count.saturating_add(route_bands.len() as u64);
                for route_band in route_bands {
                    quota_history_state
                        .record_refresh_failure_preserving_selector_windows(
                            account.account_id(),
                            route_band,
                            observed_unix_seconds,
                            QuotaRefreshErrorClass::AuthError,
                        )
                        .await?;
                    append_failure_quota_history_observations(
                        &quota_history_state,
                        account,
                        route_band,
                        observed_unix_seconds,
                        QuotaRefreshErrorClass::AuthError,
                    )
                    .await?;
                }
                tracing::warn!(
                    account.hash = telemetry_hash(account.account_id().as_str()),
                    route_band = "*",
                    error.class = QuotaRefreshErrorClass::AuthError.as_str(),
                    "codex_router.quota_refresh_failed"
                );
                record_quota_refresh_metric(
                    "*",
                    "failure",
                    QuotaRefreshErrorClass::AuthError.as_str(),
                );
                let diagnostic_account = quota_refresh_diagnostic_account_label(account);
                writeln!(
                    stdout,
                    "refresh failed: account={diagnostic_account} route_band=* error={error}",
                )
                .map_err(QuotaCommandError::Stdout)?;
                continue;
            }
        };
        if account.provider() == Provider::Claude {
            let observation_started_at = current_unix_seconds();
            let first_response = quota_provider
                .fetch_quota(QuotaRefreshProviderRequest::new_for_provider(
                    Provider::Claude,
                    account.account_id().clone(),
                    account.label(),
                    "claude_messages",
                    base_url.clone(),
                    resolved.access_token().clone(),
                    resolved.chatgpt_account_id(),
                ))
                .await;
            let initial_unauthorized = matches!(
                &first_response,
                Err(QuotaCommandError::ProviderStatus { status: 401 })
            );
            let (response, retry_generation, renewed_for_retry) = if initial_unauthorized {
                match credential_resolver
                    .recover_unauthorized_credentials_async(
                        account.account_id(),
                        Provider::Claude,
                        resolved.credential_generation(),
                    )
                    .await
                {
                    Ok((recovered, renewed_here)) => {
                        let retry_generation = Some(recovered.credential_generation());
                        let retry_response = quota_provider
                            .fetch_quota(QuotaRefreshProviderRequest::new_for_provider(
                                Provider::Claude,
                                account.account_id().clone(),
                                account.label(),
                                "claude_messages",
                                base_url.clone(),
                                recovered.access_token().clone(),
                                recovered.chatgpt_account_id(),
                            ))
                            .await;
                        (retry_response, retry_generation, renewed_here)
                    }
                    Err(error) => (
                        Err(QuotaCommandError::CredentialResolver(error)),
                        None,
                        false,
                    ),
                }
            } else {
                (first_response, None, false)
            };
            let response = match response {
                Ok(response) => response,
                Err(error) => {
                    failed_count = failed_count.saturating_add(1);
                    let error_class = quota_refresh_error_class(&error);
                    quota_history_state
                        .record_refresh_failure_preserving_selector_windows(
                            account.account_id(),
                            "claude_messages",
                            observed_unix_seconds,
                            error_class,
                        )
                        .await?;
                    let provider_rejected_credentials =
                        matches!(error, QuotaCommandError::ProviderStatus { status: 401 });
                    if provider_rejected_credentials
                        && renewed_for_retry
                        && let Some(retry_generation) = retry_generation
                    {
                        tracing::warn!(
                            account_id = account.account_id().as_str(),
                            credential_generation = retry_generation,
                            http.status_code = 401,
                            endpoint.path = "/api/oauth/usage",
                            "Claude usage endpoint rejected a freshly renewed credential"
                        );
                        record_claude_usage_auth_rejected_after_renewal();
                    }
                    let diagnostic_error_class = if provider_rejected_credentials {
                        "provider_auth_rejected"
                    } else {
                        error_class.as_str()
                    };
                    tracing::warn!(
                        account.hash = telemetry_hash(account.account_id().as_str()),
                        route_band = "claude_messages",
                        error.class = diagnostic_error_class,
                        "codex_router.claude_quota_refresh_failed"
                    );
                    record_quota_refresh_metric(
                        "claude_messages",
                        "failure",
                        diagnostic_error_class,
                    );
                    let diagnostic_account = quota_refresh_diagnostic_account_label(account);
                    writeln!(
                        stdout,
                        "refresh failed: account={diagnostic_account} route_band=claude_messages error={error}",
                    )
                    .map_err(QuotaCommandError::Stdout)?;
                    continue;
                }
            };
            let mut observations_recorded = 0_u64;
            let fresh_until_unix_seconds =
                quota_observation_fresh_until(observation_started_at, refresh_interval_seconds)?;
            for window in response.windows {
                let window_kind = match window.limit_window_seconds {
                    18_000 => WindowKind::FiveHour,
                    604_800 => WindowKind::Weekly,
                    _ => continue,
                };
                let mut properties = WindowObservationProps::new(
                    account.account_id().clone(),
                    window_kind,
                    window.headroom.basis_points().ok_or_else(|| {
                        QuotaCommandError::ProviderResponse {
                            message: "Claude quota window was not expressed in basis points"
                                .to_owned(),
                        }
                    })?,
                    observation_started_at,
                );
                if let Some(reset_unix_seconds) = window.reset_unix_seconds {
                    properties = properties.with_reset_unix_seconds(reset_unix_seconds);
                }
                properties = properties.with_fresh_until_unix_seconds(fresh_until_unix_seconds);
                let observation = WindowObservation::new(properties)?;
                if quota_history_state
                    .record_window_observation(&observation, current_unix_seconds)
                    .await?
                {
                    observations_recorded = observations_recorded.saturating_add(1);
                }
            }
            if observations_recorded > 0 {
                refreshed_count = refreshed_count.saturating_add(1);
            }
            continue;
        }
        for route_band in DEFAULT_ROUTE_BANDS {
            let first_response = quota_provider
                .fetch_quota(QuotaRefreshProviderRequest::new_for_provider(
                    account.provider(),
                    account.account_id().clone(),
                    account.label(),
                    *route_band,
                    base_url.clone(),
                    resolved.access_token().clone(),
                    resolved.chatgpt_account_id(),
                ))
                .await;
            let initial_unauthorized = matches!(
                &first_response,
                Err(QuotaCommandError::ProviderStatus { status: 401 })
            );
            let (response, retry_generation, renewed_for_retry) = if initial_unauthorized {
                match credential_resolver
                    .recover_unauthorized_credentials_async(
                        account.account_id(),
                        account.provider(),
                        resolved.credential_generation(),
                    )
                    .await
                {
                    Ok((recovered, renewed_here)) => {
                        let retry_generation = Some(recovered.credential_generation());
                        let retry_response = quota_provider
                            .fetch_quota(QuotaRefreshProviderRequest::new_for_provider(
                                account.provider(),
                                account.account_id().clone(),
                                account.label(),
                                *route_band,
                                base_url.clone(),
                                recovered.access_token().clone(),
                                recovered.chatgpt_account_id(),
                            ))
                            .await;
                        resolved = recovered;
                        (retry_response, retry_generation, renewed_here)
                    }
                    Err(error) => (
                        Err(QuotaCommandError::CredentialResolver(error)),
                        None,
                        false,
                    ),
                }
            } else {
                (first_response, None, false)
            };
            let response = match response {
                Ok(response) => response,
                Err(error) => {
                    failed_count = failed_count.saturating_add(1);
                    let error_class = quota_refresh_error_class(&error);
                    quota_history_state
                        .record_refresh_failure_preserving_selector_windows(
                            account.account_id(),
                            route_band,
                            observed_unix_seconds,
                            error_class,
                        )
                        .await?;
                    append_failure_quota_history_observations(
                        &quota_history_state,
                        account,
                        route_band,
                        observed_unix_seconds,
                        error_class,
                    )
                    .await?;
                    let provider_rejected_credentials =
                        matches!(error, QuotaCommandError::ProviderStatus { status: 401 });
                    if provider_rejected_credentials && renewed_for_retry {
                        quota_history_state
                            .disable_account_if_credential_generation_current(
                                account.account_id(),
                                retry_generation.unwrap_or(resolved.credential_generation()),
                            )
                            .await?;
                    }
                    let diagnostic_error_class = if provider_rejected_credentials {
                        "provider_auth_rejected"
                    } else {
                        error_class.as_str()
                    };
                    tracing::warn!(
                        account.hash = telemetry_hash(account.account_id().as_str()),
                        route_band,
                        error.class = diagnostic_error_class,
                        "codex_router.quota_refresh_failed"
                    );
                    record_quota_refresh_metric(route_band, "failure", diagnostic_error_class);
                    let diagnostic_account = quota_refresh_diagnostic_account_label(account);
                    writeln!(
                        stdout,
                        "refresh failed: account={diagnostic_account} route_band={route_band} error={error}",
                    )
                    .map_err(QuotaCommandError::Stdout)?;
                    if provider_rejected_credentials || initial_unauthorized {
                        break;
                    }
                    continue;
                }
            };
            let effective_window = match response.effective_window() {
                Some(effective_window) => effective_window,
                None => {
                    failed_count = failed_count.saturating_add(1);
                    quota_history_state
                        .record_refresh_failure_preserving_selector_windows(
                            account.account_id(),
                            route_band,
                            observed_unix_seconds,
                            QuotaRefreshErrorClass::ParseError,
                        )
                        .await?;
                    append_failure_quota_history_observations(
                        &quota_history_state,
                        account,
                        route_band,
                        observed_unix_seconds,
                        QuotaRefreshErrorClass::ParseError,
                    )
                    .await?;
                    tracing::warn!(
                        account.hash = telemetry_hash(account.account_id().as_str()),
                        route_band,
                        error.class = QuotaRefreshErrorClass::ParseError.as_str(),
                        "codex_router.quota_refresh_failed"
                    );
                    record_quota_refresh_metric(
                        route_band,
                        "failure",
                        QuotaRefreshErrorClass::ParseError.as_str(),
                    );
                    let diagnostic_account = quota_refresh_diagnostic_account_label(account);
                    writeln!(
                        stdout,
                        "refresh failed: account={diagnostic_account} route_band={route_band} error=missing provider quota windows",
                    )
                    .map_err(QuotaCommandError::Stdout)?;
                    continue;
                }
            };
            let mut selector_windows = Vec::new();
            for window in &response.windows {
                let remaining_headroom = window.headroom.percent().ok_or_else(|| {
                    QuotaCommandError::ProviderResponse {
                        message: "OpenAI quota window was not expressed as a percent".to_owned(),
                    }
                })?;
                let status = if remaining_headroom == 0 {
                    SelectorQuotaWindowStatus::Ineligible
                } else {
                    SelectorQuotaWindowStatus::Eligible
                };
                let selector_window = PersistedSelectorQuotaWindow::new(
                    account.account_id().clone(),
                    *route_band,
                    window.limit_window_seconds,
                    status,
                )
                .with_remaining_headroom(remaining_headroom)
                .with_effective(window.effective)
                .with_observed_unix_seconds(observed_unix_seconds);
                let selector_window = if let Some(reset_unix_seconds) = window.reset_unix_seconds {
                    selector_window.with_reset_unix_seconds(reset_unix_seconds)
                } else {
                    selector_window
                };
                selector_windows.push(selector_window);
                append_success_quota_history_observation(
                    &quota_history_state,
                    account,
                    route_band,
                    window,
                    observed_unix_seconds,
                    response.reset_credits_available,
                )
                .await?;
            }
            quota_history_state
                .record_refresh_success_and_replace_selector_windows(
                    account.account_id(),
                    route_band,
                    &selector_windows,
                    observed_unix_seconds,
                    stale_after_unix_seconds(observed_unix_seconds),
                )
                .await?;
            if *route_band == USER_QUOTA_ROUTE_BAND
                && let Some(observer) = weekly_floor_observer
            {
                let floor = weekly_quota_floors.get(account.account_id()).copied();
                let weekly_remaining_basis_points = match response
                    .windows
                    .iter()
                    .find(|window| window.limit_window_seconds == V1_WEEKLY_WINDOW_SECONDS)
                {
                    Some(window) => Some(
                        window
                            .headroom
                            .percent()
                            .ok_or_else(|| QuotaCommandError::ProviderResponse {
                                message: "OpenAI quota window was not expressed as a percent"
                                    .to_owned(),
                            })?
                            .saturating_mul(100),
                    ),
                    None => None,
                };
                let intent = match (floor, weekly_remaining_basis_points) {
                    (None, _) => Some(WeeklyQuotaFloorIntent::Clear),
                    (Some(floor), Some(remaining)) if remaining <= floor => {
                        Some(WeeklyQuotaFloorIntent::HardStop)
                    }
                    (Some(floor), Some(remaining))
                        if remaining
                            <= weekly_quota_switch_at_basis_points(Some(floor))
                                .unwrap_or(floor) =>
                    {
                        Some(WeeklyQuotaFloorIntent::GracefulSwitch)
                    }
                    (Some(_), Some(_)) => Some(WeeklyQuotaFloorIntent::Clear),
                    (Some(_), None) => None,
                };
                if let Some(intent) = intent {
                    observer.weekly_quota_floor_intent(account.account_id(), intent);
                }
            }
            let snapshot = PersistedQuotaSnapshot::new(
                account.account_id().clone(),
                QuotaSnapshotSource::OpenAiEndpoint,
            )
            .with_observed_unix_seconds(observed_unix_seconds)
            .with_route_band(
                *route_band,
                effective_window.headroom.percent().ok_or_else(|| {
                    QuotaCommandError::ProviderResponse {
                        message: "OpenAI quota window was not expressed as a percent".to_owned(),
                    }
                })?,
            )
            .with_stale_penalty(false);
            let snapshot = if let Some(reset_unix_seconds) = effective_window.reset_unix_seconds {
                snapshot.with_reset_unix_seconds(reset_unix_seconds)
            } else {
                snapshot
            };
            let snapshot = if let Some(reset_credits_available) = response.reset_credits_available {
                snapshot.with_reset_credits_available(reset_credits_available)
            } else {
                snapshot
            };
            quota_history_state
                .upsert_quota_snapshot_preserving_selector_windows(&snapshot)
                .await?;
            tracing::info!(
                account.hash = telemetry_hash(account.account_id().as_str()),
                route_band,
                windows = selector_windows.len(),
                reset_credits.available = response.reset_credits_available,
                "codex_router.quota_refresh_succeeded"
            );
            record_quota_refresh_metric(route_band, "success", "none");
            refreshed_count = refreshed_count.saturating_add(1);
        }
    }
    purge_old_quota_history(&quota_history_state, observed_unix_seconds).await?;

    writeln!(stdout, "refreshed: {refreshed_count}").map_err(QuotaCommandError::Stdout)?;
    if failed_count > 0 {
        writeln!(stdout, "failed: {failed_count}").map_err(QuotaCommandError::Stdout)?;
    }
    let refresh_result = if refreshed_count == 0 && failed_count > 0 {
        Err(QuotaCommandError::ProviderResponse {
            message: "quota refresh failed for all eligible route bands".to_owned(),
        })
    } else {
        Ok(())
    };
    quota_history_state.close().await?;

    refresh_result
}

fn quota_observation_fresh_until(
    observation_started_at: u64,
    refresh_interval_seconds: u64,
) -> Result<u64, QuotaCommandError> {
    observation_started_at
        .checked_add(refresh_interval_seconds)
        .and_then(|deadline| deadline.checked_add(QUOTA_OBSERVATION_FRESHNESS_MARGIN_SECONDS))
        .ok_or_else(|| QuotaCommandError::ProviderResponse {
            message: "quota observation freshness deadline exceeded timestamp range".to_owned(),
        })
}

fn quota_refresh_diagnostic_account_label(account: &AccountRecord) -> String {
    safe_account_label(account.label(), account.account_id())
        .as_str()
        .to_owned()
}

fn quota_refresh_error_class(error: &QuotaCommandError) -> QuotaRefreshErrorClass {
    match error {
        QuotaCommandError::CredentialResolver(_) => QuotaRefreshErrorClass::AuthError,
        QuotaCommandError::ProviderRequest { .. } => QuotaRefreshErrorClass::NetworkError,
        QuotaCommandError::ProviderStatus { status } if *status == 401 || *status == 403 => {
            QuotaRefreshErrorClass::AuthError
        }
        QuotaCommandError::ProviderStatus { status } if *status == 429 => {
            QuotaRefreshErrorClass::RateLimited
        }
        QuotaCommandError::ProviderStatus { .. } => QuotaRefreshErrorClass::ProviderError,
        QuotaCommandError::ProviderResponse { .. } => QuotaRefreshErrorClass::ParseError,
        QuotaCommandError::ResetComposition(_)
        | QuotaCommandError::ResetSessionTaskFailed
        | QuotaCommandError::AsyncDispatchRequired
        | QuotaCommandError::InvalidFormat { .. }
        | QuotaCommandError::DisallowedBaseUrl { .. }
        | QuotaCommandError::RefreshNotImplemented
        | QuotaCommandError::CredentialResolverOpen(_)
        | QuotaCommandError::StateStore(_)
        | QuotaCommandError::BackgroundWorkerInitialization(_)
        | QuotaCommandError::Stdout(_) => QuotaRefreshErrorClass::ProviderError,
    }
}

#[cfg(test)]
mod freshness_tests {
    use super::*;

    #[test]
    fn freshness_deadline_adds_the_configured_interval_and_margin() {
        assert!(matches!(quota_observation_fresh_until(100, 400), Ok(620)));
    }
}

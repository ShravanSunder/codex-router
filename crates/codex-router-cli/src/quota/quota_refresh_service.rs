use super::*;

pub(crate) trait WeeklyQuotaFloorReachedObserver: Send + Sync {
    fn weekly_quota_floor_reached(&self, account_id: &AccountId);
}

pub(crate) struct QuotaRefreshObservationContext<'a> {
    pub(crate) observed_unix_seconds: u64,
    pub(crate) weekly_floor_observer: Option<&'a dyn WeeklyQuotaFloorReachedObserver>,
}

impl WeeklyQuotaFloorReachedObserver for WebSocketQuotaFloorNotifier {
    fn weekly_quota_floor_reached(&self, account_id: &AccountId) {
        self.signal_weekly_quota_floor_reached(account_id);
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
        weekly_floor_observer,
    } = observation_context;
    let quota_history_state = AsyncSqliteStateStore::open(state_db).await?;
    let accounts = quota_history_state.list_accounts().await?;
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
    let mut weekly_floor_accounts = Vec::<AccountId>::new();
    for account in accounts
        .iter()
        .filter(|account| account.status() == AccountStatus::Enabled)
        .filter(|account| account.active_credential_generation().is_some())
    {
        let resolved = match credential_resolver
            .resolve_provider_credentials_async(account.account_id())
            .await
        {
            Ok(resolved) => resolved,
            Err(error) => {
                failed_count = failed_count.saturating_add(DEFAULT_ROUTE_BANDS.len() as u64);
                for route_band in DEFAULT_ROUTE_BANDS {
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
        for route_band in DEFAULT_ROUTE_BANDS {
            let response = match quota_provider
                .fetch_quota(QuotaRefreshProviderRequest::new(
                    account.account_id().clone(),
                    account.label(),
                    *route_band,
                    base_url.clone(),
                    resolved.access_token().clone(),
                    resolved.chatgpt_account_id(),
                ))
                .await
            {
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
                    if provider_rejected_credentials {
                        quota_history_state
                            .disable_account_if_credential_generation_current(
                                account.account_id(),
                                resolved.credential_generation(),
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
                    if provider_rejected_credentials {
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
            let snapshot = PersistedQuotaSnapshot::new(
                account.account_id().clone(),
                QuotaSnapshotSource::OpenAiEndpoint,
            )
            .with_observed_unix_seconds(observed_unix_seconds)
            .with_route_band(*route_band, effective_window.remaining_headroom)
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
            quota_history_state.upsert_quota_snapshot(&snapshot).await?;
            let mut selector_windows = Vec::new();
            for window in &response.windows {
                let status = if window.remaining_headroom == 0 {
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
                .with_remaining_headroom(window.remaining_headroom)
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
                && let Some(floor_basis_points) =
                    weekly_quota_floors.get(account.account_id()).copied()
                && response.windows.iter().any(|window| {
                    window.limit_window_seconds == V1_WEEKLY_WINDOW_SECONDS
                        && window.remaining_headroom.saturating_mul(100) <= floor_basis_points
                })
                && !weekly_floor_accounts
                    .iter()
                    .any(|account_id| account_id == account.account_id())
            {
                weekly_floor_accounts.push(account.account_id().clone());
            }
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
    if let Some(weekly_floor_observer) = weekly_floor_observer {
        for account_id in &weekly_floor_accounts {
            weekly_floor_observer.weekly_quota_floor_reached(account_id);
        }
    }

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

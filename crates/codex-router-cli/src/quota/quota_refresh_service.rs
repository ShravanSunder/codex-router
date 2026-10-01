use super::*;
use codex_router_core::ids::AccountId;

#[derive(Clone, Debug, Default)]
pub(crate) struct QuotaRefreshReport {
    committed_responses_generations: HashMap<AccountId, u64>,
}

impl QuotaRefreshReport {
    pub(crate) fn responses_observation_committed_for(
        &self,
        account_id: &AccountId,
        credential_generation: Option<u64>,
    ) -> bool {
        credential_generation.is_some_and(|generation| {
            self.committed_responses_generations.get(account_id) == Some(&generation)
        })
    }
}

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
    pub(crate) weekly_floor_observer: Option<&'a dyn WeeklyQuotaFloorIntentObserver>,
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
) -> Result<QuotaRefreshReport, QuotaCommandError>
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
) -> Result<QuotaRefreshReport, QuotaCommandError>
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
) -> Result<QuotaRefreshReport, QuotaCommandError>
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
    let mut committed_responses_generations = HashMap::new();
    'accounts: for account in accounts
        .iter()
        .filter(|account| account.status() == AccountStatus::Enabled)
        .filter(|account| account.active_credential_generation().is_some())
    {
        let Some(active_credential_generation) = account.active_credential_generation() else {
            continue;
        };
        let mut responses_credit_attempt =
            match begin_credit_refresh_attempt_for_current_generation(
                &quota_history_state,
                account.account_id(),
                active_credential_generation,
            )
            .await?
            {
                Some(attempt) => Some(attempt),
                None => {
                    record_superseded_account_refresh(&mut *stdout, account, &mut failed_count)?;
                    continue 'accounts;
                }
            };
        let mut resolved = match credential_resolver
            .resolve_provider_credentials_async(account.account_id())
            .await
        {
            Ok(resolved) => {
                if responses_credit_attempt.as_ref().is_some_and(|attempt| {
                    attempt.credential_generation() != resolved.credential_generation()
                }) {
                    responses_credit_attempt =
                        match begin_credit_refresh_attempt_for_current_generation(
                            &quota_history_state,
                            account.account_id(),
                            resolved.credential_generation(),
                        )
                        .await?
                        {
                            Some(attempt) => Some(attempt),
                            None => {
                                record_superseded_account_refresh(
                                    &mut *stdout,
                                    account,
                                    &mut failed_count,
                                )?;
                                continue 'accounts;
                            }
                        };
                }
                resolved
            }
            Err(error) => {
                failed_count = failed_count.saturating_add(DEFAULT_ROUTE_BANDS.len() as u64);
                for route_band in DEFAULT_ROUTE_BANDS {
                    if *route_band == USER_QUOTA_ROUTE_BAND {
                        let Some(attempt) = responses_credit_attempt.as_ref() else {
                            return Err(QuotaCommandError::ProviderResponse {
                                message: "Responses refresh attempt was not allocated".to_owned(),
                            });
                        };
                        quota_history_state
                            .record_responses_refresh_failure(
                                attempt,
                                observed_unix_seconds,
                                QuotaRefreshErrorClass::AuthError,
                                &failure_quota_history_observations(
                                    account,
                                    route_band,
                                    observed_unix_seconds,
                                    QuotaRefreshErrorClass::AuthError,
                                ),
                            )
                            .await?;
                    } else {
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
            let mut credit_refresh_attempt = if *route_band == USER_QUOTA_ROUTE_BAND {
                let prepared_attempt = responses_credit_attempt.take();
                Some(match prepared_attempt {
                    Some(attempt)
                        if attempt.credential_generation() == resolved.credential_generation() =>
                    {
                        attempt
                    }
                    _ => {
                        match begin_credit_refresh_attempt_for_current_generation(
                            &quota_history_state,
                            account.account_id(),
                            resolved.credential_generation(),
                        )
                        .await?
                        {
                            Some(attempt) => attempt,
                            None => {
                                record_superseded_account_refresh(
                                    &mut *stdout,
                                    account,
                                    &mut failed_count,
                                )?;
                                continue 'accounts;
                            }
                        }
                    }
                })
            } else {
                None
            };
            let first_response = quota_provider
                .fetch_quota(QuotaRefreshProviderRequest::new(
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
                        resolved.credential_generation(),
                    )
                    .await
                {
                    Ok((recovered, renewed_here)) => {
                        let retry_generation = Some(recovered.credential_generation());
                        if *route_band == USER_QUOTA_ROUTE_BAND {
                            credit_refresh_attempt =
                                match begin_credit_refresh_attempt_for_current_generation(
                                    &quota_history_state,
                                    account.account_id(),
                                    recovered.credential_generation(),
                                )
                                .await?
                                {
                                    Some(attempt) => Some(attempt),
                                    None => {
                                        record_superseded_account_refresh(
                                            &mut *stdout,
                                            account,
                                            &mut failed_count,
                                        )?;
                                        continue 'accounts;
                                    }
                                };
                        }
                        let retry_response = quota_provider
                            .fetch_quota(QuotaRefreshProviderRequest::new(
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
                    if *route_band == USER_QUOTA_ROUTE_BAND {
                        let Some(attempt) = credit_refresh_attempt.as_ref() else {
                            return Err(QuotaCommandError::ProviderResponse {
                                message: "Responses refresh attempt was not allocated".to_owned(),
                            });
                        };
                        quota_history_state
                            .record_responses_refresh_failure(
                                attempt,
                                observed_unix_seconds,
                                error_class,
                                &failure_quota_history_observations(
                                    account,
                                    route_band,
                                    observed_unix_seconds,
                                    error_class,
                                ),
                            )
                            .await?;
                    } else {
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
                    }
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
                    if *route_band == USER_QUOTA_ROUTE_BAND {
                        let Some(attempt) = credit_refresh_attempt.as_ref() else {
                            return Err(QuotaCommandError::ProviderResponse {
                                message: "Responses refresh attempt was not allocated".to_owned(),
                            });
                        };
                        quota_history_state
                            .record_responses_refresh_failure(
                                attempt,
                                observed_unix_seconds,
                                QuotaRefreshErrorClass::ParseError,
                                &failure_quota_history_observations(
                                    account,
                                    route_band,
                                    observed_unix_seconds,
                                    QuotaRefreshErrorClass::ParseError,
                                ),
                            )
                            .await?;
                    } else {
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
                    }
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
            let mut responses_history_observations = Vec::new();
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
                if *route_band == USER_QUOTA_ROUTE_BAND {
                    responses_history_observations.push(success_quota_history_observation(
                        account,
                        route_band,
                        window,
                        observed_unix_seconds,
                        response.reset_credits_available,
                    ));
                } else {
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
            }
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
            let refresh_was_committed = if *route_band == USER_QUOTA_ROUTE_BAND {
                let Some(attempt) = credit_refresh_attempt.as_ref() else {
                    return Err(QuotaCommandError::ProviderResponse {
                        message: "Responses refresh attempt was not allocated".to_owned(),
                    });
                };
                quota_history_state
                    .record_responses_refresh_success(
                        codex_router_state::credit_store::ResponsesRefreshSuccessCommit {
                            attempt,
                            selector_windows: &selector_windows,
                            observed_unix_seconds,
                            stale_after_unix_seconds: stale_after_unix_seconds(
                                observed_unix_seconds,
                            ),
                            provider_observation: &response.credit_provider_observation,
                            history_observations: &responses_history_observations,
                            snapshot: &snapshot,
                        },
                    )
                    .await?
            } else {
                quota_history_state
                    .record_refresh_success_and_replace_selector_windows(
                        account.account_id(),
                        route_band,
                        &selector_windows,
                        observed_unix_seconds,
                        stale_after_unix_seconds(observed_unix_seconds),
                    )
                    .await?;
                true
            };
            if !refresh_was_committed {
                continue;
            }
            if *route_band == USER_QUOTA_ROUTE_BAND {
                let Some(attempt) = credit_refresh_attempt.as_ref() else {
                    return Err(QuotaCommandError::ProviderResponse {
                        message: "Responses refresh attempt was not allocated".to_owned(),
                    });
                };
                committed_responses_generations.insert(
                    account.account_id().clone(),
                    attempt.credential_generation(),
                );
            }
            if *route_band != USER_QUOTA_ROUTE_BAND {
                quota_history_state
                    .upsert_quota_snapshot_preserving_selector_windows(&snapshot)
                    .await?;
            }
            if *route_band == USER_QUOTA_ROUTE_BAND
                && let Some(observer) = weekly_floor_observer
            {
                let floor = weekly_quota_floors.get(account.account_id()).copied();
                let weekly_remaining_basis_points = response
                    .windows
                    .iter()
                    .find(|window| window.limit_window_seconds == V1_WEEKLY_WINDOW_SECONDS)
                    .map(|window| window.remaining_headroom.saturating_mul(100));
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

    refresh_result.map(|()| QuotaRefreshReport {
        committed_responses_generations,
    })
}

async fn begin_credit_refresh_attempt_for_current_generation(
    state: &AsyncSqliteStateStore,
    account_id: &AccountId,
    credential_generation: u64,
) -> Result<Option<CreditRefreshAttempt>, QuotaCommandError> {
    match state
        .begin_credit_refresh_attempt(account_id, credential_generation)
        .await
    {
        Ok(attempt) => Ok(Some(attempt)),
        Err(StateStoreError::AccountConcurrentModification { .. }) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn record_superseded_account_refresh(
    stdout: &mut impl Write,
    account: &AccountRecord,
    failed_count: &mut u64,
) -> Result<(), QuotaCommandError> {
    *failed_count = failed_count.saturating_add(DEFAULT_ROUTE_BANDS.len() as u64);
    let diagnostic_account = quota_refresh_diagnostic_account_label(account);
    writeln!(
        stdout,
        "refresh skipped: account={diagnostic_account} error=credential generation changed during refresh",
    )
    .map_err(QuotaCommandError::Stdout)
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

use super::quota_account_refresh::AccountQuotaRefresh;
use super::*;
impl<R, P, W> AccountQuotaRefresh<'_, R, P, W>
where
    R: AsyncProviderCredentialResolver,
    P: QuotaRefreshProvider,
    W: Write,
{
    pub(super) async fn refresh_openai_account(self) -> Result<(), QuotaRefreshError> {
        let Self {
            stdout,
            quota_history_state,
            account,
            base_url,
            credential_resolver,
            quota_provider,
            observed_unix_seconds,
            weekly_quota_floor,
            weekly_floor_observer,
            resolved,
            refreshed_count,
            failed_count,
            committed_responses_generations,
            ..
        } = self;
        for route_band in DEFAULT_ROUTE_BANDS {
            let mut credit_refresh_attempt = if *route_band == USER_QUOTA_ROUTE_BAND {
                match begin_credit_refresh_attempt_for_current_generation(
                    quota_history_state,
                    account.account_id(),
                    resolved.credential_generation(),
                )
                .await?
                {
                    Some(attempt) => Some(attempt),
                    None => {
                        record_superseded_account_refresh(&mut *stdout, account, failed_count)?;
                        return Ok(());
                    }
                }
            } else {
                None
            };
            let first_response = quota_provider
                .fetch_quota(QuotaRefreshProviderRequest::new_for_provider(
                    account.provider(),
                    account.account_id().clone(),
                    account.label(),
                    *route_band,
                    base_url,
                    resolved.access_token().clone(),
                    resolved.chatgpt_account_id(),
                ))
                .await;
            let initial_unauthorized = matches!(
                &first_response,
                Err(QuotaRefreshError::ProviderStatus { status: 401 })
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
                        if *route_band == USER_QUOTA_ROUTE_BAND {
                            credit_refresh_attempt =
                                match begin_credit_refresh_attempt_for_current_generation(
                                    quota_history_state,
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
                                            failed_count,
                                        )?;
                                        return Ok(());
                                    }
                                };
                        }
                        let retry_response = quota_provider
                            .fetch_quota(QuotaRefreshProviderRequest::new_for_provider(
                                account.provider(),
                                account.account_id().clone(),
                                account.label(),
                                *route_band,
                                base_url,
                                recovered.access_token().clone(),
                                recovered.chatgpt_account_id(),
                            ))
                            .await;
                        *resolved = recovered;
                        (retry_response, retry_generation, renewed_here)
                    }
                    Err(error) => (
                        Err(QuotaRefreshError::CredentialResolver(error)),
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
                    *failed_count = failed_count.saturating_add(1);
                    let error_class = quota_refresh_error_class(&error);
                    if *route_band == USER_QUOTA_ROUTE_BAND {
                        let Some(attempt) = credit_refresh_attempt.as_ref() else {
                            return Err(QuotaRefreshError::ProviderResponse {
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
                            quota_history_state,
                            account,
                            route_band,
                            observed_unix_seconds,
                            error_class,
                        )
                        .await?;
                    }
                    let provider_rejected_credentials =
                        matches!(error, QuotaRefreshError::ProviderStatus { status: 401 });
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
                    .map_err(QuotaRefreshError::Stdout)?;
                    if provider_rejected_credentials || initial_unauthorized {
                        break;
                    }
                    continue;
                }
            };
            let effective_window = match response.effective_window() {
                Some(effective_window) => effective_window,
                None => {
                    *failed_count = failed_count.saturating_add(1);
                    if *route_band == USER_QUOTA_ROUTE_BAND {
                        let Some(attempt) = credit_refresh_attempt.as_ref() else {
                            return Err(QuotaRefreshError::ProviderResponse {
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
                            quota_history_state,
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
                    .map_err(QuotaRefreshError::Stdout)?;
                    continue;
                }
            };
            let mut selector_windows = Vec::new();
            let mut responses_history_observations = Vec::new();
            for window in &response.windows {
                let remaining_headroom = window.headroom.percent().ok_or_else(|| {
                    QuotaRefreshError::ProviderResponse {
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
                if *route_band == USER_QUOTA_ROUTE_BAND {
                    responses_history_observations.push(success_quota_history_observation(
                        account,
                        route_band,
                        window,
                        observed_unix_seconds,
                        response.reset_credits_available,
                    )?);
                } else {
                    append_success_quota_history_observation(
                        quota_history_state,
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
            .with_route_band(
                *route_band,
                effective_window.headroom.percent().ok_or_else(|| {
                    QuotaRefreshError::ProviderResponse {
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
            let weekly_remaining_basis_points =
                if *route_band == USER_QUOTA_ROUTE_BAND && weekly_floor_observer.is_some() {
                    match response
                        .windows
                        .iter()
                        .find(|window| window.limit_window_seconds == V1_WEEKLY_WINDOW_SECONDS)
                    {
                        Some(window) => Some(
                            window
                                .headroom
                                .percent()
                                .ok_or_else(|| QuotaRefreshError::ProviderResponse {
                                    message: "OpenAI quota window was not expressed as a percent"
                                        .to_owned(),
                                })?
                                .saturating_mul(100),
                        ),
                        None => None,
                    }
                } else {
                    None
                };
            let refresh_was_committed = if *route_band == USER_QUOTA_ROUTE_BAND {
                let Some(attempt) = credit_refresh_attempt.as_ref() else {
                    return Err(QuotaRefreshError::ProviderResponse {
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
                if *route_band == USER_QUOTA_ROUTE_BAND {
                    let Some(attempt) = credit_refresh_attempt.as_ref() else {
                        return Err(QuotaRefreshError::ProviderResponse {
                            message: "Responses refresh attempt was not allocated".to_owned(),
                        });
                    };
                    tracing::info!(
                        account.hash = telemetry_hash(account.account_id().as_str()),
                        credential_generation = attempt.credential_generation(),
                        attempt_sequence = attempt.sequence(),
                        "codex_router.responses_quota_refresh_superseded"
                    );
                    let current_generation = quota_history_state
                        .list_accounts()
                        .await?
                        .into_iter()
                        .find(|current| current.account_id() == account.account_id())
                        .and_then(|current| current.active_credential_generation());
                    if current_generation != Some(attempt.credential_generation()) {
                        return Ok(());
                    }
                    if let Some(observer) = weekly_floor_observer {
                        notify_weekly_floor_from_latest_committed_responses(
                            quota_history_state,
                            account.account_id(),
                            attempt.credential_generation(),
                            weekly_quota_floor,
                            observed_unix_seconds,
                            SupersededResponsesFloorRead {
                                attempt_sequence: attempt.sequence(),
                                weekly_remaining_basis_points,
                            },
                            observer,
                        )
                        .await?;
                    }
                }
                continue;
            }
            if *route_band == USER_QUOTA_ROUTE_BAND {
                let Some(attempt) = credit_refresh_attempt.as_ref() else {
                    return Err(QuotaRefreshError::ProviderResponse {
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
                let floor = weekly_quota_floor;
                let intent = weekly_quota_floor_intent(floor, weekly_remaining_basis_points);
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
            *refreshed_count = refreshed_count.saturating_add(1);
        }

        Ok(())
    }
}

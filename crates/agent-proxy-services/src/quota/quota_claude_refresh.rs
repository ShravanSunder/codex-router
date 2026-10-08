use super::quota_account_refresh::AccountQuotaRefresh;
use super::*;
use codex_router_core::provider::Provider;
use codex_router_core::route_profile::WindowKind;
use codex_router_state::window_observation::{
    WindowObservation, WindowObservationProps,
    calculate_window_observation_fresh_until_unix_seconds,
};
impl<R, P, W> AccountQuotaRefresh<'_, R, P, W>
where
    R: AsyncProviderCredentialResolver,
    P: QuotaRefreshProvider,
    W: Write,
{
    pub(super) async fn refresh_claude_account(self) -> Result<(), QuotaRefreshError> {
        let Self {
            stdout,
            quota_history_state,
            account,
            base_url,
            credential_resolver,
            quota_provider,
            observed_unix_seconds,
            refresh_interval_seconds,
            resolved,
            refreshed_count,
            failed_count,
            ..
        } = self;
        let observation_started_at = current_unix_seconds();
        let first_response = quota_provider
            .fetch_quota(QuotaRefreshProviderRequest::new_for_provider(
                Provider::Claude,
                account.account_id().clone(),
                account.label(),
                "claude_messages",
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
                            base_url,
                            recovered.access_token().clone(),
                            recovered.chatgpt_account_id(),
                        ))
                        .await;
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
                quota_history_state
                    .record_refresh_failure_preserving_selector_windows(
                        account.account_id(),
                        "claude_messages",
                        observation_started_at,
                        error_class,
                    )
                    .await?;
                let provider_rejected_credentials =
                    matches!(error, QuotaRefreshError::ProviderStatus { status: 401 });
                if provider_rejected_credentials
                    && renewed_for_retry
                    && let Some(retry_generation) = retry_generation
                {
                    tracing::warn!(
                        account.hash = telemetry_hash(account.account_id().as_str()),
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
                record_quota_refresh_metric("claude_messages", "failure", diagnostic_error_class);
                let diagnostic_account = quota_refresh_diagnostic_account_label(account);
                writeln!(
                        stdout,
                        "refresh failed: account={diagnostic_account} route_band=claude_messages error={error}",
                    )
                    .map_err(QuotaRefreshError::Stdout)?;
                return Ok(());
            }
        };
        let mut observations_recorded = 0_u64;
        let fresh_until_unix_seconds = calculate_window_observation_fresh_until_unix_seconds(
            observation_started_at,
            refresh_interval_seconds,
        )
        .map_err(|error| QuotaRefreshError::ProviderResponse {
            message: error.to_string(),
        })?;
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
                    QuotaRefreshError::ProviderResponse {
                        message: "Claude quota window was not expressed in basis points".to_owned(),
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
            quota_history_state
                .record_refresh_success_status(
                    account.account_id(),
                    RouteBand::ClaudeMessages.as_str(),
                    observation_started_at,
                    fresh_until_unix_seconds,
                )
                .await?;
            *refreshed_count = refreshed_count.saturating_add(1);
        }
        Ok(())
    }
}

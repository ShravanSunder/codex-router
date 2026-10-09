use super::*;
use codex_router_core::ids::AccountId;
use codex_router_core::provider::Provider;

#[derive(Clone, Debug, Default)]
pub struct QuotaRefreshReport {
    committed_responses_generations: HashMap<AccountId, u64>,
}

impl QuotaRefreshReport {
    pub fn responses_observation_committed_for(
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
pub enum WeeklyQuotaFloorIntent {
    GracefulSwitch,
    HardStop,
    Clear,
}

pub trait WeeklyQuotaFloorIntentObserver: Send + Sync {
    fn weekly_quota_floor_intent(&self, account_id: &AccountId, intent: WeeklyQuotaFloorIntent);
}

pub struct QuotaRefreshObservationContext<'a> {
    pub observed_unix_seconds: u64,
    pub schedule: QuotaRefreshSchedule,
    pub weekly_floor_observer: Option<&'a dyn WeeklyQuotaFloorIntentObserver>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuotaRefreshSchedule {
    Manual,
    Background { interval_seconds: u64 },
}

impl QuotaRefreshSchedule {
    const fn interval_seconds(self) -> u64 {
        match self {
            Self::Manual => crate::DEFAULT_QUOTA_REFRESH_INTERVAL_SECONDS,
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

pub async fn refresh_quota_with_dependencies<R, P>(
    stdout: &mut impl Write,
    router_root: PathBuf,
    base_url: String,
    credential_resolver: &R,
    quota_provider: &P,
    observed_unix_seconds: u64,
) -> Result<QuotaRefreshReport, QuotaRefreshError>
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

pub async fn refresh_quota_store_paths_with_dependencies<R, P>(
    stdout: &mut impl Write,
    state_db: &Path,
    _secret_root: &Path,
    base_url: String,
    credential_resolver: &R,
    quota_provider: &P,
    observed_unix_seconds: u64,
) -> Result<QuotaRefreshReport, QuotaRefreshError>
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

pub async fn refresh_quota_store_paths_with_dependencies_and_floor_notifier<R, P>(
    stdout: &mut impl Write,
    state_db: &Path,
    _secret_root: &Path,
    base_url: String,
    credential_resolver: &R,
    quota_provider: &P,
    observation_context: QuotaRefreshObservationContext<'_>,
) -> Result<QuotaRefreshReport, QuotaRefreshError>
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
    let mut committed_responses_generations = HashMap::new();
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
        let credential_resolution_started_at = if account.provider() == Provider::Claude {
            Some(current_unix_seconds())
        } else {
            None
        };
        let mut resolved = match credential_resolver
            .resolve_provider_credentials_async(account.account_id(), account.provider())
            .await
        {
            Ok(resolved) => resolved,
            Err(error) => {
                failed_count = failed_count.saturating_add(route_bands.len() as u64);
                let current_account = if account.provider() == Provider::Openai {
                    quota_history_state
                        .list_accounts()
                        .await?
                        .into_iter()
                        .find(|current| current.account_id() == account.account_id())
                } else {
                    None
                };
                let responses_credit_attempt = match current_account
                    .as_ref()
                    .and_then(AccountRecord::active_credential_generation)
                {
                    Some(generation) => {
                        begin_credit_refresh_attempt_for_current_generation(
                            &quota_history_state,
                            account.account_id(),
                            generation,
                        )
                        .await?
                    }
                    None => None,
                };
                let failure_status_attempt_unix_seconds =
                    credential_resolution_started_at.unwrap_or(observed_unix_seconds);
                for route_band in route_bands {
                    if *route_band == USER_QUOTA_ROUTE_BAND {
                        if let Some(attempt) = responses_credit_attempt.as_ref() {
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
                        } else if current_account.is_some() {
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
                    } else {
                        quota_history_state
                            .record_refresh_failure_preserving_selector_windows(
                                account.account_id(),
                                route_band,
                                failure_status_attempt_unix_seconds,
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
                .map_err(QuotaRefreshError::Stdout)?;
                continue;
            }
        };
        let context = quota_account_refresh::AccountQuotaRefresh {
            stdout,
            quota_history_state: &quota_history_state,
            account,
            base_url: &base_url,
            credential_resolver,
            quota_provider,
            observed_unix_seconds,
            refresh_interval_seconds,
            weekly_quota_floor: weekly_quota_floors.get(account.account_id()).copied(),
            weekly_floor_observer,
            resolved: &mut resolved,
            refreshed_count: &mut refreshed_count,
            failed_count: &mut failed_count,
            committed_responses_generations: &mut committed_responses_generations,
        };
        if account.provider() == Provider::Claude {
            context.refresh_claude_account().await?;
        } else {
            context.refresh_openai_account().await?;
        }
    }
    purge_old_quota_history(&quota_history_state, observed_unix_seconds).await?;

    writeln!(stdout, "refreshed: {refreshed_count}").map_err(QuotaRefreshError::Stdout)?;
    if failed_count > 0 {
        writeln!(stdout, "failed: {failed_count}").map_err(QuotaRefreshError::Stdout)?;
    }
    let refresh_result = if refreshed_count == 0 && failed_count > 0 {
        Err(QuotaRefreshError::ProviderResponse {
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

//! Quota command glue for persisted router-owned quota state.

use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::thread;
use std::thread::JoinHandle;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use codex_router_auth::live_quota::DEFAULT_CHATGPT_BACKEND_BASE_URL;
use codex_router_auth::live_quota::UsageResponse;
use codex_router_auth::live_quota::WindowPair;
use codex_router_auth::live_quota::usage_url;
use codex_router_auth::resolver::CredentialResolverError;
use codex_router_auth::resolver::ProviderCredentialResolver;
use codex_router_core::ids::AccountId;
use codex_router_core::redaction::SecretString;
use codex_router_core::routes::RouteBand;
use codex_router_selection::burn_down::AccountAvailability;
use codex_router_selection::burn_down::BurnDownAccountAssessment;
use codex_router_selection::burn_down::BurnDownAccountInput;
use codex_router_selection::burn_down::BurnDownRouteBandAssessmentInput;
use codex_router_selection::burn_down::LimitingWindow;
use codex_router_selection::burn_down::QuotaEvidenceFreshness;
use codex_router_selection::burn_down::QuotaEvidenceReason;
use codex_router_selection::burn_down::QuotaWindowFact;
use codex_router_selection::burn_down::QuotaWindowStatus;
use codex_router_selection::burn_down::RoutingExclusion;
use codex_router_selection::burn_down::RoutingReason;
use codex_router_selection::burn_down::SelectedPool;
use codex_router_selection::burn_down::V1_SHORT_WINDOW_SECONDS;
use codex_router_selection::burn_down::V1_WEEKLY_WINDOW_SECONDS;
use codex_router_selection::burn_down::assess_route_band;
use codex_router_selection::run_rate::QuotaRunRateConfidence;
use codex_router_selection::run_rate::QuotaRunRateEstimate;
use codex_router_selection::run_rate::QuotaRunRateEstimator;
use codex_router_selection::run_rate::QuotaRunRateObservation;
use codex_router_state::account::AccountRecord;
use codex_router_state::account::AccountStatus;
use codex_router_state::quota_snapshot::PersistedQuotaHistoryObservation;
use codex_router_state::quota_snapshot::PersistedQuotaSnapshot;
use codex_router_state::quota_snapshot::PersistedSelectorQuotaWindow;
use codex_router_state::quota_snapshot::QuotaHistoryRefreshOutcome;
use codex_router_state::quota_snapshot::QuotaRefreshErrorClass;
use codex_router_state::quota_snapshot::QuotaSnapshotSource;
use codex_router_state::quota_snapshot::SelectorQuotaInput;
use codex_router_state::quota_snapshot::SelectorQuotaWindowStatus;
use codex_router_state::repositories::AccountStateRepository;
use codex_router_state::repositories::QuotaSnapshotRepository;
use codex_router_state::repositories::SelectorQuotaRepository;
use codex_router_state::sqlite::AsyncSqliteStateStore;
use codex_router_state::sqlite::SqliteStateStore;
use codex_router_state::sqlite::StateStoreError;
use comfy_table::Table;
use comfy_table::presets::UTF8_FULL;
use serde::Serialize;
use serde_json::Value;
use thiserror::Error;

use crate::ArgumentParser;
use crate::CliError;
use crate::credential_runtime::CliCredentialResolver;
use crate::credential_runtime::CliCredentialResolverOpenError;
use crate::router_root_or_default;

const DEFAULT_ROUTE_BANDS: &[&str] = &["responses", "models"];
const USER_QUOTA_ROUTE_BAND: &str = "responses";
const DEFAULT_REFRESH_STALE_AFTER_GRACE_SECONDS: u64 = 600;

/// Quota CLI command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum QuotaCommand {
    /// Renders persisted quota status.
    Status {
        /// Router-owned root.
        router_root: PathBuf,
        /// Output format.
        format: QuotaStatusFormat,
        /// Whether to include all known route bands.
        all_limits: bool,
        /// Current clock used for pace and runout math.
        now_unix_seconds: u64,
    },
    /// Refreshes persisted quota from the provider.
    Refresh {
        /// Router-owned root.
        router_root: PathBuf,
        /// Provider base URL.
        base_url: String,
    },
}

impl QuotaCommand {
    pub(crate) fn parse(parser: &mut ArgumentParser) -> Result<Self, CliError> {
        let Some(command) = parser.next_string()? else {
            return Err(CliError::MissingCommand {
                command: "quota".to_owned(),
            });
        };

        match command.as_str() {
            "status" => {
                let options = QuotaStatusOptions::parse(parser)?;
                Ok(Self::Status {
                    router_root: options.router_root()?,
                    format: options.format,
                    all_limits: options.all_limits,
                    now_unix_seconds: options.now_unix_seconds,
                })
            }
            "refresh" => {
                let options = QuotaRefreshOptions::parse(parser)?;
                Ok(Self::Refresh {
                    router_root: options.router_root()?,
                    base_url: options.base_url,
                })
            }
            unknown => Err(CliError::UnknownCommand {
                command: format!("quota {unknown}"),
            }),
        }
    }
}

/// Quota status output format.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuotaStatusFormat {
    /// Human-readable table.
    Table,
    /// Plain tab-separated records.
    Plain,
    /// JSON debug/proof records.
    Json,
}

/// Quota command failure.
#[derive(Debug, Error)]
pub enum QuotaCommandError {
    /// Format option was invalid.
    #[error("invalid quota status format: {value}")]
    InvalidFormat {
        /// Raw value.
        value: String,
    },
    /// Quota refresh base URL is not one of the allowlisted provider URLs.
    #[error("quota refresh base URL is not allowed: {base_url}")]
    DisallowedBaseUrl {
        /// Rejected base URL.
        base_url: String,
    },
    /// Quota refresh is not implemented for allowed providers in this slice.
    #[error("quota refresh provider execution is not implemented in Plan 1A")]
    RefreshNotImplemented,
    /// Quota refresh provider request failed before a response status was available.
    #[error("quota refresh request failed: {message}")]
    ProviderRequest {
        /// Redacted request failure.
        message: String,
    },
    /// Quota refresh provider returned a non-success response.
    #[error("quota refresh provider returned HTTP {status}")]
    ProviderStatus {
        /// Provider HTTP status.
        status: u16,
    },
    /// Quota refresh provider response did not contain usable quota data.
    #[error("quota refresh provider response was unusable: {message}")]
    ProviderResponse {
        /// Redacted response failure.
        message: String,
    },
    /// Credential resolver dependencies failed to open.
    #[error(transparent)]
    CredentialResolverOpen(#[from] CliCredentialResolverOpenError),
    /// Credential resolution failed before provider quota refresh.
    #[error(transparent)]
    CredentialResolver(#[from] CredentialResolverError),
    /// State-store operation failed.
    #[error(transparent)]
    StateStore(#[from] StateStoreError),
    /// Failed to initialize quota history async runtime.
    #[error("failed to initialize quota history runtime: {0}")]
    Runtime(std::io::Error),
    /// Stdout write failed.
    #[error("failed to write stdout: {0}")]
    Stdout(std::io::Error),
}

/// Runs a quota command.
pub fn run_quota_command(
    stdout: &mut impl Write,
    command: QuotaCommand,
) -> Result<(), QuotaCommandError> {
    match command {
        QuotaCommand::Status {
            router_root,
            format,
            all_limits,
            now_unix_seconds,
        } => render_quota_status(stdout, router_root, format, all_limits, now_unix_seconds),
        QuotaCommand::Refresh {
            router_root,
            base_url,
        } => refresh_quota(stdout, router_root, base_url),
    }
}

fn refresh_quota(
    stdout: &mut impl Write,
    router_root: PathBuf,
    base_url: String,
) -> Result<(), QuotaCommandError> {
    if !is_allowed_quota_refresh_base_url(&base_url) {
        return Err(QuotaCommandError::DisallowedBaseUrl { base_url });
    }

    let resolver = CliCredentialResolver::open(
        &router_root.join("state.sqlite"),
        &router_root.join("secrets"),
        0,
    )?;
    refresh_quota_with_dependencies(
        stdout,
        router_root,
        base_url,
        &resolver,
        &HttpQuotaRefreshProvider::new()?,
        current_unix_seconds(),
    )
}

pub(crate) fn is_allowed_quota_refresh_base_url(base_url: &str) -> bool {
    let trimmed = base_url.trim_end_matches('/');
    trimmed == DEFAULT_CHATGPT_BACKEND_BASE_URL
        || trimmed == "https://chatgpt.com"
        || trimmed.starts_with("https://chatgpt.com/")
}

/// Quota provider request after provider credentials have been resolved.
pub(crate) struct QuotaRefreshProviderRequest {
    account_id: AccountId,
    account_label: String,
    route_band: String,
    base_url: String,
    access_token: SecretString,
}

impl QuotaRefreshProviderRequest {
    pub(crate) fn new(
        account_id: AccountId,
        account_label: impl Into<String>,
        route_band: impl Into<String>,
        base_url: impl Into<String>,
        access_token: SecretString,
    ) -> Self {
        Self {
            account_id,
            account_label: account_label.into(),
            route_band: route_band.into(),
            base_url: base_url.into(),
            access_token,
        }
    }

    /// Returns the account id.
    #[must_use]
    pub(crate) const fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    /// Returns the account label.
    #[must_use]
    pub(crate) fn account_label(&self) -> &str {
        &self.account_label
    }

    /// Returns the route band.
    #[must_use]
    pub(crate) fn route_band(&self) -> &str {
        &self.route_band
    }

    /// Returns the provider base URL.
    #[must_use]
    pub(crate) fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Returns the provider bearer token.
    #[must_use]
    pub(crate) const fn access_token(&self) -> &SecretString {
        &self.access_token
    }
}

/// Quota provider response.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct QuotaRefreshProviderResponse {
    pub(crate) windows: Vec<QuotaRefreshProviderWindow>,
    pub(crate) reset_credits_available: Option<u32>,
}

impl QuotaRefreshProviderResponse {
    fn effective_window(&self) -> Option<&QuotaRefreshProviderWindow> {
        self.windows
            .iter()
            .find(|window| window.effective)
            .or_else(|| self.windows.first())
    }
}

/// Quota provider response for one limit window.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct QuotaRefreshProviderWindow {
    pub(crate) limit_window_seconds: u64,
    pub(crate) remaining_headroom: u32,
    pub(crate) reset_unix_seconds: Option<u64>,
    pub(crate) effective: bool,
}

/// Provider egress dependency for quota refresh.
pub(crate) trait QuotaRefreshProvider {
    /// Fetches one route-band quota snapshot using resolved provider auth.
    fn fetch_quota(
        &self,
        request: QuotaRefreshProviderRequest,
    ) -> Result<QuotaRefreshProviderResponse, QuotaCommandError>;
}

/// HTTP quota refresh provider for ChatGPT/Codex usage endpoints.
#[derive(Debug)]
pub(crate) struct HttpQuotaRefreshProvider {
    client: reqwest::blocking::Client,
}

impl HttpQuotaRefreshProvider {
    /// Creates an HTTP quota refresh provider.
    pub(crate) fn new() -> Result<Self, QuotaCommandError> {
        Self::new_with_timeout(Duration::from_secs(30))
    }

    /// Creates an HTTP quota refresh provider with a bounded request timeout.
    pub(crate) fn new_with_timeout(timeout: Duration) -> Result<Self, QuotaCommandError> {
        let client = reqwest::blocking::Client::builder()
            .user_agent("codex-router-quota-refresh")
            .timeout(timeout)
            .build()
            .map_err(|error| QuotaCommandError::ProviderRequest {
                message: error.to_string(),
            })?;
        Ok(Self { client })
    }
}

impl QuotaRefreshProvider for HttpQuotaRefreshProvider {
    fn fetch_quota(
        &self,
        request: QuotaRefreshProviderRequest,
    ) -> Result<QuotaRefreshProviderResponse, QuotaCommandError> {
        let _account_context = (request.account_id(), request.account_label());
        let response = self
            .client
            .get(usage_url(request.base_url()))
            .bearer_auth(request.access_token().expose_secret())
            .send()
            .map_err(|error| QuotaCommandError::ProviderRequest {
                message: error.to_string(),
            })?;
        let status = response.status();
        if !status.is_success() {
            return Err(QuotaCommandError::ProviderStatus {
                status: status.as_u16(),
            });
        }
        let body = response
            .text()
            .map_err(|error| QuotaCommandError::ProviderRequest {
                message: error.to_string(),
            })?;
        let usage_value = serde_json::from_str::<Value>(&body).map_err(|error| {
            QuotaCommandError::ProviderResponse {
                message: error.to_string(),
            }
        })?;
        let reset_credits_available = reset_credits_available_from_json(&usage_value);
        let usage = serde_json::from_value::<UsageResponse>(usage_value).map_err(|error| {
            QuotaCommandError::ProviderResponse {
                message: error.to_string(),
            }
        })?;
        quota_response_for_route_band(&usage, request.route_band()).map(|mut response| {
            response.reset_credits_available = reset_credits_available;
            response
        })
    }
}

pub(crate) fn refresh_quota_with_dependencies<R, P>(
    stdout: &mut impl Write,
    router_root: PathBuf,
    base_url: String,
    credential_resolver: &R,
    quota_provider: &P,
    observed_unix_seconds: u64,
) -> Result<(), QuotaCommandError>
where
    R: ProviderCredentialResolver,
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
}

pub(crate) fn refresh_quota_store_paths_with_dependencies<R, P>(
    stdout: &mut impl Write,
    state_db: &Path,
    _secret_root: &Path,
    base_url: String,
    credential_resolver: &R,
    quota_provider: &P,
    observed_unix_seconds: u64,
) -> Result<(), QuotaCommandError>
where
    R: ProviderCredentialResolver,
    P: QuotaRefreshProvider,
{
    let state = SqliteStateStore::open(state_db)?;
    let quota_history_runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(QuotaCommandError::Runtime)?;
    let quota_history_state =
        quota_history_runtime.block_on(AsyncSqliteStateStore::open(state_db))?;
    let accounts = AccountStateRepository::list_accounts(&state)?;
    let mut refreshed_count = 0_u64;
    let mut failed_count = 0_u64;
    for account in accounts
        .iter()
        .filter(|account| account.status() == AccountStatus::Enabled)
        .filter(|account| account.active_credential_generation().is_some())
    {
        let resolved = match credential_resolver.resolve_provider_credentials(account.account_id())
        {
            Ok(resolved) => resolved,
            Err(error) => {
                failed_count = failed_count.saturating_add(DEFAULT_ROUTE_BANDS.len() as u64);
                for route_band in DEFAULT_ROUTE_BANDS {
                    SelectorQuotaRepository::record_refresh_failure_preserving_selector_windows(
                        &state,
                        account.account_id(),
                        route_band,
                        observed_unix_seconds,
                        QuotaRefreshErrorClass::AuthError,
                    )?;
                    append_failure_quota_history_observations(
                        &quota_history_runtime,
                        &quota_history_state,
                        account,
                        route_band,
                        observed_unix_seconds,
                        QuotaRefreshErrorClass::AuthError,
                    )?;
                }
                writeln!(
                    stdout,
                    "refresh failed: account={} route_band=* error={error}",
                    account.label()
                )
                .map_err(QuotaCommandError::Stdout)?;
                continue;
            }
        };
        for route_band in DEFAULT_ROUTE_BANDS {
            let response = match quota_provider.fetch_quota(QuotaRefreshProviderRequest::new(
                account.account_id().clone(),
                account.label(),
                *route_band,
                base_url.clone(),
                resolved.access_token().clone(),
            )) {
                Ok(response) => response,
                Err(error) => {
                    failed_count = failed_count.saturating_add(1);
                    SelectorQuotaRepository::record_refresh_failure_preserving_selector_windows(
                        &state,
                        account.account_id(),
                        route_band,
                        observed_unix_seconds,
                        quota_refresh_error_class(&error),
                    )?;
                    append_failure_quota_history_observations(
                        &quota_history_runtime,
                        &quota_history_state,
                        account,
                        route_band,
                        observed_unix_seconds,
                        quota_refresh_error_class(&error),
                    )?;
                    writeln!(
                        stdout,
                        "refresh failed: account={} route_band={} error={error}",
                        account.label(),
                        route_band
                    )
                    .map_err(QuotaCommandError::Stdout)?;
                    continue;
                }
            };
            let effective_window = match response.effective_window() {
                Some(effective_window) => effective_window,
                None => {
                    failed_count = failed_count.saturating_add(1);
                    SelectorQuotaRepository::record_refresh_failure_preserving_selector_windows(
                        &state,
                        account.account_id(),
                        route_band,
                        observed_unix_seconds,
                        QuotaRefreshErrorClass::ParseError,
                    )?;
                    append_failure_quota_history_observations(
                        &quota_history_runtime,
                        &quota_history_state,
                        account,
                        route_band,
                        observed_unix_seconds,
                        QuotaRefreshErrorClass::ParseError,
                    )?;
                    writeln!(
                        stdout,
                        "refresh failed: account={} route_band={} error=missing provider quota windows",
                        account.label(),
                        route_band
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
            QuotaSnapshotRepository::upsert_snapshot(&state, &snapshot)?;
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
                    &quota_history_runtime,
                    &quota_history_state,
                    account,
                    route_band,
                    window,
                    observed_unix_seconds,
                    response.reset_credits_available,
                )?;
            }
            SelectorQuotaRepository::record_refresh_success_and_replace_selector_windows(
                &state,
                account.account_id(),
                route_band,
                &selector_windows,
                observed_unix_seconds,
                stale_after_unix_seconds(observed_unix_seconds),
            )?;
            refreshed_count = refreshed_count.saturating_add(1);
        }
    }
    purge_old_quota_history(
        &quota_history_runtime,
        &quota_history_state,
        observed_unix_seconds,
    )?;

    writeln!(stdout, "refreshed: {refreshed_count}").map_err(QuotaCommandError::Stdout)?;
    if failed_count > 0 {
        writeln!(stdout, "failed: {failed_count}").map_err(QuotaCommandError::Stdout)?;
    }
    if refreshed_count == 0 && failed_count > 0 {
        return Err(QuotaCommandError::ProviderResponse {
            message: "quota refresh failed for all eligible route bands".to_owned(),
        });
    }

    Ok(())
}

fn append_success_quota_history_observation(
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

fn append_failure_quota_history_observations(
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

fn purge_old_quota_history(
    runtime: &tokio::runtime::Runtime,
    state: &AsyncSqliteStateStore,
    observed_unix_seconds: u64,
) -> Result<(), QuotaCommandError> {
    let retention_floor = observed_unix_seconds.saturating_sub(V1_WEEKLY_WINDOW_SECONDS);
    runtime
        .block_on(state.purge_quota_history_before(retention_floor))
        .map_err(QuotaCommandError::StateStore)
}

/// Stoppable background quota refresh worker.
pub(crate) struct BackgroundQuotaRefreshWorker {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

pub(crate) struct BackgroundQuotaRefreshRuntime<C, D> {
    observed_clock: C,
    diagnostic_reporter: D,
    interval: Duration,
}

impl<C, D> BackgroundQuotaRefreshRuntime<C, D> {
    pub(crate) const fn new(observed_clock: C, diagnostic_reporter: D, interval: Duration) -> Self {
        Self {
            observed_clock,
            diagnostic_reporter,
            interval,
        }
    }
}

impl Drop for BackgroundQuotaRefreshWorker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _result = thread.join();
        }
    }
}

#[cfg(test)]
pub(crate) fn start_background_quota_refresh_worker_with_dependencies<R, P>(
    state_db: PathBuf,
    secret_root: PathBuf,
    base_url: String,
    credential_resolver: R,
    quota_provider: P,
    interval: Duration,
) -> BackgroundQuotaRefreshWorker
where
    R: ProviderCredentialResolver + Send + 'static,
    P: QuotaRefreshProvider + Send + 'static,
{
    start_background_quota_refresh_worker_with_clock(
        state_db,
        secret_root,
        base_url,
        credential_resolver,
        quota_provider,
        current_unix_seconds,
        interval,
    )
}

#[cfg(test)]
pub(crate) fn start_background_quota_refresh_worker_with_clock<R, P, C>(
    state_db: PathBuf,
    secret_root: PathBuf,
    base_url: String,
    credential_resolver: R,
    quota_provider: P,
    observed_clock: C,
    interval: Duration,
) -> BackgroundQuotaRefreshWorker
where
    R: ProviderCredentialResolver + Send + 'static,
    P: QuotaRefreshProvider + Send + 'static,
    C: FnMut() -> u64 + Send + 'static,
{
    start_background_quota_refresh_worker_with_reporter(
        state_db,
        secret_root,
        base_url,
        credential_resolver,
        quota_provider,
        BackgroundQuotaRefreshRuntime::new(observed_clock, |_diagnostic| {}, interval),
    )
}

pub(crate) fn start_background_quota_refresh_worker_with_reporter<R, P, C, D>(
    state_db: PathBuf,
    secret_root: PathBuf,
    base_url: String,
    credential_resolver: R,
    quota_provider: P,
    runtime: BackgroundQuotaRefreshRuntime<C, D>,
) -> BackgroundQuotaRefreshWorker
where
    R: ProviderCredentialResolver + Send + 'static,
    P: QuotaRefreshProvider + Send + 'static,
    C: FnMut() -> u64 + Send + 'static,
    D: FnMut(String) + Send + 'static,
{
    let BackgroundQuotaRefreshRuntime {
        mut observed_clock,
        mut diagnostic_reporter,
        interval,
    } = runtime;
    let stop = Arc::new(AtomicBool::new(false));
    let stop_for_thread = Arc::clone(&stop);
    let thread = thread::spawn(move || {
        loop {
            let mut sink = Vec::new();
            let observed_unix_seconds = observed_clock();
            let result = refresh_quota_store_paths_with_dependencies(
                &mut sink,
                &state_db,
                &secret_root,
                base_url.clone(),
                &credential_resolver,
                &quota_provider,
                observed_unix_seconds,
            );
            let diagnostic_output = String::from_utf8_lossy(&sink).into_owned();
            if diagnostic_output
                .lines()
                .any(|line| line.starts_with("refresh failed:") || line.starts_with("failed:"))
            {
                diagnostic_reporter(diagnostic_output.trim_end().to_owned());
            }
            if let Err(error) = result {
                diagnostic_reporter(format!("background quota refresh failed: {error}"));
            }
            if interval.is_zero() || !sleep_interruptibly(&stop_for_thread, interval) {
                break;
            }
        }
    });

    BackgroundQuotaRefreshWorker {
        stop,
        thread: Some(thread),
    }
}

pub(crate) fn start_background_quota_refresh_worker(
    state_db: PathBuf,
    secret_root: PathBuf,
    base_url: String,
    interval: Duration,
) -> Result<BackgroundQuotaRefreshWorker, QuotaCommandError> {
    let resolver = CliCredentialResolver::open(&state_db, &secret_root, current_unix_seconds())?;
    let provider = HttpQuotaRefreshProvider::new()?;
    Ok(start_background_quota_refresh_worker_with_reporter(
        state_db,
        secret_root,
        base_url,
        resolver,
        provider,
        BackgroundQuotaRefreshRuntime::new(
            current_unix_seconds,
            |diagnostic| eprintln!("{diagnostic}"),
            interval,
        ),
    ))
}

fn sleep_interruptibly(stop: &AtomicBool, interval: Duration) -> bool {
    let mut remaining = interval;
    while !stop.load(Ordering::SeqCst) {
        if remaining.is_zero() {
            return true;
        }
        let step = remaining.min(Duration::from_millis(50));
        thread::sleep(step);
        remaining = remaining.saturating_sub(step);
    }

    false
}

fn quota_response_for_route_band(
    usage: &UsageResponse,
    route_band: &str,
) -> Result<QuotaRefreshProviderResponse, QuotaCommandError> {
    let window_pair = match route_band {
        "code_review" => usage.code_review_rate_limit.as_ref(),
        _ => usage.rate_limit.as_ref(),
    }
    .ok_or_else(|| QuotaCommandError::ProviderResponse {
        message: format!("missing quota window for route band {route_band}"),
    })?;
    quota_response_from_window_pair(window_pair, route_band)
}

const fn stale_after_unix_seconds(observed_unix_seconds: u64) -> u64 {
    observed_unix_seconds.saturating_add(DEFAULT_REFRESH_STALE_AFTER_GRACE_SECONDS)
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
        QuotaCommandError::InvalidFormat { .. }
        | QuotaCommandError::DisallowedBaseUrl { .. }
        | QuotaCommandError::RefreshNotImplemented
        | QuotaCommandError::CredentialResolverOpen(_)
        | QuotaCommandError::StateStore(_)
        | QuotaCommandError::Runtime(_)
        | QuotaCommandError::Stdout(_) => QuotaRefreshErrorClass::ProviderError,
    }
}

fn quota_response_from_window_pair(
    window_pair: &WindowPair,
    route_band: &str,
) -> Result<QuotaRefreshProviderResponse, QuotaCommandError> {
    let mut windows = Vec::new();
    if let Some(primary_window) = window_pair.primary_window.as_ref() {
        windows.push(quota_provider_window_from_usage_window(
            primary_window,
            route_band,
            true,
        )?);
    }
    if let Some(secondary_window) = window_pair.secondary_window.as_ref() {
        windows.push(quota_provider_window_from_usage_window(
            secondary_window,
            route_band,
            window_pair.primary_window.is_none(),
        )?);
    }
    if windows.is_empty() {
        return Err(QuotaCommandError::ProviderResponse {
            message: format!("missing provider quota windows for route band {route_band}"),
        });
    }

    Ok(QuotaRefreshProviderResponse {
        windows,
        reset_credits_available: None,
    })
}

fn reset_credits_available_from_json(value: &Value) -> Option<u32> {
    match value {
        Value::Object(object) => {
            for (key, child) in object {
                let normalized_key = normalize_json_key(key);
                if matches!(
                    normalized_key.as_str(),
                    "resetcreditsavailable" | "availableresetcredits"
                ) && let Some(value) = json_u32(child)
                {
                    return Some(value);
                }
                if normalized_key == "resetcredits"
                    && let Some(value) = reset_credits_available_from_reset_credits_value(child)
                {
                    return Some(value);
                }
            }
            object.values().find_map(reset_credits_available_from_json)
        }
        Value::Array(values) => values.iter().find_map(reset_credits_available_from_json),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => None,
    }
}

fn reset_credits_available_from_reset_credits_value(value: &Value) -> Option<u32> {
    match value {
        Value::Number(_) | Value::String(_) => json_u32(value),
        Value::Object(object) => object.iter().find_map(|(key, child)| {
            let normalized_key = normalize_json_key(key);
            if matches!(normalized_key.as_str(), "available" | "remaining" | "count") {
                json_u32(child)
            } else {
                reset_credits_available_from_reset_credits_value(child)
            }
        }),
        Value::Array(values) => values
            .iter()
            .find_map(reset_credits_available_from_reset_credits_value),
        Value::Null | Value::Bool(_) => None,
    }
}

fn normalize_json_key(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn json_u32(value: &Value) -> Option<u32> {
    match value {
        Value::Number(number) => number.as_u64().and_then(|value| u32::try_from(value).ok()),
        Value::String(value) => value.trim().parse::<u32>().ok(),
        Value::Null | Value::Bool(_) | Value::Array(_) | Value::Object(_) => None,
    }
}

fn quota_provider_window_from_usage_window(
    window: &codex_router_auth::live_quota::UsageWindow,
    route_band: &str,
    effective: bool,
) -> Result<QuotaRefreshProviderWindow, QuotaCommandError> {
    let used_percent = window
        .used_percent
        .ok_or_else(|| QuotaCommandError::ProviderResponse {
            message: format!("missing used_percent for route band {route_band}"),
        })?
        .clamp(0, 100);
    let remaining_headroom = u32::try_from(100_i64 - used_percent).map_err(|_error| {
        QuotaCommandError::ProviderResponse {
            message: format!("invalid used_percent for route band {route_band}"),
        }
    })?;
    let limit_window_seconds = window
        .limit_window_seconds
        .and_then(|limit_window_seconds| u64::try_from(limit_window_seconds).ok())
        .ok_or_else(|| QuotaCommandError::ProviderResponse {
            message: format!("missing limit_window_seconds for route band {route_band}"),
        })?;
    let reset_unix_seconds = window
        .reset_at
        .and_then(|reset_at| u64::try_from(reset_at).ok());

    Ok(QuotaRefreshProviderWindow {
        limit_window_seconds,
        remaining_headroom,
        reset_unix_seconds,
        effective,
    })
}

fn current_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn render_quota_status(
    stdout: &mut impl Write,
    router_root: PathBuf,
    format: QuotaStatusFormat,
    all_limits: bool,
    now_unix_seconds: u64,
) -> Result<(), QuotaCommandError> {
    let state_db_path = router_root.join("state.sqlite");
    let state = SqliteStateStore::open(&state_db_path)?;
    let quota_history_runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(QuotaCommandError::Runtime)?;
    let quota_history_state =
        quota_history_runtime.block_on(AsyncSqliteStateStore::open(&state_db_path))?;
    let accounts = AccountStateRepository::list_accounts(&state)?;
    let unicode_bars = format != QuotaStatusFormat::Plain;
    let report = quota_status_report(
        &state,
        &quota_history_runtime,
        &quota_history_state,
        &accounts,
        all_limits,
        now_unix_seconds,
        unicode_bars,
    )?;
    match format {
        QuotaStatusFormat::Table => write_quota_table(stdout, report.rows()),
        QuotaStatusFormat::Plain => write_quota_plain(stdout, report.rows()),
        QuotaStatusFormat::Json => write_quota_json(stdout, &report),
    }
}

fn quota_status_report(
    state: &SqliteStateStore,
    quota_history_runtime: &tokio::runtime::Runtime,
    quota_history_state: &AsyncSqliteStateStore,
    accounts: &[AccountRecord],
    _all_limits: bool,
    now_unix_seconds: u64,
    unicode_bars: bool,
) -> Result<QuotaStatusReport, QuotaCommandError> {
    let selector_inputs = SelectorQuotaRepository::selector_inputs_for_route_band(
        state,
        USER_QUOTA_ROUTE_BAND,
        now_unix_seconds,
    )?;
    let mut status_inputs = Vec::new();
    let mut burn_down_inputs = Vec::new();
    for account in accounts {
        let selector_input = selector_inputs
            .iter()
            .find(|input| input.account_id() == account.account_id());
        let snapshot = QuotaSnapshotRepository::load_snapshot_for_route_band(
            state,
            account.account_id(),
            USER_QUOTA_ROUTE_BAND,
        )?;
        let reset_credits_available = snapshot
            .as_ref()
            .and_then(PersistedQuotaSnapshot::reset_credits_available);
        let mut display_windows = if let Some(selector_input) = selector_input {
            display_windows_from_selector_input(selector_input)
        } else {
            snapshot.as_ref().map_or_else(Vec::new, |snapshot| {
                vec![DisplayQuotaWindow::from_snapshot(snapshot)]
            })
        };
        attach_history_estimates_to_display_windows(
            quota_history_runtime,
            quota_history_state,
            account.account_id(),
            USER_QUOTA_ROUTE_BAND,
            now_unix_seconds,
            &mut display_windows,
        )?;
        burn_down_inputs.push(burn_down_input_from_display_windows(
            account,
            &display_windows,
        ));
        status_inputs.push(QuotaStatusAccountInput {
            account_label: account.label().to_owned(),
            account_status: account.status().as_str().to_owned(),
            account_id: account.account_id().clone(),
            reset_credits_available,
            windows: display_windows,
        });
    }

    let assessment = assess_route_band(BurnDownRouteBandAssessmentInput::new(
        RouteBand::Responses,
        now_unix_seconds,
        burn_down_inputs,
    ));
    let selected_pool = assessment.selected_pool();
    let preferred_next_account_id = assessment.preferred_next().cloned();
    let rows = status_inputs
        .iter()
        .filter_map(|input| {
            assessment
                .accounts()
                .iter()
                .find(|assessment| assessment.account_id() == &input.account_id)
                .map(|assessment| {
                    QuotaStatusRow::from_assessment(
                        input,
                        assessment,
                        now_unix_seconds,
                        unicode_bars,
                    )
                })
        })
        .collect::<Vec<_>>();

    Ok(QuotaStatusReport {
        route_band: USER_QUOTA_ROUTE_BAND.to_owned(),
        selected_pool,
        weighted_candidates: assessment.weighted_candidates().to_vec(),
        preferred_next_account_id,
        now_unix_seconds,
        rows,
    })
}

fn write_quota_table(
    stdout: &mut impl Write,
    rows: &[QuotaStatusRow],
) -> Result<(), QuotaCommandError> {
    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    table.set_header([
        "account",
        "status",
        "5h",
        "weekly",
        "pace",
        "burn",
        "resets available",
        "routing",
        "next use",
    ]);
    for row in rows {
        table.add_row([
            row.account_label.as_str(),
            row.account_status.as_str(),
            row.short_window.as_str(),
            row.weekly_window.as_str(),
            row.pace.as_str(),
            row.burn.as_str(),
            row.reset_credits_available.as_str(),
            row.routing.as_str(),
            row.next_use.as_str(),
        ]);
    }

    writeln!(stdout, "{table}").map_err(QuotaCommandError::Stdout)?;
    write_selector_summary_table(stdout, rows)
}

fn write_quota_plain(
    stdout: &mut impl Write,
    rows: &[QuotaStatusRow],
) -> Result<(), QuotaCommandError> {
    writeln!(
        stdout,
        "account\tstatus\t5h\tweekly\tpace\tburn\tresets available\trouting\tnext use"
    )
    .map_err(QuotaCommandError::Stdout)?;
    for row in rows {
        writeln!(
            stdout,
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            row.account_label,
            row.account_status,
            row.short_window.replace('\n', " "),
            row.weekly_window.replace('\n', " "),
            row.pace.replace('\n', " "),
            row.burn.replace('\n', " "),
            row.reset_credits_available,
            row.routing.replace('\n', " "),
            row.next_use,
        )
        .map_err(QuotaCommandError::Stdout)?;
    }

    write_selector_summary_plain(stdout, rows)
}

fn write_selector_summary_table(
    stdout: &mut impl Write,
    rows: &[QuotaStatusRow],
) -> Result<(), QuotaCommandError> {
    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    table.set_header(["route", "next", "why"]);
    let next = selected_account_label(rows).to_owned();
    let summary = selector_summary(rows);
    table.add_row(["responses".to_owned(), next, summary]);

    writeln!(stdout, "{table}").map_err(QuotaCommandError::Stdout)
}

fn write_selector_summary_plain(
    stdout: &mut impl Write,
    rows: &[QuotaStatusRow],
) -> Result<(), QuotaCommandError> {
    writeln!(
        stdout,
        "responses route\tnext: {}\twhy: {}",
        selected_account_label(rows),
        selector_summary(rows)
    )
    .map_err(QuotaCommandError::Stdout)
}

fn selected_account_label(rows: &[QuotaStatusRow]) -> &str {
    rows.iter()
        .find(|row| row.preferred_next)
        .map(|row| row.account_label.as_str())
        .unwrap_or("none")
}

fn selector_summary(rows: &[QuotaStatusRow]) -> String {
    let Some(selected_row) = rows.iter().find(|row| row.preferred_next) else {
        return "no usable accounts".to_owned();
    };
    selected_row.routing.replace('\n', " ")
}

fn write_quota_json(
    stdout: &mut impl Write,
    report: &QuotaStatusReport,
) -> Result<(), QuotaCommandError> {
    let json_report = JsonQuotaStatusReport::from_report(report);
    serde_json::to_writer_pretty(&mut *stdout, &json_report).map_err(|error| {
        QuotaCommandError::Stdout(std::io::Error::other(format!(
            "failed to serialize quota status json: {error}"
        )))
    })?;
    writeln!(stdout).map_err(QuotaCommandError::Stdout)
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct QuotaStatusReport {
    route_band: String,
    selected_pool: SelectedPool,
    weighted_candidates: Vec<(AccountId, u32)>,
    preferred_next_account_id: Option<AccountId>,
    now_unix_seconds: u64,
    rows: Vec<QuotaStatusRow>,
}

impl QuotaStatusReport {
    fn rows(&self) -> &[QuotaStatusRow] {
        &self.rows
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct QuotaStatusAccountInput {
    account_label: String,
    account_status: String,
    account_id: AccountId,
    reset_credits_available: Option<u32>,
    windows: Vec<DisplayQuotaWindow>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct QuotaStatusRow {
    account_id: AccountId,
    account_label: String,
    account_status: String,
    short_window: String,
    weekly_window: String,
    pace: String,
    burn: String,
    reset_credits_available: String,
    reset_credits_available_value: Option<u32>,
    routing: String,
    next_use: String,
    windows: Vec<DisplayQuotaWindow>,
    availability: AccountAvailability,
    freshness: QuotaEvidenceFreshness,
    routing_exclusion: RoutingExclusion,
    quota_evidence_reason: QuotaEvidenceReason,
    routing_reason: RoutingReason,
    routing_weight: Option<u32>,
    preferred_next: bool,
    short_pressure: u32,
    long_pressure: u32,
    short_salvage: u32,
    long_salvage: u32,
    limiting_window: Option<LimitingWindow>,
}

impl QuotaStatusRow {
    fn from_assessment(
        input: &QuotaStatusAccountInput,
        assessment: &BurnDownAccountAssessment,
        now_unix_seconds: u64,
        unicode_bars: bool,
    ) -> Self {
        Self {
            account_id: input.account_id.clone(),
            account_label: assessment.account_label().to_owned(),
            account_status: input.account_status.clone(),
            short_window: format_window_cell(
                &input.windows,
                V1_SHORT_WINDOW_SECONDS,
                now_unix_seconds,
                unicode_bars,
            ),
            weekly_window: format_window_cell(
                &input.windows,
                V1_WEEKLY_WINDOW_SECONDS,
                now_unix_seconds,
                unicode_bars,
            ),
            pace: format_pace_cell(&input.windows, assessment, now_unix_seconds),
            burn: format_burn_cell(assessment),
            reset_credits_available: format_reset_credits(input.reset_credits_available),
            reset_credits_available_value: input.reset_credits_available,
            routing: format_routing_cell(assessment),
            next_use: format_next_use(assessment).to_owned(),
            windows: input.windows.clone(),
            availability: assessment.availability(),
            freshness: assessment.freshness(),
            routing_exclusion: assessment.routing_exclusion(),
            quota_evidence_reason: assessment.quota_evidence_reason(),
            routing_reason: assessment.routing_reason(),
            routing_weight: assessment.routing_weight(),
            preferred_next: assessment.preferred_next(),
            short_pressure: assessment.short_pressure(),
            long_pressure: assessment.long_pressure(),
            short_salvage: assessment.short_salvage(),
            long_salvage: assessment.long_salvage(),
            limiting_window: assessment.limiting_window(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DisplayQuotaWindow {
    window_seconds: u64,
    status: QuotaWindowStatus,
    remaining_headroom: u32,
    reset_unix_seconds: Option<u64>,
    observed_unix_seconds: u64,
    effective: bool,
    run_rate_estimate: QuotaRunRateEstimate,
}

impl DisplayQuotaWindow {
    fn from_selector_window(window: &PersistedSelectorQuotaWindow) -> Self {
        Self {
            window_seconds: window.limit_window_seconds(),
            status: quota_window_status_from_selector_status(window.status()),
            remaining_headroom: window.remaining_headroom(),
            reset_unix_seconds: window.reset_unix_seconds(),
            observed_unix_seconds: window.observed_unix_seconds(),
            effective: window.effective(),
            run_rate_estimate: QuotaRunRateEstimate::unknown(),
        }
    }

    fn from_snapshot(snapshot: &PersistedQuotaSnapshot) -> Self {
        Self {
            window_seconds: V1_SHORT_WINDOW_SECONDS,
            status: if snapshot.stale_penalty() {
                QuotaWindowStatus::Stale
            } else {
                QuotaWindowStatus::Eligible
            },
            remaining_headroom: snapshot.remaining_headroom(),
            reset_unix_seconds: snapshot.reset_unix_seconds(),
            observed_unix_seconds: snapshot.observed_unix_seconds(),
            effective: true,
            run_rate_estimate: QuotaRunRateEstimate::unknown(),
        }
    }
}

fn display_windows_from_selector_input(input: &SelectorQuotaInput) -> Vec<DisplayQuotaWindow> {
    input
        .windows()
        .iter()
        .map(DisplayQuotaWindow::from_selector_window)
        .collect()
}

fn attach_history_estimates_to_display_windows(
    quota_history_runtime: &tokio::runtime::Runtime,
    quota_history_state: &AsyncSqliteStateStore,
    account_id: &AccountId,
    route_band: &str,
    now_unix_seconds: u64,
    windows: &mut [DisplayQuotaWindow],
) -> Result<(), QuotaCommandError> {
    let estimator = QuotaRunRateEstimator::new(DEFAULT_REFRESH_STALE_AFTER_GRACE_SECONDS);
    let observed_from_unix_seconds = now_unix_seconds.saturating_sub(V1_WEEKLY_WINDOW_SECONDS);
    for window in windows {
        let Some(reset_unix_seconds) = window.reset_unix_seconds else {
            continue;
        };
        let observations = quota_history_runtime.block_on(
            quota_history_state.quota_history_observations_for_window(
                account_id,
                route_band,
                window.window_seconds,
                observed_from_unix_seconds,
                now_unix_seconds,
            ),
        )?;
        let observations = observations
            .iter()
            .filter_map(quota_run_rate_observation_from_history)
            .collect::<Vec<_>>();
        window.run_rate_estimate =
            estimator.estimate(now_unix_seconds, reset_unix_seconds, &observations);
    }

    Ok(())
}

fn quota_run_rate_observation_from_history(
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

fn burn_down_input_from_display_windows(
    account: &AccountRecord,
    windows: &[DisplayQuotaWindow],
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
            fact
        })
        .collect::<Vec<_>>();

    BurnDownAccountInput::new(account.account_id().clone(), account.label(), facts)
        .with_account_enabled(account.status() == AccountStatus::Enabled)
        .with_active_credential(account.active_credential_generation().is_some())
}

fn format_window_cell(
    windows: &[DisplayQuotaWindow],
    window_seconds: u64,
    now_unix_seconds: u64,
    unicode: bool,
) -> String {
    let Some(window) = windows
        .iter()
        .find(|window| window.window_seconds == window_seconds)
    else {
        return format!("{} no data\nneeds refresh", quota_bar(0, unicode));
    };
    format!(
        "{} {} left\n{}",
        quota_bar(window.remaining_headroom, unicode),
        format_percent(window.remaining_headroom),
        window_display_note(window, now_unix_seconds)
    )
}

fn quota_bar(percent: u32, unicode: bool) -> String {
    let filled = percent.min(100).div_ceil(10) as usize;
    let empty = 10_usize.saturating_sub(filled);
    if unicode {
        format!("{}{}", "█".repeat(filled), "░".repeat(empty))
    } else {
        format!("{}{}", "#".repeat(filled), "-".repeat(empty))
    }
}

fn format_reset_credits(reset_credits_available: Option<u32>) -> String {
    reset_credits_available.map_or_else(
        || "-".to_owned(),
        |credits| {
            if credits == 1 {
                "1 available".to_owned()
            } else {
                format!("{credits} available")
            }
        },
    )
}

fn format_pace_cell(
    windows: &[DisplayQuotaWindow],
    assessment: &BurnDownAccountAssessment,
    now_unix_seconds: u64,
) -> String {
    if matches!(
        assessment.quota_evidence_reason(),
        QuotaEvidenceReason::NeedsQuotaProbe
            | QuotaEvidenceReason::MissingExpectedWindow
            | QuotaEvidenceReason::UnknownQuotaWindow
            | QuotaEvidenceReason::MissingResetTime
    ) {
        return "needs refresh".to_owned();
    }
    let short = format_window_pace(windows, V1_SHORT_WINDOW_SECONDS, "5h", now_unix_seconds);
    let weekly = format_window_pace(
        windows,
        V1_WEEKLY_WINDOW_SECONDS,
        "weekly",
        now_unix_seconds,
    );
    format!("{short}\n{weekly}")
}

fn format_burn_cell(assessment: &BurnDownAccountAssessment) -> String {
    if matches!(
        assessment.quota_evidence_reason(),
        QuotaEvidenceReason::NeedsQuotaProbe
            | QuotaEvidenceReason::MissingExpectedWindow
            | QuotaEvidenceReason::UnknownQuotaWindow
            | QuotaEvidenceReason::MissingResetTime
    ) {
        return "needs refresh".to_owned();
    }

    let score = assessment.routing_weight().map_or_else(
        || "not selectable".to_owned(),
        |weight| format!("score {weight}"),
    );
    format!(
        "{score}\nrisk 5h {}% / weekly {}%",
        assessment.short_pressure(),
        assessment.long_pressure()
    )
}

fn format_window_pace(
    windows: &[DisplayQuotaWindow],
    window_seconds: u64,
    label: &'static str,
    now_unix_seconds: u64,
) -> String {
    let Some(window) = windows
        .iter()
        .find(|window| window.window_seconds == window_seconds)
    else {
        return format!("{label} needs refresh");
    };
    match window.status {
        QuotaWindowStatus::Unknown => format!("{label} needs refresh"),
        QuotaWindowStatus::Ineligible if window.remaining_headroom == 0 => {
            format!("{label} empty")
        }
        QuotaWindowStatus::Ineligible => format!("{label} ineligible"),
        QuotaWindowStatus::Eligible | QuotaWindowStatus::Stale => {
            let (pressure, surplus) = window_pressure_and_surplus(window, now_unix_seconds);
            let pace = match (pressure.unwrap_or(0), surplus.unwrap_or(0)) {
                (0, 0) => format!("{label} on pace"),
                (behind, 0) => format!("{label} {behind}% behind"),
                (0, ahead) => format!("{label} {ahead}% ahead"),
                _ => format!("{label} needs refresh"),
            };
            format!(
                "{pace}; {}",
                format_run_rate_estimate(window.run_rate_estimate, now_unix_seconds)
            )
        }
    }
}

fn format_run_rate_estimate(estimate: QuotaRunRateEstimate, now_unix_seconds: u64) -> String {
    match estimate.confidence() {
        QuotaRunRateConfidence::Unknown => "history unknown".to_owned(),
        QuotaRunRateConfidence::Insufficient => "history insufficient".to_owned(),
        QuotaRunRateConfidence::Stale => "history stale".to_owned(),
        QuotaRunRateConfidence::Low | QuotaRunRateConfidence::Normal => {
            let confidence = run_rate_confidence_label(estimate.confidence());
            let burn_rate = estimate.burn_rate_percent_per_hour().unwrap_or(0);
            match estimate.projected_exhaustion_unix_seconds(now_unix_seconds) {
                Some(runout) => {
                    format!(
                        "{confidence} burn {burn_rate}%/h; runout {}",
                        format_relative_time(runout, now_unix_seconds)
                    )
                }
                None => format!("{confidence} burn {burn_rate}%/h; no runout"),
            }
        }
    }
}

const fn run_rate_confidence_label(confidence: QuotaRunRateConfidence) -> &'static str {
    match confidence {
        QuotaRunRateConfidence::Unknown => "unknown",
        QuotaRunRateConfidence::Insufficient => "insufficient",
        QuotaRunRateConfidence::Low => "low",
        QuotaRunRateConfidence::Normal => "normal",
        QuotaRunRateConfidence::Stale => "stale",
    }
}

fn format_routing_cell(assessment: &BurnDownAccountAssessment) -> String {
    let first_line = assessment.routing_reason().human_phrase();
    if let Some(limiting_window) = assessment.limiting_window() {
        format!(
            "{first_line}\nlimiting window: {} {} left",
            quota_window_label(limiting_window.window_seconds()),
            format_percent(limiting_window.remaining_headroom())
        )
    } else {
        first_line.to_owned()
    }
}

fn format_next_use(assessment: &BurnDownAccountAssessment) -> &'static str {
    match assessment.routing_reason() {
        RoutingReason::PreferredWeeklyHealthier
        | RoutingReason::PreferredWeeklyResetSoon
        | RoutingReason::PreferredShortResetSoon
        | RoutingReason::PreferredProjectedBurn
        | RoutingReason::PreferredHighestWeight => "preferred",
        RoutingReason::AvailableSamePool => "available",
        RoutingReason::HeldReserve | RoutingReason::HeldUnknown => "held",
        RoutingReason::UnknownFallbackPreferred | RoutingReason::UnknownFallbackAvailable => {
            "fallback"
        }
        RoutingReason::ExcludedDisabled
        | RoutingReason::ExcludedMissingCredential
        | RoutingReason::BlockedWindowExhausted
        | RoutingReason::BlockedWindowIneligible => "blocked",
    }
}

fn format_percent(value: u32) -> String {
    format!("{}%", value.min(100))
}

#[derive(Serialize)]
struct JsonQuotaStatusReport {
    route_result: &'static str,
    route_band: String,
    selected_pool: &'static str,
    selected_pool_reason: &'static str,
    preferred_next_account_id: Option<String>,
    weighted_candidates: Vec<JsonWeightedCandidate>,
    accounts: Vec<JsonQuotaStatusAccount>,
}

impl JsonQuotaStatusReport {
    fn from_report(report: &QuotaStatusReport) -> Self {
        Self {
            route_result: "ok",
            route_band: report.route_band.clone(),
            selected_pool: selected_pool_json(report.selected_pool),
            selected_pool_reason: selected_pool_reason_json(report.selected_pool),
            preferred_next_account_id: report
                .preferred_next_account_id
                .as_ref()
                .map(|account_id| account_id.as_str().to_owned()),
            weighted_candidates: report
                .weighted_candidates
                .iter()
                .map(|(account_id, routing_weight)| JsonWeightedCandidate {
                    account_id: account_id.as_str().to_owned(),
                    routing_weight: *routing_weight,
                })
                .collect(),
            accounts: report
                .rows
                .iter()
                .map(|row| JsonQuotaStatusAccount::from_row(row, report.now_unix_seconds))
                .collect(),
        }
    }
}

#[derive(Serialize)]
struct JsonWeightedCandidate {
    account_id: String,
    routing_weight: u32,
}

#[derive(Serialize)]
struct JsonQuotaStatusAccount {
    account_id: String,
    safe_account_label: String,
    availability: &'static str,
    freshness: &'static str,
    routing_exclusion: &'static str,
    next_use: String,
    limiting_window: &'static str,
    quota_evidence_reason: &'static str,
    short_pressure: Option<u32>,
    long_pressure: Option<u32>,
    short_salvage: Option<u32>,
    long_salvage: Option<u32>,
    salvage_tie_key: Option<JsonSalvageTieKey>,
    routing_reason: &'static str,
    routing_weight: Option<u32>,
    preferred_next: bool,
    reset_credits_available: Option<u32>,
    window_slots: JsonWindowSlots,
    windows: Vec<JsonQuotaWindow>,
}

impl JsonQuotaStatusAccount {
    fn from_row(row: &QuotaStatusRow, now_unix_seconds: u64) -> Self {
        Self {
            account_id: row.account_id.as_str().to_owned(),
            safe_account_label: row.account_label.clone(),
            availability: availability_json(row.availability),
            freshness: freshness_json(row.freshness),
            routing_exclusion: routing_exclusion_json(row.routing_exclusion),
            next_use: row.next_use.clone(),
            limiting_window: row
                .limiting_window
                .map_or("none", |window| quota_window_label(window.window_seconds())),
            quota_evidence_reason: quota_evidence_reason_json(row.quota_evidence_reason),
            short_pressure: Some(row.short_pressure),
            long_pressure: Some(row.long_pressure),
            short_salvage: Some(row.short_salvage),
            long_salvage: Some(row.long_salvage),
            salvage_tie_key: None,
            routing_reason: routing_reason_json(row.routing_reason),
            routing_weight: row.routing_weight,
            preferred_next: row.preferred_next,
            reset_credits_available: row.reset_credits_available_value,
            window_slots: JsonWindowSlots::from_windows(&row.windows, now_unix_seconds),
            windows: row
                .windows
                .iter()
                .map(|window| JsonQuotaWindow::from_window(window, now_unix_seconds))
                .collect(),
        }
    }
}

#[derive(Serialize)]
struct JsonSalvageTieKey {
    reset_unix_seconds: u64,
    window_seconds: u64,
}

#[derive(Serialize)]
struct JsonWindowSlots {
    #[serde(rename = "5h")]
    short: JsonWindowSlot,
    weekly: JsonWindowSlot,
}

impl JsonWindowSlots {
    fn from_windows(windows: &[DisplayQuotaWindow], now_unix_seconds: u64) -> Self {
        Self {
            short: JsonWindowSlot::from_windows(windows, V1_SHORT_WINDOW_SECONDS, now_unix_seconds),
            weekly: JsonWindowSlot::from_windows(
                windows,
                V1_WEEKLY_WINDOW_SECONDS,
                now_unix_seconds,
            ),
        }
    }
}

#[derive(Serialize)]
struct JsonWindowSlot {
    slot: &'static str,
    evidence_state: &'static str,
    remaining_headroom: Option<u32>,
    reset_unix_seconds: Option<u64>,
    reset_duration_seconds: Option<u64>,
    display_note: String,
    run_rate: JsonRunRateEstimate,
}

impl JsonWindowSlot {
    fn from_windows(
        windows: &[DisplayQuotaWindow],
        window_seconds: u64,
        now_unix_seconds: u64,
    ) -> Self {
        let Some(window) = windows
            .iter()
            .find(|window| window.window_seconds == window_seconds)
        else {
            return Self {
                slot: quota_window_label(window_seconds),
                evidence_state: "no_data",
                remaining_headroom: None,
                reset_unix_seconds: None,
                reset_duration_seconds: None,
                display_note: "needs refresh".to_owned(),
                run_rate: JsonRunRateEstimate::unknown(),
            };
        };
        let reset_duration_seconds = window
            .reset_unix_seconds
            .map(|reset_unix_seconds| reset_unix_seconds.saturating_sub(now_unix_seconds));
        let display_note = window_display_note(window, now_unix_seconds);
        Self {
            slot: quota_window_label(window_seconds),
            evidence_state: window_evidence_state(window.status),
            remaining_headroom: window_known_headroom(window),
            reset_unix_seconds: window.reset_unix_seconds,
            reset_duration_seconds,
            display_note,
            run_rate: JsonRunRateEstimate::from_estimate(
                window.run_rate_estimate,
                now_unix_seconds,
            ),
        }
    }
}

#[derive(Serialize)]
struct JsonRunRateEstimate {
    confidence: &'static str,
    burn_rate_percent_per_hour: Option<u32>,
    projected_exhaustion_unix_seconds: Option<u64>,
}

impl JsonRunRateEstimate {
    fn unknown() -> Self {
        Self {
            confidence: "unknown",
            burn_rate_percent_per_hour: None,
            projected_exhaustion_unix_seconds: None,
        }
    }

    fn from_estimate(estimate: QuotaRunRateEstimate, now_unix_seconds: u64) -> Self {
        Self {
            confidence: run_rate_confidence_label(estimate.confidence()),
            burn_rate_percent_per_hour: estimate.burn_rate_percent_per_hour(),
            projected_exhaustion_unix_seconds: estimate
                .projected_exhaustion_unix_seconds(now_unix_seconds),
        }
    }
}

#[derive(Serialize)]
struct JsonQuotaWindow {
    window_seconds: u64,
    status: &'static str,
    remaining_headroom: Option<u32>,
    reset_unix_seconds: Option<u64>,
    observed_unix_seconds: Option<u64>,
    effective: bool,
    pressure_percent: Option<u32>,
    surplus_percent: Option<u32>,
    contributed_to_salvage: bool,
    run_rate: JsonRunRateEstimate,
}

impl JsonQuotaWindow {
    fn from_window(window: &DisplayQuotaWindow, now_unix_seconds: u64) -> Self {
        let (pressure_percent, surplus_percent) =
            window_pressure_and_surplus(window, now_unix_seconds);
        Self {
            window_seconds: window.window_seconds,
            status: quota_window_status_json(window.status),
            remaining_headroom: window_known_headroom(window),
            reset_unix_seconds: window.reset_unix_seconds,
            observed_unix_seconds: Some(window.observed_unix_seconds),
            effective: window.effective,
            pressure_percent,
            surplus_percent,
            contributed_to_salvage: surplus_percent.is_some_and(|surplus| surplus > 0),
            run_rate: JsonRunRateEstimate::from_estimate(
                window.run_rate_estimate,
                now_unix_seconds,
            ),
        }
    }
}

const fn selected_pool_json(value: SelectedPool) -> &'static str {
    match value {
        SelectedPool::Usable => "usable",
        SelectedPool::Reserve => "reserve",
        SelectedPool::Unknown => "unknown",
        SelectedPool::None => "none",
    }
}

const fn selected_pool_reason_json(value: SelectedPool) -> &'static str {
    match value {
        SelectedPool::Usable => "usable_available",
        SelectedPool::Reserve => "reserve_only",
        SelectedPool::Unknown => "unknown_fallback_only",
        SelectedPool::None => "none_available",
    }
}

const fn availability_json(value: AccountAvailability) -> &'static str {
    match value {
        AccountAvailability::Usable => "usable",
        AccountAvailability::Reserve => "reserve",
        AccountAvailability::Blocked => "blocked",
        AccountAvailability::Unknown => "unknown",
        AccountAvailability::Excluded => "excluded",
    }
}

const fn freshness_json(value: QuotaEvidenceFreshness) -> &'static str {
    match value {
        QuotaEvidenceFreshness::Fresh => "fresh",
        QuotaEvidenceFreshness::Stale => "stale",
        QuotaEvidenceFreshness::Unknown => "unknown",
    }
}

const fn routing_exclusion_json(value: RoutingExclusion) -> &'static str {
    match value {
        RoutingExclusion::None => "none",
        RoutingExclusion::Disabled => "disabled",
        RoutingExclusion::MissingCredential => "missing_credential",
    }
}

const fn quota_evidence_reason_json(value: QuotaEvidenceReason) -> &'static str {
    match value {
        QuotaEvidenceReason::Ok => "none",
        QuotaEvidenceReason::NeedsQuotaProbe => "needs_quota_refresh",
        QuotaEvidenceReason::MissingExpectedWindow => "missing_expected_window",
        QuotaEvidenceReason::WindowIneligible => "window_ineligible",
        QuotaEvidenceReason::WindowExhausted => "window_exhausted",
        QuotaEvidenceReason::UnknownQuotaWindow => "unknown_quota_window",
        QuotaEvidenceReason::MissingResetTime => "missing_reset_time",
        QuotaEvidenceReason::AccountDisabled => "account_disabled",
        QuotaEvidenceReason::MissingCredential => "missing_credential",
    }
}

const fn routing_reason_json(value: RoutingReason) -> &'static str {
    value.as_str()
}

const fn quota_window_status_json(value: QuotaWindowStatus) -> &'static str {
    match value {
        QuotaWindowStatus::Eligible => "eligible",
        QuotaWindowStatus::Stale => "stale",
        QuotaWindowStatus::Unknown => "unknown",
        QuotaWindowStatus::Ineligible => "ineligible",
    }
}

const fn window_evidence_state(value: QuotaWindowStatus) -> &'static str {
    match value {
        QuotaWindowStatus::Eligible | QuotaWindowStatus::Stale | QuotaWindowStatus::Ineligible => {
            "known"
        }
        QuotaWindowStatus::Unknown => "unknown",
    }
}

fn window_known_headroom(window: &DisplayQuotaWindow) -> Option<u32> {
    match window.status {
        QuotaWindowStatus::Unknown => None,
        QuotaWindowStatus::Eligible | QuotaWindowStatus::Stale | QuotaWindowStatus::Ineligible => {
            Some(window.remaining_headroom)
        }
    }
}

fn window_display_note(window: &DisplayQuotaWindow, now_unix_seconds: u64) -> String {
    let reset = window.reset_unix_seconds.map_or_else(
        || "reset unknown".to_owned(),
        |reset| format!("resets {}", format_relative_time(reset, now_unix_seconds)),
    );
    match window.status {
        QuotaWindowStatus::Eligible => reset,
        QuotaWindowStatus::Stale => format!("{reset}; needs refresh"),
        QuotaWindowStatus::Unknown => "unknown; needs refresh".to_owned(),
        QuotaWindowStatus::Ineligible if window.remaining_headroom == 0 => "empty".to_owned(),
        QuotaWindowStatus::Ineligible => "quota ineligible".to_owned(),
    }
}

fn window_pressure_and_surplus(
    window: &DisplayQuotaWindow,
    now_unix_seconds: u64,
) -> (Option<u32>, Option<u32>) {
    if window.status == QuotaWindowStatus::Unknown {
        return (None, None);
    }
    let Some(reset_unix_seconds) = window.reset_unix_seconds else {
        return (None, None);
    };
    let time_left_seconds = reset_unix_seconds
        .saturating_sub(now_unix_seconds)
        .min(window.window_seconds);
    let expected_remaining_percent = time_left_seconds
        .saturating_mul(100)
        .saturating_add(window.window_seconds.saturating_sub(1))
        / window.window_seconds;
    let expected_remaining_percent = u32::try_from(expected_remaining_percent)
        .unwrap_or(u32::MAX)
        .min(100);
    let remaining_headroom = window.remaining_headroom.min(100);

    (
        Some(expected_remaining_percent.saturating_sub(remaining_headroom)),
        Some(remaining_headroom.saturating_sub(expected_remaining_percent)),
    )
}

fn format_relative_time(target_unix_seconds: u64, now_unix_seconds: u64) -> String {
    if target_unix_seconds >= now_unix_seconds {
        format!(
            "in {}",
            format_duration(target_unix_seconds.saturating_sub(now_unix_seconds))
        )
    } else {
        format!(
            "{} ago",
            format_duration(now_unix_seconds.saturating_sub(target_unix_seconds))
        )
    }
}

fn format_duration(seconds: u64) -> String {
    const MINUTE: u64 = 60;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;

    if seconds >= DAY {
        let days = seconds / DAY;
        let hours = (seconds % DAY) / HOUR;
        if hours == 0 {
            format!("{days}d")
        } else {
            format!("{days}d {hours}h")
        }
    } else if seconds >= HOUR {
        let hours = seconds / HOUR;
        let minutes = (seconds % HOUR) / MINUTE;
        if minutes == 0 {
            format!("{hours}h")
        } else {
            format!("{hours}h {minutes}m")
        }
    } else if seconds >= MINUTE {
        let minutes = seconds / MINUTE;
        let remaining_seconds = seconds % MINUTE;
        if remaining_seconds == 0 {
            format!("{minutes}m")
        } else {
            format!("{minutes}m {remaining_seconds}s")
        }
    } else {
        format!("{seconds}s")
    }
}

const fn quota_window_status_from_selector_status(
    status: SelectorQuotaWindowStatus,
) -> QuotaWindowStatus {
    match status {
        SelectorQuotaWindowStatus::Eligible => QuotaWindowStatus::Eligible,
        SelectorQuotaWindowStatus::Stale => QuotaWindowStatus::Stale,
        SelectorQuotaWindowStatus::Unknown => QuotaWindowStatus::Unknown,
        SelectorQuotaWindowStatus::Ineligible => QuotaWindowStatus::Ineligible,
    }
}

fn quota_window_label(limit_window_seconds: u64) -> &'static str {
    match limit_window_seconds {
        18_000 => "5h",
        86_400 => "daily",
        604_800 => "weekly",
        2_592_000 => "monthly",
        _ => "window",
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct QuotaStatusOptions {
    router_root: Option<PathBuf>,
    format: QuotaStatusFormat,
    all_limits: bool,
    now_unix_seconds: u64,
}

impl Default for QuotaStatusOptions {
    fn default() -> Self {
        Self {
            router_root: None,
            format: QuotaStatusFormat::Table,
            all_limits: false,
            now_unix_seconds: current_unix_seconds(),
        }
    }
}

impl QuotaStatusOptions {
    fn parse(parser: &mut ArgumentParser) -> Result<Self, CliError> {
        let mut options = Self::default();

        while let Some(argument) = parser.next_string()? {
            match argument.as_str() {
                "--router-root" => {
                    options.router_root =
                        Some(PathBuf::from(parser.next_required_value("--router-root")?));
                }
                "--format" => {
                    let value = parser.next_required_value("--format")?;
                    options.format = parse_quota_status_format(&value)?;
                }
                "--all-limits" => {
                    options.all_limits = true;
                }
                "--now-unix-seconds" => {
                    let value = parser.next_required_value("--now-unix-seconds")?;
                    options.now_unix_seconds =
                        value
                            .parse::<u64>()
                            .map_err(|_| CliError::InvalidNumericOption {
                                option: "--now-unix-seconds",
                                value,
                            })?;
                }
                unknown => {
                    return Err(CliError::UnknownOption {
                        option: unknown.to_owned(),
                    });
                }
            }
        }

        Ok(options)
    }

    fn router_root(&self) -> Result<PathBuf, CliError> {
        router_root_or_default(self.router_root.clone())
    }
}

fn parse_quota_status_format(value: &str) -> Result<QuotaStatusFormat, CliError> {
    match value {
        "table" => Ok(QuotaStatusFormat::Table),
        "plain" => Ok(QuotaStatusFormat::Plain),
        "json" => Ok(QuotaStatusFormat::Json),
        unknown => Err(CliError::Quota(QuotaCommandError::InvalidFormat {
            value: unknown.to_owned(),
        })),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct QuotaRefreshOptions {
    router_root: Option<PathBuf>,
    base_url: String,
}

impl Default for QuotaRefreshOptions {
    fn default() -> Self {
        Self {
            router_root: None,
            base_url: DEFAULT_CHATGPT_BACKEND_BASE_URL.to_owned(),
        }
    }
}

impl QuotaRefreshOptions {
    fn parse(parser: &mut ArgumentParser) -> Result<Self, CliError> {
        let mut options = Self::default();

        while let Some(argument) = parser.next_string()? {
            match argument.as_str() {
                "--router-root" => {
                    options.router_root =
                        Some(PathBuf::from(parser.next_required_value("--router-root")?));
                }
                "--base-url" => {
                    options.base_url = parser.next_required_value("--base-url")?;
                }
                unknown => {
                    return Err(CliError::UnknownOption {
                        option: unknown.to_owned(),
                    });
                }
            }
        }

        Ok(options)
    }

    fn router_root(&self) -> Result<PathBuf, CliError> {
        router_root_or_default(self.router_root.clone())
    }
}

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
use codex_router_state::account::AccountRecord;
use codex_router_state::account::AccountStatus;
use codex_router_state::quota_snapshot::PersistedQuotaSnapshot;
use codex_router_state::quota_snapshot::PersistedSelectorQuotaWindow;
use codex_router_state::quota_snapshot::QuotaSnapshotSource;
use codex_router_state::quota_snapshot::SelectorQuotaInput;
use codex_router_state::quota_snapshot::SelectorQuotaWindowStatus;
use codex_router_state::repositories::AccountStateRepository;
use codex_router_state::repositories::QuotaSnapshotRepository;
use codex_router_state::repositories::SelectorQuotaRepository;
use codex_router_state::sqlite::SqliteStateStore;
use codex_router_state::sqlite::StateStoreError;
use comfy_table::Table;
use comfy_table::presets::UTF8_FULL;
use thiserror::Error;

use crate::ArgumentParser;
use crate::CliError;
use crate::credential_runtime::CliCredentialResolver;
use crate::credential_runtime::CliCredentialResolverOpenError;
use crate::router_root_or_default;

const DEFAULT_ROUTE_BANDS: &[&str] = &["responses", "models"];
const USER_QUOTA_ROUTE_BAND: &str = "responses";
const USER_QUOTA_WINDOWS: [u64; 2] = [18_000, 604_800];

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

fn is_allowed_quota_refresh_base_url(base_url: &str) -> bool {
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
        let usage = serde_json::from_str::<UsageResponse>(&body).map_err(|error| {
            QuotaCommandError::ProviderResponse {
                message: error.to_string(),
            }
        })?;
        quota_response_for_route_band(&usage, request.route_band())
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
            let effective_window =
                response
                    .effective_window()
                    .ok_or_else(|| QuotaCommandError::ProviderResponse {
                        message: format!(
                            "missing provider quota windows for route band {route_band}"
                        ),
                    })?;
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
            QuotaSnapshotRepository::upsert_snapshot(&state, &snapshot)?;
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
                SelectorQuotaRepository::upsert_selector_window(&state, &selector_window)?;
            }
            refreshed_count = refreshed_count.saturating_add(1);
        }
    }

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

    Ok(QuotaRefreshProviderResponse { windows })
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
    let state = SqliteStateStore::open(&router_root.join("state.sqlite"))?;
    let accounts = AccountStateRepository::list_accounts(&state)?;
    let rows = quota_status_rows(&state, &accounts, all_limits, now_unix_seconds)?;
    match format {
        QuotaStatusFormat::Table => write_quota_table(stdout, &rows),
        QuotaStatusFormat::Plain => write_quota_plain(stdout, &rows),
    }
}

fn quota_status_rows(
    state: &SqliteStateStore,
    accounts: &[AccountRecord],
    _all_limits: bool,
    now_unix_seconds: u64,
) -> Result<Vec<QuotaStatusRow>, QuotaCommandError> {
    let mut rows = Vec::new();
    let selector_inputs =
        SelectorQuotaRepository::selector_inputs_for_route_band(state, USER_QUOTA_ROUTE_BAND)?;
    for account in accounts {
        let selector_input = selector_inputs
            .iter()
            .find(|input| input.account_id() == account.account_id());
        if let Some(selector_input) = selector_input {
            rows.push(QuotaStatusRow::from_selector_input(
                selector_input,
                now_unix_seconds,
            ));
            continue;
        }

        if let Some(snapshot) = QuotaSnapshotRepository::load_snapshot_for_route_band(
            state,
            account.account_id(),
            USER_QUOTA_ROUTE_BAND,
        )? {
            rows.push(QuotaStatusRow::from_snapshot(
                account,
                &snapshot,
                now_unix_seconds,
            ));
        } else {
            rows.push(QuotaStatusRow::missing(account));
        }
    }
    mark_next_usable_account(&mut rows);

    Ok(rows)
}

fn write_quota_table(
    stdout: &mut impl Write,
    rows: &[QuotaStatusRow],
) -> Result<(), QuotaCommandError> {
    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    table.set_header(["account", "status", "5h", "weekly", "routing", "next use"]);
    for row in rows {
        table.add_row([
            row.account_label.as_str(),
            row.account_status.as_str(),
            row.five_hour.as_str(),
            row.weekly.as_str(),
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
    writeln!(stdout, "account\tstatus\t5h\tweekly\trouting\tnext use")
        .map_err(QuotaCommandError::Stdout)?;
    for row in rows {
        writeln!(
            stdout,
            "{}\t{}\t{}\t{}\t{}\t{}",
            row.account_label,
            row.account_status,
            row.five_hour,
            row.weekly,
            row.routing,
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
    let summary = selector_summary(rows);
    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    table.set_header(["route", "next", "why"]);
    table.add_row(["responses", summary.next.as_str(), summary.why.as_str()]);
    writeln!(stdout, "{table}").map_err(QuotaCommandError::Stdout)
}

fn write_selector_summary_plain(
    stdout: &mut impl Write,
    rows: &[QuotaStatusRow],
) -> Result<(), QuotaCommandError> {
    let summary = selector_summary(rows);
    writeln!(
        stdout,
        "responses route\tnext: {}\twhy: {}",
        summary.next, summary.why
    )
    .map_err(QuotaCommandError::Stdout)
}

fn selector_summary(rows: &[QuotaStatusRow]) -> QuotaSelectorSummary {
    if let Some(row) = rows.iter().find(|row| row.next_use == "next") {
        return QuotaSelectorSummary {
            next: row.account_label.clone(),
            why: format!(
                "highest usable bottleneck {}%",
                row.bottleneck_headroom.unwrap_or(0)
            ),
        };
    }

    QuotaSelectorSummary {
        next: "none".to_owned(),
        why: "no usable accounts".to_owned(),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct QuotaSelectorSummary {
    next: String,
    why: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct QuotaStatusRow {
    account_label: String,
    account_status: String,
    five_hour: String,
    weekly: String,
    routing: String,
    next_use: String,
    bottleneck_headroom: Option<u32>,
}

impl QuotaStatusRow {
    fn from_selector_input(input: &SelectorQuotaInput, now_unix_seconds: u64) -> Self {
        let five_hour = input
            .windows()
            .iter()
            .find(|window| window.limit_window_seconds() == USER_QUOTA_WINDOWS[0])
            .map(|window| QuotaWindowCell::from_selector_window(window, now_unix_seconds))
            .unwrap_or_else(QuotaWindowCell::missing);
        let weekly = input
            .windows()
            .iter()
            .find(|window| window.limit_window_seconds() == USER_QUOTA_WINDOWS[1])
            .map(|window| QuotaWindowCell::from_selector_window(window, now_unix_seconds))
            .unwrap_or_else(QuotaWindowCell::missing);
        let routing = AccountRoutingStatus::from_windows(&five_hour, &weekly);

        Self {
            account_label: input.account_label().to_owned(),
            account_status: input.account_status().as_str().to_owned(),
            five_hour: five_hour.render(),
            weekly: weekly.render(),
            routing: routing.render(),
            next_use: if routing.usable { "backup" } else { "no" }.to_owned(),
            bottleneck_headroom: routing.bottleneck_headroom,
        }
    }

    fn from_snapshot(
        account: &AccountRecord,
        snapshot: &PersistedQuotaSnapshot,
        now_unix_seconds: u64,
    ) -> Self {
        let five_hour = QuotaWindowCell::from_snapshot(snapshot, now_unix_seconds);
        let weekly = QuotaWindowCell::missing();
        let routing = AccountRoutingStatus::from_windows(&five_hour, &weekly);

        Self {
            account_label: account.label().to_owned(),
            account_status: account.status().as_str().to_owned(),
            five_hour: five_hour.render(),
            weekly: weekly.render(),
            routing: routing.render(),
            next_use: "no".to_owned(),
            bottleneck_headroom: routing.bottleneck_headroom,
        }
    }

    fn missing(account: &AccountRecord) -> Self {
        let five_hour = QuotaWindowCell::missing();
        let weekly = QuotaWindowCell::missing();
        let routing = AccountRoutingStatus::from_windows(&five_hour, &weekly);

        Self {
            account_label: account.label().to_owned(),
            account_status: account.status().as_str().to_owned(),
            five_hour: five_hour.render(),
            weekly: weekly.render(),
            routing: routing.render(),
            next_use: "no".to_owned(),
            bottleneck_headroom: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct QuotaWindowCell {
    remaining_headroom: Option<u32>,
    reset: Option<String>,
    state: QuotaWindowCellState,
}

impl QuotaWindowCell {
    fn from_selector_window(window: &PersistedSelectorQuotaWindow, now_unix_seconds: u64) -> Self {
        Self {
            remaining_headroom: Some(window.remaining_headroom()),
            reset: window
                .reset_unix_seconds()
                .map(|reset| format_relative_time(reset, now_unix_seconds)),
            state: QuotaWindowCellState::from_selector_window(window),
        }
    }

    fn from_snapshot(snapshot: &PersistedQuotaSnapshot, now_unix_seconds: u64) -> Self {
        let state = if snapshot.stale_penalty() {
            QuotaWindowCellState::NeedsRefresh
        } else if snapshot.remaining_headroom() == 0 {
            QuotaWindowCellState::Empty
        } else {
            QuotaWindowCellState::Ready
        };
        Self {
            remaining_headroom: Some(snapshot.remaining_headroom()),
            reset: snapshot
                .reset_unix_seconds()
                .map(|reset| format_relative_time(reset, now_unix_seconds)),
            state,
        }
    }

    const fn missing() -> Self {
        Self {
            remaining_headroom: None,
            reset: None,
            state: QuotaWindowCellState::NeedsRefresh,
        }
    }

    fn render(&self) -> String {
        match self.remaining_headroom {
            Some(remaining_headroom) => {
                let reset = self.reset.as_ref().map_or_else(
                    || "reset unknown".to_owned(),
                    |reset| format!("resets {reset}"),
                );
                format!(
                    "{} {} {reset}",
                    quota_bar(remaining_headroom),
                    format_percent(remaining_headroom)
                )
            }
            None => format!("{} - needs refresh", quota_bar(0)),
        }
    }

    const fn usable(&self) -> bool {
        matches!(self.state, QuotaWindowCellState::Ready)
            && matches!(self.remaining_headroom, Some(remaining_headroom) if remaining_headroom > 0)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum QuotaWindowCellState {
    Ready,
    Empty,
    NeedsRefresh,
}

impl QuotaWindowCellState {
    const fn from_selector_window(window: &PersistedSelectorQuotaWindow) -> Self {
        if window.observed_unix_seconds() == 0 {
            Self::NeedsRefresh
        } else if window.remaining_headroom() == 0 {
            Self::Empty
        } else {
            match window.status() {
                SelectorQuotaWindowStatus::Eligible => Self::Ready,
                SelectorQuotaWindowStatus::Ineligible => Self::Empty,
                SelectorQuotaWindowStatus::Stale | SelectorQuotaWindowStatus::Unknown => {
                    Self::NeedsRefresh
                }
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct AccountRoutingStatus {
    usable: bool,
    bottleneck_headroom: Option<u32>,
    reason: String,
}

impl AccountRoutingStatus {
    fn from_windows(five_hour: &QuotaWindowCell, weekly: &QuotaWindowCell) -> Self {
        if !five_hour.usable() {
            return Self::blocked("5h", five_hour.state);
        }
        if !weekly.usable() {
            return Self::blocked("weekly", weekly.state);
        }
        let bottleneck_headroom = five_hour
            .remaining_headroom
            .zip(weekly.remaining_headroom)
            .map(|(five_hour, weekly)| five_hour.min(weekly));
        Self {
            usable: true,
            bottleneck_headroom,
            reason: format!("✓ usable bottleneck {}%", bottleneck_headroom.unwrap_or(0)),
        }
    }

    fn blocked(window_label: &str, state: QuotaWindowCellState) -> Self {
        let reason = match state {
            QuotaWindowCellState::Empty => format!("× {window_label} empty"),
            QuotaWindowCellState::NeedsRefresh => "↻ needs refresh".to_owned(),
            QuotaWindowCellState::Ready => format!("× {window_label} unavailable"),
        };
        Self {
            usable: false,
            bottleneck_headroom: None,
            reason,
        }
    }

    fn render(&self) -> String {
        self.reason.clone()
    }
}

fn mark_next_usable_account(rows: &mut [QuotaStatusRow]) {
    let next_index = rows
        .iter()
        .enumerate()
        .filter_map(|(index, row)| row.bottleneck_headroom.map(|headroom| (index, headroom)))
        .max_by_key(|(_index, headroom)| *headroom)
        .map(|(index, _headroom)| index);
    for (index, row) in rows.iter_mut().enumerate() {
        if row.bottleneck_headroom.is_some() {
            row.next_use = if Some(index) == next_index {
                "next".to_owned()
            } else {
                "backup".to_owned()
            };
        }
    }
}

fn quota_bar(value: u32) -> String {
    let filled = ((value.min(100) + 5) / 10).min(10);
    let mut output = String::with_capacity(30);
    for _index in 0..filled {
        output.push('█');
    }
    for _index in filled..10 {
        output.push('░');
    }
    output
}

fn format_percent(value: u32) -> String {
    format!("{}%", value.min(100))
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

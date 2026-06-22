//! Quota command glue for persisted router-owned quota state.

use std::io::Write;
use std::path::PathBuf;
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

const DEFAULT_ROUTE_BANDS: &[&str] = &["responses", "models"];

const ALL_ROUTE_BANDS: &[&str] = &[
    "responses",
    "models",
    "memories_trace_summarize",
    "responses_compact",
    "code_review",
];

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
    fn new(
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
    pub(crate) remaining_headroom: u32,
    pub(crate) reset_unix_seconds: Option<u64>,
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
        let client = reqwest::blocking::Client::builder()
            .user_agent("codex-router-quota-refresh")
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
    let state = SqliteStateStore::open(&router_root.join("state.sqlite"))?;
    let accounts = AccountStateRepository::list_accounts(&state)?;
    let mut refreshed_count = 0_u64;
    for account in accounts
        .iter()
        .filter(|account| account.status() == AccountStatus::Enabled)
        .filter(|account| account.active_credential_generation().is_some())
    {
        let resolved = credential_resolver.resolve_provider_credentials(account.account_id())?;
        for route_band in DEFAULT_ROUTE_BANDS {
            let response = quota_provider.fetch_quota(QuotaRefreshProviderRequest::new(
                account.account_id().clone(),
                account.label(),
                *route_band,
                base_url.clone(),
                resolved.access_token().clone(),
            ))?;
            let snapshot = PersistedQuotaSnapshot::new(
                account.account_id().clone(),
                QuotaSnapshotSource::OpenAiEndpoint,
            )
            .with_observed_unix_seconds(observed_unix_seconds)
            .with_route_band(*route_band, response.remaining_headroom)
            .with_stale_penalty(false);
            let snapshot = if let Some(reset_unix_seconds) = response.reset_unix_seconds {
                snapshot.with_reset_unix_seconds(reset_unix_seconds)
            } else {
                snapshot
            };
            QuotaSnapshotRepository::upsert_snapshot(&state, &snapshot)?;
            refreshed_count = refreshed_count.saturating_add(1);
        }
    }

    writeln!(stdout, "refreshed: {refreshed_count}").map_err(QuotaCommandError::Stdout)
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
    let window = window_pair
        .primary_window
        .as_ref()
        .or(window_pair.secondary_window.as_ref())
        .ok_or_else(|| QuotaCommandError::ProviderResponse {
            message: format!("missing provider quota windows for route band {route_band}"),
        })?;
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
    let reset_unix_seconds = window
        .reset_at
        .and_then(|reset_at| u64::try_from(reset_at).ok());

    Ok(QuotaRefreshProviderResponse {
        remaining_headroom,
        reset_unix_seconds,
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
    all_limits: bool,
    now_unix_seconds: u64,
) -> Result<Vec<QuotaStatusRow>, QuotaCommandError> {
    if all_limits {
        return quota_status_selector_rows(state, now_unix_seconds);
    }

    let route_bands = if all_limits {
        ALL_ROUTE_BANDS
    } else {
        DEFAULT_ROUTE_BANDS
    };
    let mut rows = Vec::new();
    for account in accounts {
        for route_band in route_bands {
            if let Some(snapshot) = QuotaSnapshotRepository::load_snapshot_for_route_band(
                state,
                account.account_id(),
                route_band,
            )? {
                rows.push(QuotaStatusRow::from_snapshot(
                    account,
                    &snapshot,
                    now_unix_seconds,
                ));
            }
        }
    }

    Ok(rows)
}

fn quota_status_selector_rows(
    state: &SqliteStateStore,
    now_unix_seconds: u64,
) -> Result<Vec<QuotaStatusRow>, QuotaCommandError> {
    let mut rows = Vec::new();
    for route_band in ALL_ROUTE_BANDS {
        let inputs = SelectorQuotaRepository::selector_inputs_for_route_band(state, route_band)?;
        for input in inputs {
            if let Some(effective_window) = input.windows().iter().find(|window| window.effective())
            {
                rows.push(QuotaStatusRow::from_selector_window(
                    &input,
                    effective_window,
                    "effective",
                    now_unix_seconds,
                ));
            }
            for window in input.windows() {
                rows.push(QuotaStatusRow::from_selector_window(
                    &input,
                    window,
                    quota_window_label(window.limit_window_seconds()),
                    now_unix_seconds,
                ));
            }
        }
    }

    Ok(rows)
}

fn write_quota_table(
    stdout: &mut impl Write,
    rows: &[QuotaStatusRow],
) -> Result<(), QuotaCommandError> {
    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    table.set_header([
        "account",
        "account_id",
        "status",
        "route_band",
        "window",
        "remaining",
        "reset",
        "pace",
        "runout",
        "stale",
        "source",
    ]);
    for row in rows {
        table.add_row([
            row.account_label.as_str(),
            row.account_id.as_str(),
            row.account_status.as_str(),
            row.route_band.as_str(),
            row.window.as_str(),
            row.remaining_headroom.as_str(),
            row.reset.as_str(),
            row.pace.as_str(),
            row.runout.as_str(),
            row.stale.as_str(),
            row.source.as_str(),
        ]);
    }

    writeln!(stdout, "{table}").map_err(QuotaCommandError::Stdout)
}

fn write_quota_plain(
    stdout: &mut impl Write,
    rows: &[QuotaStatusRow],
) -> Result<(), QuotaCommandError> {
    writeln!(
        stdout,
        "account\taccount_id\tstatus\troute_band\twindow\tremaining\treset\tpace\trunout\tstale\tsource"
    )
    .map_err(QuotaCommandError::Stdout)?;
    for row in rows {
        writeln!(
            stdout,
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            row.account_label,
            row.account_id,
            row.account_status,
            row.route_band,
            row.window,
            row.remaining_headroom,
            row.reset,
            row.pace,
            row.runout,
            row.stale,
            row.source,
        )
        .map_err(QuotaCommandError::Stdout)?;
    }

    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct QuotaStatusRow {
    account_label: String,
    account_id: String,
    account_status: String,
    route_band: String,
    window: String,
    remaining_headroom: String,
    reset: String,
    pace: String,
    runout: String,
    stale: String,
    source: String,
}

impl QuotaStatusRow {
    fn from_snapshot(
        account: &AccountRecord,
        snapshot: &PersistedQuotaSnapshot,
        now_unix_seconds: u64,
    ) -> Self {
        let reset = snapshot
            .reset_unix_seconds()
            .map_or_else(|| "-".to_owned(), |reset| reset.to_string());
        let math = snapshot.reset_unix_seconds().map_or_else(
            QuotaStatusMath::unknown,
            |reset_unix_seconds| {
                QuotaStatusMath::from_window(
                    snapshot.remaining_headroom(),
                    now_unix_seconds,
                    snapshot.observed_unix_seconds(),
                    reset_unix_seconds,
                )
            },
        );
        Self {
            account_label: account.label().to_owned(),
            account_id: account.account_id().as_str().to_owned(),
            account_status: account.status().as_str().to_owned(),
            route_band: snapshot.route_band().to_owned(),
            window: "effective".to_owned(),
            remaining_headroom: snapshot.remaining_headroom().to_string(),
            reset,
            pace: math.pace,
            runout: math.runout,
            stale: snapshot.stale_penalty().to_string(),
            source: snapshot.source().as_str().to_owned(),
        }
    }

    fn from_selector_window(
        input: &SelectorQuotaInput,
        window: &PersistedSelectorQuotaWindow,
        window_label: impl Into<String>,
        now_unix_seconds: u64,
    ) -> Self {
        let reset = window
            .reset_unix_seconds()
            .map_or_else(|| "-".to_owned(), |reset| reset.to_string());
        let math = window.reset_unix_seconds().map_or_else(
            QuotaStatusMath::unknown,
            |reset_unix_seconds| {
                QuotaStatusMath::from_window(
                    window.remaining_headroom(),
                    now_unix_seconds,
                    reset_unix_seconds.saturating_sub(window.limit_window_seconds()),
                    reset_unix_seconds,
                )
            },
        );
        Self {
            account_label: input.account_label().to_owned(),
            account_id: input.account_id().as_str().to_owned(),
            account_status: input.account_status().as_str().to_owned(),
            route_band: input.route_band().to_owned(),
            window: window_label.into(),
            remaining_headroom: window.remaining_headroom().to_string(),
            reset,
            pace: math.pace,
            runout: math.runout,
            stale: (window.status() != SelectorQuotaWindowStatus::Eligible).to_string(),
            source: "selector".to_owned(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct QuotaStatusMath {
    pace: String,
    runout: String,
}

impl QuotaStatusMath {
    fn unknown() -> Self {
        Self {
            pace: "-".to_owned(),
            runout: "-".to_owned(),
        }
    }

    fn from_window(
        remaining_headroom: u32,
        now_unix_seconds: u64,
        window_start_unix_seconds: u64,
        reset_unix_seconds: u64,
    ) -> Self {
        let window_seconds = reset_unix_seconds.saturating_sub(window_start_unix_seconds);
        if window_seconds == 0 {
            return Self::unknown();
        }

        let elapsed_seconds = now_unix_seconds
            .saturating_sub(window_start_unix_seconds)
            .min(window_seconds);
        let used_percent = 100_u32.saturating_sub(remaining_headroom.min(100));
        let expected_used_percent = (elapsed_seconds.saturating_mul(100) / window_seconds).min(100);
        let pace_points =
            i64::from(used_percent) - i64::try_from(expected_used_percent).unwrap_or(0);
        let pace = if pace_points > 0 {
            format!("+{pace_points}pp")
        } else {
            format!("{pace_points}pp")
        };
        let runout = projected_runout_duration(used_percent, remaining_headroom, elapsed_seconds);

        Self { pace, runout }
    }
}

fn projected_runout_duration(
    used_percent: u32,
    remaining_headroom: u32,
    elapsed_seconds: u64,
) -> String {
    if used_percent == 0 || remaining_headroom == 0 || elapsed_seconds == 0 {
        return "-".to_owned();
    }
    let seconds =
        u64::from(remaining_headroom).saturating_mul(elapsed_seconds) / u64::from(used_percent);
    format!("{seconds}s")
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
        self.router_root.clone().ok_or(CliError::MissingOption {
            option: "--router-root",
        })
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
        self.router_root.clone().ok_or(CliError::MissingOption {
            option: "--router-root",
        })
    }
}

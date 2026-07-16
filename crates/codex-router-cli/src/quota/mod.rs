//! Quota command glue for persisted router-owned quota state.

use std::collections::HashMap;
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
use codex_router_auth::live_quota::reset_credits_url;
use codex_router_auth::live_quota::usage_url;
use codex_router_auth::resolver::CredentialResolverError;
use codex_router_auth::resolver::ProviderCredentialResolver;
use codex_router_core::ids::AccountId;
use codex_router_core::redaction::SecretString;
use codex_router_core::redaction::safe_account_label;
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
use codex_router_state::quota_snapshot::QuotaRefreshStatusSource;
use codex_router_state::quota_snapshot::QuotaRefreshStatusView;
use codex_router_state::quota_snapshot::QuotaSnapshotSource;
use codex_router_state::quota_snapshot::SelectorQuotaInput;
use codex_router_state::quota_snapshot::SelectorQuotaWindowStatus;
use codex_router_state::selection_projection::project_route_band_selection_inputs_read_only;
use codex_router_state::sqlite::AsyncSqliteStateStore;
use codex_router_state::sqlite::StateStoreError;
use opentelemetry::KeyValue;
use opentelemetry::global;
use serde::Serialize;
use serde_json::Value;
use sha2::Digest;
use sha2::Sha256;
use thiserror::Error;

use crate::ArgumentParser;
use crate::CliError;
use crate::credential_runtime::CliCredentialResolver;
use crate::credential_runtime::CliCredentialResolverOpenError;
use crate::presentation::quota::QuotaSelectedAccountViewModel;
use crate::presentation::quota::QuotaStatusAccountViewModel;
use crate::presentation::quota::QuotaStatusViewModel;
use crate::presentation::quota::QuotaStatusViewModelLoader;
use crate::presentation::quota::ResetPaceMeterSegments;
use crate::presentation::quota::ResetPaceState;
use crate::presentation::quota::ResetPaceViewModel;
use crate::presentation::quota::SampleConfidence;
use crate::presentation::quota::SampleMetadata;
use crate::presentation::quota::run_quota_status_view;
use crate::presentation::quota::write_quota_status_view;
use crate::router_root_or_default;

const DEFAULT_ROUTE_BANDS: &[&str] = &["responses", "models"];
const USER_QUOTA_ROUTE_BAND: &str = "responses";
const DEFAULT_REFRESH_STALE_AFTER_GRACE_SECONDS: u64 = 300;
const QUOTA_STATUS_SAMPLE_FRESH_SECONDS: u64 = 900;
const QUOTA_STATUS_SHORT_BURN_LOOKBACK_SECONDS: u64 = 30 * 60;
const QUOTA_STATUS_WEEKLY_BURN_LOOKBACK_SECONDS: u64 = 3 * 60 * 60;
const QUOTA_STATUS_DISPLAY_MIN_RATE_SAMPLES: usize = 3;
const QUOTA_STATUS_DISPLAY_NORMAL_CONFIDENCE_SAMPLES: usize = 5;
const RESET_PACE_RUNOUT_LABEL_THRESHOLD_HUNDREDTHS: u32 = 200;
const ACTIVE_CLIENT_LEASE_MAX_AGE_SECONDS: u64 = 7_200;
const DEPLETED_QUOTA_LABEL: &str = "Exhausted";

/// Quota CLI command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum QuotaCommand {
    /// Prints quota command help.
    Help(&'static str),
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
    /// Interactively consumes one guarded live quota reset.
    Reset {
        /// Router-owned root used only for read-only lookup.
        router_root: PathBuf,
    },
}

impl QuotaCommand {
    pub(crate) fn parse(parser: &mut ArgumentParser) -> Result<Self, CliError> {
        let mut arguments = parser.remaining_arguments();
        let command = arguments.first().and_then(|argument| argument.to_str());
        match command {
            Some("--help" | "-h" | "help") => Ok(Self::Help(QUOTA_HELP_TEXT)),
            Some("refresh") => {
                arguments.remove(0);
                if matches!(
                    arguments.first().and_then(|argument| argument.to_str()),
                    Some("--help" | "-h" | "help")
                ) {
                    return Ok(Self::Help(QUOTA_REFRESH_HELP_TEXT));
                }
                let mut parser = ArgumentParser::new(arguments);
                let options = QuotaRefreshOptions::parse(&mut parser)?;
                Ok(Self::Refresh {
                    router_root: options.router_root()?,
                    base_url: options.base_url,
                })
            }
            Some("reset") => {
                arguments.remove(0);
                if matches!(
                    arguments.first().and_then(|argument| argument.to_str()),
                    Some("--help" | "-h" | "help")
                ) {
                    return Ok(Self::Help(QUOTA_RESET_HELP_TEXT));
                }
                let mut parser = ArgumentParser::new(arguments);
                parser.reject_remaining()?;
                Ok(Self::Reset {
                    router_root: router_root_or_default(None)?,
                })
            }
            Some("status") => {
                arguments.remove(0);
                let mut parser = ArgumentParser::new(arguments);
                let options = QuotaStatusOptions::parse(&mut parser)?;
                Ok(Self::Status {
                    router_root: options.router_root()?,
                    format: options.format,
                    all_limits: options.all_limits,
                    now_unix_seconds: options.now_unix_seconds,
                })
            }
            Some(unknown) if !unknown.starts_with('-') => Err(CliError::UnknownCommand {
                command: format!("quota {unknown}"),
            }),
            _ => {
                let mut parser = ArgumentParser::new(arguments);
                let options = QuotaStatusOptions::parse(&mut parser)?;
                Ok(Self::Status {
                    router_root: options.router_root()?,
                    format: options.format,
                    all_limits: options.all_limits,
                    now_unix_seconds: options.now_unix_seconds,
                })
            }
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
    /// Reset is dispatched only by the native async CLI entrypoint.
    #[error("quota reset requires the async CLI dispatcher")]
    AsyncResetDispatchRequired,
}

/// Runs a quota command.
pub fn run_quota_command(
    stdout: &mut impl Write,
    command: QuotaCommand,
    stdout_is_terminal: bool,
    stdout_terminal_width: Option<usize>,
) -> Result<(), QuotaCommandError> {
    match command {
        QuotaCommand::Help(help_text) => stdout
            .write_all(help_text.as_bytes())
            .map_err(QuotaCommandError::Stdout),
        QuotaCommand::Status {
            router_root,
            format,
            all_limits,
            now_unix_seconds,
        } => render_quota_status(
            stdout,
            router_root,
            format,
            stdout_is_terminal,
            stdout_terminal_width,
            all_limits,
            now_unix_seconds,
        ),
        QuotaCommand::Refresh {
            router_root,
            base_url,
        } => refresh_quota(stdout, router_root, base_url),
        QuotaCommand::Reset { .. } => Err(QuotaCommandError::AsyncResetDispatchRequired),
    }
}

/// Returns whether quota status should use the interactive terminal presentation.
pub(crate) fn should_run_interactive_quota(
    format: QuotaStatusFormat,
    stdin_is_terminal: bool,
    stdout_is_terminal: bool,
) -> bool {
    effective_human_quota_format(format, stdout_is_terminal) == QuotaStatusFormat::Table
        && stdin_is_terminal
        && stdout_is_terminal
}

pub(crate) async fn run_interactive_quota_status(
    command: QuotaCommand,
    stdin_is_terminal: bool,
    stdout_is_terminal: bool,
    stdout_terminal_width: Option<usize>,
) -> Result<(), QuotaCommandError> {
    let QuotaCommand::Status {
        router_root,
        format,
        all_limits,
        now_unix_seconds,
    } = command
    else {
        return Err(QuotaCommandError::AsyncResetDispatchRequired);
    };
    debug_assert!(should_run_interactive_quota(
        format,
        stdin_is_terminal,
        stdout_is_terminal
    ));
    render_interactive_quota_status(
        router_root,
        stdout_terminal_width,
        all_limits,
        now_unix_seconds,
    )
    .await
}

const QUOTA_HELP_TEXT: &str = "\
codex-router quota

commands:
  quota          Show persisted quota status and next account
  quota refresh  Refresh quota data now
  quota reset    Interactively use an eligible usage-limit reset
";

const QUOTA_REFRESH_HELP_TEXT: &str = "\
codex-router quota refresh

Refreshes persisted quota data from configured OAuth accounts.
";

const QUOTA_RESET_HELP_TEXT: &str = "\
codex-router quota reset

Interactively selects one account, checks live weekly usage, and offers the earliest-expiring
available usage-limit reset only when live weekly remaining is strictly below 1%.

shortcuts:
  up/down  select
  enter    check or confirm
  esc      cancel
  ctrl-c   cancel
  ctrl-r   cancel
";

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
    chatgpt_account_id: Option<String>,
}

impl QuotaRefreshProviderRequest {
    pub(crate) fn new(
        account_id: AccountId,
        account_label: impl Into<String>,
        route_band: impl Into<String>,
        base_url: impl Into<String>,
        access_token: SecretString,
        chatgpt_account_id: Option<&str>,
    ) -> Self {
        Self {
            account_id,
            account_label: account_label.into(),
            route_band: route_band.into(),
            base_url: base_url.into(),
            access_token,
            chatgpt_account_id: chatgpt_account_id.map(str::to_owned),
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

    /// Returns the ChatGPT account id header value, if known.
    #[must_use]
    pub(crate) fn chatgpt_account_id(&self) -> Option<&str> {
        self.chatgpt_account_id.as_deref()
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
        let mut usage_request = self
            .client
            .get(usage_url(request.base_url()))
            .bearer_auth(request.access_token().expose_secret());
        if let Some(chatgpt_account_id) = request.chatgpt_account_id() {
            usage_request = usage_request.header("ChatGPT-Account-ID", chatgpt_account_id);
        }
        let response =
            usage_request
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
        let usage = serde_json::from_value::<UsageResponse>(usage_value).map_err(|error| {
            QuotaCommandError::ProviderResponse {
                message: error.to_string(),
            }
        })?;
        let reset_credits_available = self.fetch_reset_credits_available(&request)?;
        quota_response_for_route_band(&usage, request.route_band()).map(|mut response| {
            response.reset_credits_available = reset_credits_available;
            response
        })
    }
}

impl HttpQuotaRefreshProvider {
    fn fetch_reset_credits_available(
        &self,
        request: &QuotaRefreshProviderRequest,
    ) -> Result<Option<u32>, QuotaCommandError> {
        let mut reset_request = self
            .client
            .get(reset_credits_url(request.base_url()))
            .bearer_auth(request.access_token().expose_secret());
        if let Some(chatgpt_account_id) = request.chatgpt_account_id() {
            reset_request = reset_request.header("ChatGPT-Account-ID", chatgpt_account_id);
        }
        let response =
            reset_request
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
        let value = serde_json::from_str::<Value>(&body).map_err(|error| {
            QuotaCommandError::ProviderResponse {
                message: error.to_string(),
            }
        })?;
        Ok(reset_credits_available_from_json(&value))
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
    let quota_history_runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(QuotaCommandError::Runtime)?;
    let quota_history_state =
        quota_history_runtime.block_on(AsyncSqliteStateStore::open(state_db))?;
    let accounts = quota_history_runtime.block_on(quota_history_state.list_accounts())?;
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
                    quota_history_runtime.block_on(
                        quota_history_state.record_refresh_failure_preserving_selector_windows(
                            account.account_id(),
                            route_band,
                            observed_unix_seconds,
                            QuotaRefreshErrorClass::AuthError,
                        ),
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
            let response = match quota_provider.fetch_quota(QuotaRefreshProviderRequest::new(
                account.account_id().clone(),
                account.label(),
                *route_band,
                base_url.clone(),
                resolved.access_token().clone(),
                resolved.chatgpt_account_id(),
            )) {
                Ok(response) => response,
                Err(error) => {
                    failed_count = failed_count.saturating_add(1);
                    let error_class = quota_refresh_error_class(&error);
                    quota_history_runtime.block_on(
                        quota_history_state.record_refresh_failure_preserving_selector_windows(
                            account.account_id(),
                            route_band,
                            observed_unix_seconds,
                            error_class,
                        ),
                    )?;
                    append_failure_quota_history_observations(
                        &quota_history_runtime,
                        &quota_history_state,
                        account,
                        route_band,
                        observed_unix_seconds,
                        error_class,
                    )?;
                    tracing::warn!(
                        account.hash = telemetry_hash(account.account_id().as_str()),
                        route_band,
                        error.class = error_class.as_str(),
                        "codex_router.quota_refresh_failed"
                    );
                    record_quota_refresh_metric(route_band, "failure", error_class.as_str());
                    let diagnostic_account = quota_refresh_diagnostic_account_label(account);
                    writeln!(
                        stdout,
                        "refresh failed: account={diagnostic_account} route_band={route_band} error={error}",
                    )
                    .map_err(QuotaCommandError::Stdout)?;
                    continue;
                }
            };
            let effective_window = match response.effective_window() {
                Some(effective_window) => effective_window,
                None => {
                    failed_count = failed_count.saturating_add(1);
                    quota_history_runtime.block_on(
                        quota_history_state.record_refresh_failure_preserving_selector_windows(
                            account.account_id(),
                            route_band,
                            observed_unix_seconds,
                            QuotaRefreshErrorClass::ParseError,
                        ),
                    )?;
                    append_failure_quota_history_observations(
                        &quota_history_runtime,
                        &quota_history_state,
                        account,
                        route_band,
                        observed_unix_seconds,
                        QuotaRefreshErrorClass::ParseError,
                    )?;
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
            quota_history_runtime.block_on(quota_history_state.upsert_quota_snapshot(&snapshot))?;
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
            quota_history_runtime.block_on(
                quota_history_state.record_refresh_success_and_replace_selector_windows(
                    account.account_id(),
                    route_band,
                    &selector_windows,
                    observed_unix_seconds,
                    stale_after_unix_seconds(observed_unix_seconds),
                ),
            )?;
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
    purge_old_quota_history(
        &quota_history_runtime,
        &quota_history_state,
        observed_unix_seconds,
    )?;

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
    quota_history_runtime.block_on(quota_history_state.close())?;

    refresh_result
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

fn quota_refresh_diagnostic_account_label(account: &AccountRecord) -> String {
    safe_account_label(account.label(), account.account_id())
        .as_str()
        .to_owned()
}

fn quota_response_for_route_band(
    usage: &UsageResponse,
    route_band: &str,
) -> Result<QuotaRefreshProviderResponse, QuotaCommandError> {
    if route_band == "code_review" {
        let window_pair = usage.code_review_rate_limit.as_ref().ok_or_else(|| {
            QuotaCommandError::ProviderResponse {
                message: format!("missing quota window for route band {route_band}"),
            }
        })?;
        return quota_response_from_window_pair(window_pair, route_band);
    }

    let window_pair =
        usage
            .rate_limit
            .as_ref()
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
        | QuotaCommandError::Stdout(_)
        | QuotaCommandError::AsyncResetDispatchRequired => QuotaRefreshErrorClass::ProviderError,
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
                    "resetcreditsavailable" | "availableresetcredits" | "availablecount"
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

mod options;
mod selection_projection;
mod status_command;
mod status_formatting;
mod status_json;
mod status_loader;
mod status_metrics;
mod status_model;
mod status_pace;
mod status_projection;

use options::*;
use selection_projection::*;
use status_command::*;
use status_formatting::*;
use status_json::*;
use status_loader::*;
use status_metrics::*;
use status_model::*;
use status_pace::*;
use status_projection::*;

#[cfg(test)]
#[cfg(test)]
mod tests;

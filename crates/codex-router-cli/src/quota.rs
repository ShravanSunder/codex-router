//! Quota command glue for persisted router-owned quota state.

use std::collections::HashMap;
use std::io::IsTerminal;
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
    }
}

const QUOTA_HELP_TEXT: &str = "\
codex-router quota

commands:
  quota          Show persisted quota status and next account
  quota refresh  Refresh quota data now
";

const QUOTA_REFRESH_HELP_TEXT: &str = "\
codex-router quota refresh

Refreshes persisted quota data from configured OAuth accounts.
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
            .bearer_auth(request.access_token().expose_secret())
            .header("OpenAI-Beta", "codex-1")
            .header("originator", "Codex Desktop");
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
            .bearer_auth(request.access_token().expose_secret())
            .header("OpenAI-Beta", "codex-1")
            .header("originator", "Codex Desktop");
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

fn render_quota_status(
    stdout: &mut impl Write,
    router_root: PathBuf,
    format: QuotaStatusFormat,
    stdout_is_terminal: bool,
    stdout_terminal_width: Option<usize>,
    all_limits: bool,
    now_unix_seconds: u64,
) -> Result<(), QuotaCommandError> {
    render_quota_status_once(
        stdout,
        &router_root,
        format,
        stdout_is_terminal,
        stdout_terminal_width,
        all_limits,
        now_unix_seconds,
    )
}

fn render_quota_status_once(
    stdout: &mut impl Write,
    router_root: &Path,
    format: QuotaStatusFormat,
    stdout_is_terminal: bool,
    stdout_terminal_width: Option<usize>,
    all_limits: bool,
    now_unix_seconds: u64,
) -> Result<(), QuotaCommandError> {
    let effective_format = effective_human_quota_format(format, stdout_is_terminal);
    let unicode_bars = effective_format != QuotaStatusFormat::Plain;
    let report = load_quota_status_report(router_root, all_limits, now_unix_seconds, unicode_bars)?;
    match effective_format {
        QuotaStatusFormat::Table if std::io::stdout().is_terminal() => {
            let rows = report.rows();
            let width = stdout_terminal_width.unwrap_or(100).max(40);
            let view_model = quota_status_view_model(&report, rows, width);
            let reload_view_model = quota_status_view_model_loader(
                router_root.to_path_buf(),
                all_limits,
                unicode_bars,
                width,
            );
            run_quota_status_view(view_model, Some(reload_view_model))
                .map_err(QuotaCommandError::Stdout)
        }
        QuotaStatusFormat::Table => write_quota_table_with_style(
            stdout,
            &report,
            stdout_terminal_width,
            QuotaTableStyle::TerminalColor,
        ),
        QuotaStatusFormat::Plain => write_quota_plain(stdout, &report),
        QuotaStatusFormat::Json => write_quota_json(stdout, &report),
    }
}

fn quota_status_view_model_loader(
    router_root: PathBuf,
    all_limits: bool,
    unicode_bars: bool,
    width: usize,
) -> QuotaStatusViewModelLoader {
    Arc::new(move || {
        let report = load_quota_status_report(
            &router_root,
            all_limits,
            current_unix_seconds(),
            unicode_bars,
        )
        .ok()?;
        Some(quota_status_view_model(&report, report.rows(), width))
    })
}

fn effective_human_quota_format(
    format: QuotaStatusFormat,
    stdout_is_terminal: bool,
) -> QuotaStatusFormat {
    match format {
        QuotaStatusFormat::Json | QuotaStatusFormat::Plain => format,
        QuotaStatusFormat::Table if stdout_is_terminal => QuotaStatusFormat::Table,
        QuotaStatusFormat::Table => QuotaStatusFormat::Plain,
    }
}

fn load_quota_status_report(
    router_root: &Path,
    all_limits: bool,
    now_unix_seconds: u64,
    unicode_bars: bool,
) -> Result<QuotaStatusReport, QuotaCommandError> {
    let state_db_path = router_root.join("state.sqlite");
    let quota_history_runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(QuotaCommandError::Runtime)?;
    let quota_history_state =
        quota_history_runtime.block_on(AsyncSqliteStateStore::open_read_only(&state_db_path))?;
    let accounts = quota_history_runtime.block_on(quota_history_state.list_accounts())?;
    let report = quota_status_report(
        &quota_history_runtime,
        &quota_history_state,
        &accounts,
        all_limits,
        now_unix_seconds,
        unicode_bars,
    )?;
    quota_history_runtime.block_on(quota_history_state.close())?;
    Ok(report)
}

fn quota_status_report(
    quota_history_runtime: &tokio::runtime::Runtime,
    quota_history_state: &AsyncSqliteStateStore,
    accounts: &[AccountRecord],
    _all_limits: bool,
    now_unix_seconds: u64,
    unicode_bars: bool,
) -> Result<QuotaStatusReport, QuotaCommandError> {
    let selector_inputs = quota_history_runtime.block_on(
        quota_history_state.selector_inputs_for_route_band(USER_QUOTA_ROUTE_BAND, now_unix_seconds),
    )?;
    let refresh_statuses = quota_history_runtime.block_on(
        quota_history_state.quota_refresh_statuses_for_route_band(USER_QUOTA_ROUTE_BAND),
    )?;
    let refresh_statuses = refresh_statuses
        .into_iter()
        .map(|status| (status.account_id().clone(), status))
        .collect::<HashMap<_, _>>();
    let active_client_counts_result = quota_history_runtime.block_on(
        quota_history_state.active_client_counts_for_route_band_read_only(
            USER_QUOTA_ROUTE_BAND,
            now_unix_seconds,
            ACTIVE_CLIENT_LEASE_MAX_AGE_SECONDS,
        ),
    );
    let active_client_mirror_source = if active_client_counts_result.is_ok() {
        "sqlx_mirror"
    } else {
        "unavailable"
    };
    let active_client_counts = active_client_counts_result.as_ref().ok().map(|counts| {
        counts
            .iter()
            .map(|count| {
                (
                    count.account_id().clone(),
                    ActiveClientMirrorLoad {
                        count: count.active_clients(),
                        pressure: count.active_pressure(),
                    },
                )
            })
            .collect::<HashMap<_, _>>()
    });
    let selection_projection_result =
        quota_history_runtime.block_on(project_route_band_selection_inputs_read_only(
            quota_history_state,
            USER_QUOTA_ROUTE_BAND,
            now_unix_seconds,
            ACTIVE_CLIENT_LEASE_MAX_AGE_SECONDS,
        ));
    let selection_projection_source = if selection_projection_result.is_ok() {
        SelectionProjectionSource::SqlxProjection
    } else {
        SelectionProjectionSource::DisplayWindowsFallback
    };
    let selection_projection = selection_projection_result.as_ref().ok();
    let mut status_inputs = Vec::new();
    let mut assessment_inputs = Vec::new();
    for account in accounts {
        let selector_input = selector_inputs
            .iter()
            .find(|input| input.account_id() == account.account_id());
        let snapshot = quota_history_runtime.block_on(
            quota_history_state
                .load_quota_snapshot_for_route_band(account.account_id(), USER_QUOTA_ROUTE_BAND),
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
        let projection_account = selection_projection.and_then(|projection| {
            projection
                .accounts()
                .iter()
                .find(|projected_account| projected_account.account_id() == account.account_id())
        });
        let projected_weekly_window = projection_account.and_then(|projected_account| {
            projected_account
                .windows()
                .iter()
                .find(|window| window.window_seconds() == V1_WEEKLY_WINDOW_SECONDS)
        });
        let weekly_pace =
            quota_pace_snapshot(&display_windows, projected_weekly_window, now_unix_seconds);
        let assessment_input = projection_account.cloned().unwrap_or_else(|| {
            burn_down_input_from_display_windows(account, &display_windows, now_unix_seconds)
        });
        let active_clients =
            active_client_counts
                .as_ref()
                .map_or(ActiveClientMirrorStatus::Unavailable, |counts| {
                    let load = counts
                        .get(account.account_id())
                        .copied()
                        .unwrap_or(ActiveClientMirrorLoad::EMPTY);
                    ActiveClientMirrorStatus::MirrorFresh {
                        count: load.count,
                        pressure: load.pressure,
                        max_age_seconds: ACTIVE_CLIENT_LEASE_MAX_AGE_SECONDS,
                    }
                });
        status_inputs.push(QuotaStatusAccountInput {
            account_label: account.label().to_owned(),
            account_status: account.status().as_str().to_owned(),
            account_id: account.account_id().clone(),
            reset_credits_available,
            updated: format_refresh_status(
                refresh_statuses.get(account.account_id()),
                now_unix_seconds,
            ),
            active_clients,
            windows: display_windows,
            weekly_pace,
        });
        assessment_inputs.push(assessment_input);
    }

    let assessment = assess_route_band(BurnDownRouteBandAssessmentInput::new(
        RouteBand::Responses,
        now_unix_seconds,
        assessment_inputs,
    ));
    let selected_pool = assessment.selected_pool();
    let authoritative_projection = selection_projection_source.is_authoritative();
    let preferred_next_account_id = authoritative_projection
        .then(|| assessment.preferred_next().cloned())
        .flatten();
    let preferred_next_hash = preferred_next_account_id
        .as_ref()
        .map(|account_id| telemetry_hash(account_id.as_str()))
        .unwrap_or_else(|| "none".to_owned());
    let preferred_selection_reason = preferred_next_account_id
        .as_ref()
        .and_then(|preferred_account_id| {
            assessment
                .accounts()
                .iter()
                .find(|account| account.account_id() == preferred_account_id)
        })
        .map_or("none", |account| {
            routing_reason_json(account.routing_reason())
        });
    tracing::info!(
        route_band = USER_QUOTA_ROUTE_BAND,
        selected_pool = selected_pool_json(selected_pool),
        selection.reason = preferred_selection_reason,
        preferred.account_hash = preferred_next_hash.as_str(),
        active_client.source = active_client_mirror_source,
        "codex_router.quota_status_selection"
    );
    let mut rows = status_inputs
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
    if !authoritative_projection {
        for row in &mut rows {
            row.normalize_degraded_projection_authority();
        }
    }
    emit_quota_status_metrics(USER_QUOTA_ROUTE_BAND, &rows);

    Ok(QuotaStatusReport {
        app_version: env!("CARGO_PKG_VERSION").to_owned(),
        route_band: USER_QUOTA_ROUTE_BAND.to_owned(),
        selected_pool,
        preferred_next_account_id,
        selection_projection_source,
        now_unix_seconds,
        rows,
    })
}

#[cfg(test)]
fn write_quota_table(
    stdout: &mut impl Write,
    report: &QuotaStatusReport,
    terminal_width: Option<usize>,
) -> Result<(), QuotaCommandError> {
    write_quota_table_with_style(stdout, report, terminal_width, QuotaTableStyle::PlainText)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum QuotaTableStyle {
    #[cfg(test)]
    PlainText,
    TerminalColor,
}

impl QuotaTableStyle {
    const fn ansi(self) -> bool {
        match self {
            #[cfg(test)]
            Self::PlainText => false,
            Self::TerminalColor => true,
        }
    }
}

fn write_quota_table_with_style(
    stdout: &mut impl Write,
    report: &QuotaStatusReport,
    terminal_width: Option<usize>,
    style: QuotaTableStyle,
) -> Result<(), QuotaCommandError> {
    let rows = report.rows();
    let width = terminal_width.unwrap_or(100).max(40);
    let view_model = quota_status_view_model(report, rows, width);
    write_quota_status_view(stdout, view_model, style.ansi()).map_err(QuotaCommandError::Stdout)
}

fn quota_status_view_model(
    report: &QuotaStatusReport,
    rows: &[QuotaStatusRow],
    width: usize,
) -> QuotaStatusViewModel {
    let selected_row = rows.iter().find(|row| row.preferred_next);
    QuotaStatusViewModel {
        width,
        route_line: quota_status_route_line(report, rows),
        why_line: String::new(),
        serving_clients: quota_status_serving_clients(rows),
        rows: rows
            .iter()
            .map(|row| QuotaStatusAccountViewModel {
                selected: row.preferred_next,
                account: row.account_label.clone(),
                status: quota_state_text(row).to_owned(),
                active_clients: active_clients_label(row),
                reset_credits: reset_credits_account_list_label(row.reset_credits_available_value),
                reason: reason_summary(row),
                weekly_window: quota_window_visual_summary(
                    &row.windows,
                    V1_WEEKLY_WINDOW_SECONDS,
                    "",
                    report.now_unix_seconds,
                )
                .trim()
                .to_owned(),
                burn_meter: quota_safe_pace_meter(row.weekly_pace, report.now_unix_seconds),
                sample_metadata: sample_metadata_from_display_window(
                    &row.windows,
                    V1_WEEKLY_WINDOW_SECONDS,
                    report.now_unix_seconds,
                ),
                reset_pace: reset_pace_view_model_from_snapshot(
                    row.weekly_pace,
                    report.now_unix_seconds,
                ),
                weekly_pace: quota_pace_summary(row.weekly_pace, report.now_unix_seconds),
                details: quota_selected_account_view_model(report, row),
            })
            .collect(),
        selected: selected_row.map(|row| quota_selected_account_view_model(report, row)),
    }
}

fn quota_status_serving_clients(rows: &[QuotaStatusRow]) -> Option<u32> {
    let total = rows
        .iter()
        .filter_map(|row| row.active_clients_value)
        .fold(0_u32, u32::saturating_add);
    (total > 0).then_some(total)
}

fn quota_status_route_line(report: &QuotaStatusReport, rows: &[QuotaStatusRow]) -> String {
    let Some(selected_row) = rows.iter().find(|row| row.preferred_next) else {
        return format!(
            "{} -> none    {}",
            report.route_band,
            selector_summary(rows)
        );
    };
    let mut parts = vec![
        format!("{} -> {}", report.route_band, selected_row.account_label),
        compact_routing_summary(selected_row),
    ];
    if let Some(total_rate) = quota_compact_total_burn_rate(selected_row.weekly_pace) {
        parts.push(total_rate);
    }
    if let Some(limiting_window) = selected_row.limiting_window {
        parts.push(format!(
            "{} {} left",
            quota_window_label(limiting_window.window_seconds()),
            format_percent(limiting_window.remaining_headroom())
        ));
    }
    parts.join("    ")
}

fn compact_routing_summary(row: &QuotaStatusRow) -> String {
    first_line(&row.routing)
        .strip_prefix("preferred by quota: ")
        .unwrap_or_else(|| first_line(&row.routing))
        .to_owned()
}

fn quota_selected_account_view_model(
    report: &QuotaStatusReport,
    row: &QuotaStatusRow,
) -> QuotaSelectedAccountViewModel {
    QuotaSelectedAccountViewModel {
        account: row.account_label.clone(),
        status: quota_state_text(row).to_owned(),
        reason: first_line(&row.routing).to_owned(),
        short_window: quota_window_visual_summary(
            &row.windows,
            V1_SHORT_WINDOW_SECONDS,
            "",
            report.now_unix_seconds,
        )
        .trim()
        .to_owned(),
        weekly_window: quota_window_visual_summary(
            &row.windows,
            V1_WEEKLY_WINDOW_SECONDS,
            "",
            report.now_unix_seconds,
        )
        .trim()
        .to_owned(),
        burn_meter: quota_safe_pace_meter(row.weekly_pace, report.now_unix_seconds),
        burn_pace: quota_pace_summary(row.weekly_pace, report.now_unix_seconds).replace("  ", " "),
        sample_metadata: sample_metadata_from_display_windows(
            &row.windows,
            report.now_unix_seconds,
        ),
        reset_pace: reset_pace_view_model_from_snapshot(row.weekly_pace, report.now_unix_seconds),
        short_reset_pace: short_reset_pace_view_model_from_snapshot(
            quota_display_pace_snapshot(
                &row.windows,
                V1_SHORT_WINDOW_SECONDS,
                report.now_unix_seconds,
            ),
            report.now_unix_seconds,
        ),
        total_rate: quota_total_rate_summary(row.weekly_pace),
        connection_rate: quota_connection_rate_summary(row.weekly_pace),
        active_clients: active_clients_label(row),
        guards: format!("5h {}% / weekly {}%", row.short_pressure, row.long_pressure),
        reset: row.reset_credits_available.clone(),
        note: first_line(&row.routing).to_owned(),
    }
}

fn write_quota_plain(
    stdout: &mut impl Write,
    report: &QuotaStatusReport,
) -> Result<(), QuotaCommandError> {
    let rows = report.rows();
    writeln!(stdout, "codex-router {}", report.app_version).map_err(QuotaCommandError::Stdout)?;
    writeln!(
        stdout,
        "account\tstatus\t5h\tweekly\treset pace\tsample\tupdated\tclients\tresets available\trouting\tnext use"
    )
    .map_err(QuotaCommandError::Stdout)?;
    for row in rows {
        let reset_pace =
            reset_pace_view_model_from_snapshot(row.weekly_pace, report.now_unix_seconds);
        let sample_metadata =
            sample_metadata_from_display_windows(&row.windows, report.now_unix_seconds);
        writeln!(
            stdout,
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            row.account_label,
            row.account_status,
            row.short_window.replace('\n', " "),
            row.weekly_window.replace('\n', " "),
            plain_reset_pace_summary(&reset_pace),
            plain_sample_metadata_summary(&sample_metadata),
            row.updated.replace('\n', " "),
            row.active_clients.replace('\n', " "),
            row.reset_credits_available,
            row.routing.replace('\n', " "),
            row.next_use,
        )
        .map_err(QuotaCommandError::Stdout)?;
    }

    write_selector_summary_plain(stdout, rows)
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

fn plain_reset_pace_summary(reset_pace: &ResetPaceViewModel) -> String {
    if reset_pace.state == ResetPaceState::Unavailable {
        return reset_pace.semantic_label.to_owned();
    }
    if let Some(impact_label) = &reset_pace.impact_label {
        return impact_label.clone();
    }
    format!(
        "{} {}",
        reset_pace.multiple_label, reset_pace.semantic_label
    )
}

fn plain_sample_metadata_summary(sample_metadata: &SampleMetadata) -> String {
    if sample_metadata.confidence == SampleConfidence::Unknown {
        return sample_metadata.semantic_label.to_owned();
    }
    format!(
        "{} {}",
        sample_metadata.semantic_label, sample_metadata.age_label
    )
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum QuotaHumanGroup {
    Preferred,
    Available,
    Held,
    BlockedOrStale,
}

fn quota_human_group(row: &QuotaStatusRow) -> QuotaHumanGroup {
    if row.preferred_next {
        return QuotaHumanGroup::Preferred;
    }
    if row.freshness != QuotaEvidenceFreshness::Fresh {
        return QuotaHumanGroup::BlockedOrStale;
    }
    match row.routing_reason {
        RoutingReason::AvailableSamePool => QuotaHumanGroup::Available,
        RoutingReason::HeldReserve
        | RoutingReason::HeldUnknown
        | RoutingReason::HeldShortWindowGuard => QuotaHumanGroup::Held,
        RoutingReason::UnknownFallbackAvailable => QuotaHumanGroup::BlockedOrStale,
        _ => match row.availability {
            AccountAvailability::Usable => QuotaHumanGroup::Available,
            AccountAvailability::Reserve => QuotaHumanGroup::Held,
            AccountAvailability::Retiring
            | AccountAvailability::Blocked
            | AccountAvailability::Unknown
            | AccountAvailability::Excluded => QuotaHumanGroup::BlockedOrStale,
        },
    }
}

fn quota_state_text(row: &QuotaStatusRow) -> &'static str {
    if row.preferred_next {
        return "preferred";
    }
    if row.freshness == QuotaEvidenceFreshness::Stale {
        return "stale";
    }
    if row.freshness == QuotaEvidenceFreshness::Unknown {
        return "unknown";
    }
    match quota_human_group(row) {
        QuotaHumanGroup::Preferred => "preferred",
        QuotaHumanGroup::Available => "available",
        QuotaHumanGroup::Held => "held",
        QuotaHumanGroup::BlockedOrStale => "blocked",
    }
}

fn quota_window_visual_summary(
    windows: &[DisplayQuotaWindow],
    window_seconds: u64,
    label: &'static str,
    now_unix_seconds: u64,
) -> String {
    let Some(window) = windows
        .iter()
        .find(|window| window.window_seconds == window_seconds)
    else {
        return format!("{label} {} no data", quota_bar(0, true));
    };
    let note = window_display_note(window, now_unix_seconds)
        .replace("resets in ", "reset ")
        .replace("resets ", "reset ");
    format!(
        "{label} {} {} left, {note}",
        quota_bar(window.remaining_headroom, true),
        format_percent(window.remaining_headroom)
    )
}

fn first_line(value: &str) -> &str {
    value.lines().next().unwrap_or(value).trim()
}

fn active_clients_label(row: &QuotaStatusRow) -> String {
    row.active_clients_value.map_or_else(
        || "unknown clients".to_owned(),
        |count| {
            if count == 1 {
                "1 client".to_owned()
            } else {
                format!("{count} clients")
            }
        },
    )
}

fn reset_credits_account_list_label(reset_credits_available: Option<u32>) -> String {
    reset_credits_available.map_or_else(
        || "resets unknown".to_owned(),
        |credits| {
            if credits == 1 {
                "1 reset".to_owned()
            } else {
                format!("{credits} resets")
            }
        },
    )
}

fn reason_summary(row: &QuotaStatusRow) -> String {
    first_line(&row.routing)
        .replace("preferred by quota: ", "")
        .replace("available by quota: ", "")
        .replace("held by quota: ", "")
        .replace("fallback by quota: ", "")
        .replace("blocked: ", "")
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
    app_version: String,
    route_band: String,
    selected_pool: SelectedPool,
    preferred_next_account_id: Option<AccountId>,
    selection_projection_source: SelectionProjectionSource,
    now_unix_seconds: u64,
    rows: Vec<QuotaStatusRow>,
}

impl QuotaStatusReport {
    fn rows(&self) -> &[QuotaStatusRow] {
        &self.rows
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SelectionProjectionSource {
    SqlxProjection,
    DisplayWindowsFallback,
}

impl SelectionProjectionSource {
    const fn as_json(self) -> &'static str {
        match self {
            Self::SqlxProjection => "sqlx_projection",
            Self::DisplayWindowsFallback => "display_windows_fallback",
        }
    }

    const fn route_result(self) -> &'static str {
        match self {
            Self::SqlxProjection => "ok",
            Self::DisplayWindowsFallback => "degraded",
        }
    }

    const fn is_authoritative(self) -> bool {
        matches!(self, Self::SqlxProjection)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct QuotaStatusAccountInput {
    account_label: String,
    account_status: String,
    account_id: AccountId,
    reset_credits_available: Option<u32>,
    updated: String,
    active_clients: ActiveClientMirrorStatus,
    windows: Vec<DisplayQuotaWindow>,
    weekly_pace: Option<QuotaPaceSnapshot>,
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
    updated: String,
    active_clients: String,
    active_clients_value: Option<u32>,
    active_clients_source: &'static str,
    reset_credits_available: String,
    reset_credits_available_value: Option<u32>,
    routing: String,
    next_use: String,
    weekly_pace: Option<QuotaPaceSnapshot>,
    windows: Vec<DisplayQuotaWindow>,
    availability: AccountAvailability,
    freshness: QuotaEvidenceFreshness,
    routing_exclusion: RoutingExclusion,
    quota_evidence_reason: QuotaEvidenceReason,
    routing_reason: RoutingReason,
    preferred_next: bool,
    short_pressure: u32,
    long_pressure: u32,
    short_salvage: u32,
    long_salvage: u32,
    limiting_window: Option<LimitingWindow>,
    weekly_survival_margin_basis_points: Option<i64>,
    weekly_projected_exhaustion_unix_seconds: Option<u64>,
    weekly_burn_rate_confidence: QuotaRunRateConfidence,
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
            updated: input.updated.clone(),
            active_clients: format_active_clients(input.active_clients),
            active_clients_value: input.active_clients.count(),
            active_clients_source: input.active_clients.source(),
            reset_credits_available: format_reset_credits(input.reset_credits_available),
            reset_credits_available_value: input.reset_credits_available,
            routing: format_routing_cell(assessment),
            next_use: format_next_use(assessment).to_owned(),
            weekly_pace: input.weekly_pace,
            windows: input.windows.clone(),
            availability: assessment.availability(),
            freshness: assessment.freshness(),
            routing_exclusion: assessment.routing_exclusion(),
            quota_evidence_reason: assessment.quota_evidence_reason(),
            routing_reason: assessment.routing_reason(),
            preferred_next: assessment.preferred_next(),
            short_pressure: assessment.short_pressure(),
            long_pressure: assessment.long_pressure(),
            short_salvage: assessment.short_salvage(),
            long_salvage: assessment.long_salvage(),
            limiting_window: assessment.limiting_window(),
            weekly_survival_margin_basis_points: assessment.weekly_survival_margin_basis_points(),
            weekly_projected_exhaustion_unix_seconds: assessment
                .weekly_projected_exhaustion_unix_seconds(),
            weekly_burn_rate_confidence: assessment.weekly_burn_rate_confidence(),
        }
    }

    fn normalize_degraded_projection_authority(&mut self) {
        self.preferred_next = false;
        if routing_reason_is_preferred(self.routing_reason) {
            self.routing_reason = RoutingReason::UnknownFallbackAvailable;
            self.routing = format_routing_reason(self.routing_reason).to_owned();
            self.next_use = format_next_use_from_routing_reason(self.routing_reason).to_owned();
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct QuotaPaceSnapshot {
    remaining_headroom: u32,
    reset_unix_seconds: Option<u64>,
    projected_exhaustion_unix_seconds: Option<u64>,
    projected_candidate_burn_basis_points_per_hour: Option<u32>,
    aggregate_burn_basis_points_per_hour: Option<u32>,
    per_connection_burn_basis_points_per_hour: Option<u32>,
    confidence: QuotaRunRateConfidence,
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
    for window in windows {
        let Some(reset_unix_seconds) = window.reset_unix_seconds else {
            continue;
        };
        let observed_from_unix_seconds = now_unix_seconds.saturating_sub(
            quota_status_display_burn_lookback_seconds(window.window_seconds),
        );
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
        window.run_rate_estimate = display_quota_run_rate_estimate(
            window.window_seconds,
            now_unix_seconds,
            reset_unix_seconds,
            &observations,
        );
    }

    Ok(())
}

fn display_quota_run_rate_estimate(
    window_seconds: u64,
    now_unix_seconds: u64,
    reset_unix_seconds: u64,
    observations: &[QuotaRunRateObservation],
) -> QuotaRunRateEstimate {
    let observed_from_unix_seconds =
        now_unix_seconds.saturating_sub(quota_status_display_burn_lookback_seconds(window_seconds));
    let recent_observations = observations
        .iter()
        .copied()
        .filter(|observation| observation.observed_unix_seconds() >= observed_from_unix_seconds)
        .collect::<Vec<_>>();
    if recent_observations.len() < QUOTA_STATUS_DISPLAY_MIN_RATE_SAMPLES {
        return QuotaRunRateEstimate::insufficient();
    }
    let estimate = display_quota_run_rate_estimator().estimate(
        now_unix_seconds,
        reset_unix_seconds,
        &recent_observations,
    );
    if recent_observations.len() < QUOTA_STATUS_DISPLAY_NORMAL_CONFIDENCE_SAMPLES
        && estimate.confidence() == QuotaRunRateConfidence::Normal
        && let (Some(rate), Some(headroom)) = (
            estimate.burn_rate_basis_points_per_hour(),
            estimate.latest_remaining_headroom_percent(),
        )
    {
        return QuotaRunRateEstimate::with_rate_basis_points_per_hour(
            QuotaRunRateConfidence::Low,
            rate,
            headroom,
        );
    }
    estimate
}

const fn quota_status_display_burn_lookback_seconds(window_seconds: u64) -> u64 {
    if window_seconds == V1_SHORT_WINDOW_SECONDS {
        QUOTA_STATUS_SHORT_BURN_LOOKBACK_SECONDS
    } else if window_seconds == V1_WEEKLY_WINDOW_SECONDS {
        QUOTA_STATUS_WEEKLY_BURN_LOOKBACK_SECONDS
    } else {
        QUOTA_STATUS_SAMPLE_FRESH_SECONDS
    }
}

fn display_quota_run_rate_estimator() -> QuotaRunRateEstimator {
    QuotaRunRateEstimator::new(QUOTA_STATUS_SAMPLE_FRESH_SECONDS)
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
    now_unix_seconds: u64,
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
            if matches!(
                window.run_rate_estimate.confidence(),
                QuotaRunRateConfidence::Low | QuotaRunRateConfidence::Normal
            ) && let Some(projected_exhaustion_unix_seconds) = window
                .run_rate_estimate
                .projected_exhaustion_unix_seconds(now_unix_seconds)
            {
                fact =
                    fact.with_projected_exhaustion_unix_seconds(projected_exhaustion_unix_seconds);
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ActiveClientMirrorStatus {
    MirrorFresh {
        count: u32,
        pressure: u32,
        max_age_seconds: u64,
    },
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ActiveClientMirrorLoad {
    count: u32,
    pressure: u32,
}

impl ActiveClientMirrorLoad {
    const EMPTY: Self = Self {
        count: 0,
        pressure: 0,
    };
}

impl ActiveClientMirrorStatus {
    const fn count(self) -> Option<u32> {
        match self {
            Self::MirrorFresh { count, .. } => Some(count),
            Self::Unavailable => None,
        }
    }

    const fn source(self) -> &'static str {
        match self {
            Self::MirrorFresh { .. } => "sqlx_mirror",
            Self::Unavailable => "unavailable",
        }
    }
}

fn format_active_clients(active_clients: ActiveClientMirrorStatus) -> String {
    match active_clients {
        ActiveClientMirrorStatus::MirrorFresh {
            count,
            pressure: _,
            max_age_seconds,
        } => {
            let count_text = if count == 1 {
                "1 client".to_owned()
            } else {
                format!("{count} clients")
            };
            format!(
                "{count_text}\nmirror <= {}",
                format_duration(max_age_seconds)
            )
        }
        ActiveClientMirrorStatus::Unavailable => "unknown\nmirror unavailable".to_owned(),
    }
}

fn format_refresh_status(
    refresh_status: Option<&QuotaRefreshStatusView>,
    now_unix_seconds: u64,
) -> String {
    let Some(refresh_status) = refresh_status else {
        return "never\nneeds refresh".to_owned();
    };
    match refresh_status.status_source() {
        QuotaRefreshStatusSource::LegacyMissingRefreshStatus => "legacy\nneeds refresh".to_owned(),
        QuotaRefreshStatusSource::Recorded => {
            let success = refresh_status.last_success_unix_seconds().map_or_else(
                || "no success".to_owned(),
                |last_success| {
                    format!(
                        "ok {}",
                        format_relative_time(last_success, now_unix_seconds)
                    )
                },
            );
            if let Some(error_class) = refresh_status.last_error_class() {
                let attempt = refresh_status.last_attempt_unix_seconds().map_or_else(
                    || "attempt unknown".to_owned(),
                    |last_attempt| {
                        format!(
                            "failed {}",
                            format_relative_time(last_attempt, now_unix_seconds)
                        )
                    },
                );
                format!(
                    "{success}\n{attempt}: {}",
                    quota_refresh_error_class_label(error_class)
                )
            } else {
                success
            }
        }
    }
}

const fn quota_refresh_error_class_label(error_class: QuotaRefreshErrorClass) -> &'static str {
    match error_class {
        QuotaRefreshErrorClass::AuthError => "auth",
        QuotaRefreshErrorClass::NetworkError => "network",
        QuotaRefreshErrorClass::ProviderError => "provider",
        QuotaRefreshErrorClass::ParseError => "parse",
        QuotaRefreshErrorClass::RateLimited => "rate limited",
    }
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

    let quota_guard = format!(
        "quota guard 5h {}% / weekly {}%",
        assessment.short_pressure(),
        assessment.long_pressure()
    );
    if assessment.routing_weight().is_some() {
        quota_guard
    } else {
        format!("not selectable\n{quota_guard}")
    }
}

fn quota_pace_snapshot(
    windows: &[DisplayQuotaWindow],
    projected_weekly_window: Option<&QuotaWindowFact>,
    now_unix_seconds: u64,
) -> Option<QuotaPaceSnapshot> {
    quota_pace_snapshot_for_window(
        windows,
        V1_WEEKLY_WINDOW_SECONDS,
        projected_weekly_window,
        now_unix_seconds,
    )
}

fn quota_display_pace_snapshot(
    windows: &[DisplayQuotaWindow],
    window_seconds: u64,
    now_unix_seconds: u64,
) -> Option<QuotaPaceSnapshot> {
    quota_pace_snapshot_for_window(windows, window_seconds, None, now_unix_seconds)
}

fn quota_pace_snapshot_for_window(
    windows: &[DisplayQuotaWindow],
    window_seconds: u64,
    projected_window: Option<&QuotaWindowFact>,
    now_unix_seconds: u64,
) -> Option<QuotaPaceSnapshot> {
    let display_window = windows
        .iter()
        .find(|window| window.window_seconds == window_seconds)?;
    let projected_aggregate_rate =
        projected_window.and_then(QuotaWindowFact::aggregate_burn_basis_points_per_hour);
    let display_aggregate_rate = display_window
        .run_rate_estimate
        .burn_rate_basis_points_per_hour();
    let aggregate_rate = projected_aggregate_rate.or(display_aggregate_rate);
    let projected_candidate_rate =
        projected_window.and_then(QuotaWindowFact::projected_candidate_burn_basis_points_per_hour);
    let candidate_rate = projected_candidate_rate.or(aggregate_rate);
    let projected_exhaustion_from_projection =
        projected_window.and_then(QuotaWindowFact::projected_exhaustion_unix_seconds);
    let display_exhaustion = display_window
        .run_rate_estimate
        .projected_exhaustion_unix_seconds(now_unix_seconds);
    let projected_exhaustion = projected_exhaustion_from_projection.or(display_exhaustion);
    let projection_has_burn_estimate = projected_aggregate_rate.is_some()
        || projected_candidate_rate.is_some()
        || projected_exhaustion_from_projection.is_some();
    let confidence = if projection_has_burn_estimate {
        projected_window.map_or(
            display_window.run_rate_estimate.confidence(),
            QuotaWindowFact::burn_rate_confidence,
        )
    } else {
        display_window.run_rate_estimate.confidence()
    };
    Some(QuotaPaceSnapshot {
        remaining_headroom: display_window.remaining_headroom,
        reset_unix_seconds: display_window.reset_unix_seconds,
        projected_exhaustion_unix_seconds: projected_exhaustion,
        projected_candidate_burn_basis_points_per_hour: candidate_rate,
        aggregate_burn_basis_points_per_hour: aggregate_rate,
        per_connection_burn_basis_points_per_hour: projected_window
            .and_then(QuotaWindowFact::per_connection_burn_basis_points_per_hour),
        confidence,
    })
}

fn quota_pace_summary(snapshot: Option<QuotaPaceSnapshot>, now_unix_seconds: u64) -> String {
    let Some(snapshot) = snapshot else {
        return "burn unavailable".to_owned();
    };
    let direction = quota_pace_direction(snapshot, now_unix_seconds);
    let reset_pace = reset_pace_view_model_from_snapshot(Some(snapshot), now_unix_seconds);
    if reset_pace.state == ResetPaceState::Unavailable {
        return format!("{direction}  burn unavailable");
    }
    format!(
        "{direction}  {} {}",
        reset_pace.multiple_label, reset_pace.semantic_label
    )
}

fn quota_total_rate_summary(snapshot: Option<QuotaPaceSnapshot>) -> String {
    let Some(snapshot) = snapshot else {
        return "rate unknown".to_owned();
    };
    let total_rate = snapshot
        .projected_candidate_burn_basis_points_per_hour
        .or(snapshot.aggregate_burn_basis_points_per_hour)
        .map_or_else(
            || "unknown".to_owned(),
            format_burn_rate_basis_points_per_hour,
        );
    format!(
        "{total_rate} total ({})",
        run_rate_confidence_label(snapshot.confidence)
    )
}

fn quota_compact_total_burn_rate(snapshot: Option<QuotaPaceSnapshot>) -> Option<String> {
    let snapshot = snapshot?;
    snapshot
        .projected_candidate_burn_basis_points_per_hour
        .or(snapshot.aggregate_burn_basis_points_per_hour)
        .map(format_burn_rate_basis_points_per_hour)
        .map(|rate| format!("burn {rate}"))
}

fn quota_connection_rate_summary(snapshot: Option<QuotaPaceSnapshot>) -> String {
    let Some(snapshot) = snapshot else {
        return "connection rate unknown".to_owned();
    };
    let Some(per_connection_rate) = snapshot.per_connection_burn_basis_points_per_hour else {
        if snapshot.aggregate_burn_basis_points_per_hour.is_some()
            || snapshot
                .projected_candidate_burn_basis_points_per_hour
                .is_some()
        {
            return format!(
                "not attributed ({})",
                run_rate_confidence_label(snapshot.confidence)
            );
        }
        return format!(
            "unknown ({})",
            run_rate_confidence_label(snapshot.confidence)
        );
    };
    format!(
        "{}/conn ({})",
        format_burn_rate_basis_points_per_hour(per_connection_rate),
        run_rate_confidence_label(snapshot.confidence)
    )
}

fn quota_pace_direction(snapshot: QuotaPaceSnapshot, now_unix_seconds: u64) -> String {
    match (
        snapshot.projected_exhaustion_unix_seconds,
        snapshot.reset_unix_seconds,
    ) {
        (Some(projected_exhaustion), Some(reset)) if projected_exhaustion < reset => {
            format!(
                "behind {}",
                format_duration(reset.saturating_sub(projected_exhaustion))
            )
        }
        (Some(projected_exhaustion), Some(reset)) => {
            format!(
                "ahead {}",
                format_duration(projected_exhaustion.saturating_sub(reset))
            )
        }
        (None, Some(reset)) => format!(
            "ahead to reset ({})",
            format_relative_time(reset, now_unix_seconds)
        ),
        _ => "pace unknown".to_owned(),
    }
}

fn quota_safe_pace_meter(snapshot: Option<QuotaPaceSnapshot>, now_unix_seconds: u64) -> String {
    let reset_pace = reset_pace_view_model_from_snapshot(snapshot, now_unix_seconds);
    reset_pace_meter_text(&reset_pace)
}

fn quota_pace_load(snapshot: QuotaPaceSnapshot, now_unix_seconds: u64) -> Option<u32> {
    let reset_unix_seconds = snapshot.reset_unix_seconds?;
    if snapshot.remaining_headroom == 0 {
        return Some(999);
    }
    let time_left_seconds = reset_unix_seconds.saturating_sub(now_unix_seconds);
    if time_left_seconds == 0 {
        return None;
    }
    let candidate_rate = u128::from(snapshot.projected_candidate_burn_basis_points_per_hour?);
    let safe_rate = u128::from(snapshot.remaining_headroom)
        .saturating_mul(100)
        .saturating_mul(3_600)
        .checked_div(u128::from(time_left_seconds))?;
    if safe_rate == 0 {
        return None;
    }
    Some(((candidate_rate.saturating_mul(100)) / safe_rate).min(999) as u32)
}

fn sample_metadata_from_display_windows(
    windows: &[DisplayQuotaWindow],
    now_unix_seconds: u64,
) -> SampleMetadata {
    let observed_unix_seconds = windows
        .iter()
        .filter(|window| !matches!(window.status, QuotaWindowStatus::Unknown))
        .map(|window| window.observed_unix_seconds)
        .collect::<Vec<_>>();
    sample_metadata_from_observed_windows(&observed_unix_seconds, now_unix_seconds)
}

fn sample_metadata_from_display_window(
    windows: &[DisplayQuotaWindow],
    window_seconds: u64,
    now_unix_seconds: u64,
) -> SampleMetadata {
    let observed_unix_seconds = windows
        .iter()
        .filter(|window| {
            window.window_seconds == window_seconds
                && !matches!(window.status, QuotaWindowStatus::Unknown)
        })
        .map(|window| window.observed_unix_seconds)
        .collect::<Vec<_>>();
    sample_metadata_from_observed_windows(&observed_unix_seconds, now_unix_seconds)
}

fn sample_metadata_from_observed_windows(
    observed_unix_seconds: &[u64],
    now_unix_seconds: u64,
) -> SampleMetadata {
    let Some(oldest_observed_unix_seconds) = observed_unix_seconds.iter().min().copied() else {
        return SampleMetadata::default();
    };
    let age_seconds = now_unix_seconds.saturating_sub(oldest_observed_unix_seconds);
    let confidence = if age_seconds <= QUOTA_STATUS_SAMPLE_FRESH_SECONDS {
        SampleConfidence::Fresh
    } else {
        SampleConfidence::Stale
    };
    let semantic_label = match confidence {
        SampleConfidence::Fresh => "sample fresh",
        SampleConfidence::Stale => "sample stale",
        SampleConfidence::Unknown => "sample unknown",
    };
    SampleMetadata {
        confidence,
        age_label: format_duration(age_seconds),
        age_seconds: Some(age_seconds),
        semantic_label,
    }
}

fn reset_pace_view_model_from_snapshot(
    snapshot: Option<QuotaPaceSnapshot>,
    now_unix_seconds: u64,
) -> ResetPaceViewModel {
    let Some(snapshot) = snapshot else {
        return reset_pace_view_model_from_multiple_basis_points(None);
    };
    let multiple_hundredths = quota_pace_load(snapshot, now_unix_seconds);
    let impact_label = reset_pace_impact_label(snapshot, multiple_hundredths, now_unix_seconds);
    let mut view_model = reset_pace_view_model_from_multiple_basis_points(multiple_hundredths);
    if let Some(multiple_hundredths) = multiple_hundredths {
        let (left_filled, right_filled) =
            reset_pace_meter_fill_for_snapshot(snapshot, multiple_hundredths, now_unix_seconds);
        view_model.meter_left_segments = ResetPaceMeterSegments {
            filled: left_filled,
            empty: 7_usize.saturating_sub(left_filled),
        };
        view_model.meter_right_segments = ResetPaceMeterSegments {
            filled: right_filled,
            empty: 7_usize.saturating_sub(right_filled),
        };
    }
    view_model.impact_label = impact_label;
    view_model
}

fn short_reset_pace_view_model_from_snapshot(
    snapshot: Option<QuotaPaceSnapshot>,
    now_unix_seconds: u64,
) -> ResetPaceViewModel {
    let mut view_model = reset_pace_view_model_from_snapshot(snapshot, now_unix_seconds);
    if view_model.state == ResetPaceState::Unavailable
        && snapshot
            .is_some_and(|snapshot| snapshot.confidence == QuotaRunRateConfidence::Insufficient)
    {
        view_model.semantic_label = "collecting data";
        view_model.multiple_label = "collecting data".to_owned();
        view_model.unavailable_reason = Some("collecting quota samples".to_owned());
    }
    view_model
}

fn reset_pace_impact_label(
    snapshot: QuotaPaceSnapshot,
    multiple_hundredths: Option<u32>,
    now_unix_seconds: u64,
) -> Option<String> {
    if snapshot.remaining_headroom == 0 && snapshot.reset_unix_seconds.is_some() {
        return Some(DEPLETED_QUOTA_LABEL.to_owned());
    }
    if multiple_hundredths? <= RESET_PACE_RUNOUT_LABEL_THRESHOLD_HUNDREDTHS {
        return None;
    }
    let projected_exhaustion_unix_seconds = snapshot.projected_exhaustion_unix_seconds?;
    if projected_exhaustion_unix_seconds >= snapshot.reset_unix_seconds? {
        return None;
    }
    if projected_exhaustion_unix_seconds <= now_unix_seconds {
        return Some("runs out now".to_owned());
    }

    Some(format!(
        "runs out {}",
        format_duration(projected_exhaustion_unix_seconds.saturating_sub(now_unix_seconds))
    ))
}

fn reset_pace_view_model_from_multiple_basis_points(
    multiple_hundredths: Option<u32>,
) -> ResetPaceViewModel {
    let Some(multiple_hundredths) = multiple_hundredths else {
        return ResetPaceViewModel::default();
    };
    let state = match multiple_hundredths {
        0..=79 => ResetPaceState::UnderBurning,
        80..=120 => ResetPaceState::Healthy,
        _ => ResetPaceState::OverBurning,
    };
    let semantic_label = match state {
        ResetPaceState::UnderBurning => "under",
        ResetPaceState::Healthy => "healthy",
        ResetPaceState::OverBurning => "over",
        ResetPaceState::Unavailable => "burn unavailable",
    };
    let (left_filled, right_filled) = reset_pace_meter_fill(multiple_hundredths);
    ResetPaceViewModel {
        state,
        multiple_label: format_reset_pace_multiple_label(multiple_hundredths),
        impact_label: None,
        semantic_label,
        meter_left_segments: ResetPaceMeterSegments {
            filled: left_filled,
            empty: 7_usize.saturating_sub(left_filled),
        },
        meter_right_segments: ResetPaceMeterSegments {
            filled: right_filled,
            empty: 7_usize.saturating_sub(right_filled),
        },
        center_marker: '│',
        unavailable_reason: None,
    }
}

fn reset_pace_meter_text(reset_pace: &ResetPaceViewModel) -> String {
    reset_pace_meter_slots(
        reset_pace.meter_left_segments.filled,
        reset_pace.center_marker,
        reset_pace.meter_right_segments.filled,
    )
}

fn reset_pace_meter_slots(left_filled: usize, center_marker: char, right_filled: usize) -> String {
    const RESET_PACE_METER_SIDE_WIDTH: usize = 7;
    const RESET_PACE_METER_EMPTY: char = '□';
    const RESET_PACE_METER_FILLED: char = '■';
    let mut left_slots = [RESET_PACE_METER_EMPTY; RESET_PACE_METER_SIDE_WIDTH];
    let mut right_slots = [RESET_PACE_METER_EMPTY; RESET_PACE_METER_SIDE_WIDTH];
    for slot in left_slots
        .iter_mut()
        .rev()
        .take(left_filled.min(RESET_PACE_METER_SIDE_WIDTH))
    {
        *slot = RESET_PACE_METER_FILLED;
    }
    for slot in right_slots
        .iter_mut()
        .take(right_filled.min(RESET_PACE_METER_SIDE_WIDTH))
    {
        *slot = RESET_PACE_METER_FILLED;
    }

    left_slots
        .into_iter()
        .chain(std::iter::once(center_marker))
        .chain(right_slots)
        .collect()
}

fn reset_pace_meter_fill(multiple_hundredths: u32) -> (usize, usize) {
    const HEALTHY_LOWER_BOUND_HUNDREDTHS: u32 = 80;
    const HEALTHY_UPPER_BOUND_HUNDREDTHS: u32 = 120;
    const METER_SIDE_WIDTH: u32 = 7;

    if multiple_hundredths < HEALTHY_LOWER_BOUND_HUNDREDTHS {
        let under_distance = HEALTHY_LOWER_BOUND_HUNDREDTHS.saturating_sub(multiple_hundredths);
        (
            under_distance
                .saturating_mul(METER_SIDE_WIDTH)
                .div_ceil(HEALTHY_LOWER_BOUND_HUNDREDTHS) as usize,
            0,
        )
    } else if multiple_hundredths > HEALTHY_UPPER_BOUND_HUNDREDTHS {
        let over_distance = multiple_hundredths
            .saturating_sub(HEALTHY_UPPER_BOUND_HUNDREDTHS)
            .min(HEALTHY_LOWER_BOUND_HUNDREDTHS);
        (
            0,
            over_distance
                .saturating_mul(METER_SIDE_WIDTH)
                .div_ceil(HEALTHY_LOWER_BOUND_HUNDREDTHS) as usize,
        )
    } else {
        (0, 0)
    }
}

fn reset_pace_meter_fill_for_snapshot(
    snapshot: QuotaPaceSnapshot,
    multiple_hundredths: u32,
    now_unix_seconds: u64,
) -> (usize, usize) {
    let Some(reset_unix_seconds) = snapshot.reset_unix_seconds else {
        return reset_pace_meter_fill(multiple_hundredths);
    };
    let Some(projected_exhaustion_unix_seconds) = snapshot.projected_exhaustion_unix_seconds else {
        return reset_pace_meter_fill(multiple_hundredths);
    };
    if projected_exhaustion_unix_seconds >= reset_unix_seconds {
        return reset_pace_meter_fill(multiple_hundredths);
    }
    let time_until_reset_seconds = reset_unix_seconds.saturating_sub(now_unix_seconds);
    if time_until_reset_seconds == 0 {
        return reset_pace_meter_fill(multiple_hundredths);
    }
    let early_by_seconds =
        reset_unix_seconds.saturating_sub(projected_exhaustion_unix_seconds.max(now_unix_seconds));
    (
        0,
        early_by_seconds
            .saturating_mul(7)
            .div_ceil(time_until_reset_seconds) as usize,
    )
}

fn format_reset_pace_multiple_label(multiple_hundredths: u32) -> String {
    format!(
        "{}.{:02}x reset pace",
        multiple_hundredths / 100,
        multiple_hundredths % 100
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
            format!("{label} ineligible")
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
            let burn_rate = format_burn_rate_basis_points_per_hour(
                estimate.burn_rate_basis_points_per_hour().unwrap_or(0),
            );
            match estimate.projected_exhaustion_unix_seconds(now_unix_seconds) {
                Some(runout) => {
                    format!(
                        "{confidence} burn {burn_rate}; runout {}",
                        format_relative_time(runout, now_unix_seconds)
                    )
                }
                None => format!("{confidence} burn {burn_rate}; no runout"),
            }
        }
    }
}

fn format_burn_rate_basis_points_per_hour(burn_rate_basis_points_per_hour: u32) -> String {
    let whole_percent = burn_rate_basis_points_per_hour / 100;
    let fractional_basis_points = burn_rate_basis_points_per_hour % 100;
    if fractional_basis_points == 0 {
        return format!("{whole_percent}%/h");
    }
    if fractional_basis_points.is_multiple_of(10) {
        return format!("{}.{}%/h", whole_percent, fractional_basis_points / 10);
    }

    format!("{whole_percent}.{fractional_basis_points:02}%/h")
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
    let first_line = format_routing_reason(assessment.routing_reason());
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

const fn routing_reason_is_preferred(reason: RoutingReason) -> bool {
    matches!(
        reason,
        RoutingReason::PreferredNearResetDrainable
            | RoutingReason::PreferredNearResetControlledDrain
            | RoutingReason::PreferredWeeklyHealthier
            | RoutingReason::PreferredWeeklyResetSoon
            | RoutingReason::PreferredShortResetSoon
            | RoutingReason::PreferredProjectedBurn
            | RoutingReason::PreferredSafestQuota
            | RoutingReason::PreferredLastResortShortWindowGuard
    )
}

fn format_routing_reason(reason: RoutingReason) -> &'static str {
    match reason {
        RoutingReason::PreferredNearResetDrainable => "preferred by quota: near-reset drainable",
        RoutingReason::PreferredNearResetControlledDrain => {
            "preferred by quota: near-reset controlled drain"
        }
        RoutingReason::PreferredWeeklyHealthier => "preferred by quota: weekly healthier",
        RoutingReason::PreferredWeeklyResetSoon => "preferred by quota: weekly reset soon",
        RoutingReason::PreferredShortResetSoon => "preferred by quota: 5h reset soon",
        RoutingReason::PreferredProjectedBurn => "preferred by quota: projected burn",
        RoutingReason::PreferredSafestQuota => "preferred by quota: safest quota",
        RoutingReason::PreferredLastResortShortWindowGuard => {
            "preferred by quota: last-resort 5h guard"
        }
        RoutingReason::AvailableSamePool => "available by quota: same pool",
        RoutingReason::HeldReserve => "held by quota: reserve",
        RoutingReason::HeldUnknown => "held by quota: needs refresh",
        RoutingReason::HeldShortWindowGuard => "held by quota: 5h guard",
        RoutingReason::UnknownFallbackPreferred => "fallback by quota: needs refresh",
        RoutingReason::UnknownFallbackAvailable => "fallback by quota: same unknown pool",
        RoutingReason::RetiringNearZero => "retiring: near zero quota",
        RoutingReason::ExcludedDisabled => "excluded: account disabled",
        RoutingReason::ExcludedMissingCredential => "excluded: missing credential",
        RoutingReason::BlockedWindowExhausted => "blocked: quota empty",
        RoutingReason::BlockedWindowIneligible => "blocked: quota ineligible",
    }
}

fn format_next_use(assessment: &BurnDownAccountAssessment) -> &'static str {
    format_next_use_from_routing_reason(assessment.routing_reason())
}

fn format_next_use_from_routing_reason(reason: RoutingReason) -> &'static str {
    match reason {
        RoutingReason::PreferredWeeklyHealthier
        | RoutingReason::PreferredNearResetDrainable
        | RoutingReason::PreferredNearResetControlledDrain
        | RoutingReason::PreferredWeeklyResetSoon
        | RoutingReason::PreferredShortResetSoon
        | RoutingReason::PreferredProjectedBurn
        | RoutingReason::PreferredSafestQuota
        | RoutingReason::PreferredLastResortShortWindowGuard => "preferred by quota",
        RoutingReason::AvailableSamePool => "available by quota",
        RoutingReason::HeldReserve
        | RoutingReason::HeldUnknown
        | RoutingReason::HeldShortWindowGuard => "held by quota",
        RoutingReason::UnknownFallbackPreferred | RoutingReason::UnknownFallbackAvailable => {
            "fallback by quota"
        }
        RoutingReason::RetiringNearZero => "retiring",
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
    app_version: String,
    route_band: String,
    selection_projection_source: &'static str,
    selected_pool: &'static str,
    selected_pool_reason: &'static str,
    preferred_next_account_hash: Option<String>,
    accounts: Vec<JsonQuotaStatusAccount>,
}

impl JsonQuotaStatusReport {
    fn from_report(report: &QuotaStatusReport) -> Self {
        Self {
            route_result: report.selection_projection_source.route_result(),
            app_version: report.app_version.clone(),
            route_band: report.route_band.clone(),
            selection_projection_source: report.selection_projection_source.as_json(),
            selected_pool: selected_pool_json(report.selected_pool),
            selected_pool_reason: selected_pool_reason_json(report.selected_pool),
            preferred_next_account_hash: report
                .preferred_next_account_id
                .as_ref()
                .map(|account_id| telemetry_hash(account_id.as_str())),
            accounts: report
                .rows
                .iter()
                .map(|row| JsonQuotaStatusAccount::from_row(row, report.now_unix_seconds))
                .collect(),
        }
    }
}

#[derive(Serialize)]
struct JsonQuotaStatusAccount {
    account_hash: String,
    safe_account_label: String,
    availability: &'static str,
    freshness: &'static str,
    routing_exclusion: &'static str,
    next_use: String,
    limiting_window: &'static str,
    quota_evidence_reason: &'static str,
    short_quota_guard: Option<u32>,
    weekly_quota_guard: Option<u32>,
    weekly_survival_margin_basis_points: Option<i64>,
    weekly_projected_exhaustion_unix_seconds: Option<u64>,
    short_guard_result: &'static str,
    current_active_sessions: Option<u32>,
    active_session_source: &'static str,
    weekly_burn_rate_confidence: &'static str,
    hard_block_reason: Option<&'static str>,
    short_salvage: Option<u32>,
    long_salvage: Option<u32>,
    salvage_tie_key: Option<JsonSalvageTieKey>,
    routing_reason: &'static str,
    preferred_next: bool,
    reset_credits_available: Option<u32>,
    active_clients: Option<u32>,
    active_clients_source: &'static str,
    updated: String,
    window_slots: JsonWindowSlots,
    windows: Vec<JsonQuotaWindow>,
}

impl JsonQuotaStatusAccount {
    fn from_row(row: &QuotaStatusRow, now_unix_seconds: u64) -> Self {
        Self {
            account_hash: telemetry_hash(row.account_id.as_str()),
            safe_account_label: row.account_label.clone(),
            availability: availability_json(row.availability),
            freshness: freshness_json(row.freshness),
            routing_exclusion: routing_exclusion_json(row.routing_exclusion),
            next_use: row.next_use.clone(),
            limiting_window: row
                .limiting_window
                .map_or("none", |window| quota_window_label(window.window_seconds())),
            quota_evidence_reason: quota_evidence_reason_json(row.quota_evidence_reason),
            short_quota_guard: Some(row.short_pressure),
            weekly_quota_guard: Some(row.long_pressure),
            weekly_survival_margin_basis_points: row.weekly_survival_margin_basis_points,
            weekly_projected_exhaustion_unix_seconds: row.weekly_projected_exhaustion_unix_seconds,
            short_guard_result: short_guard_result_json(row),
            current_active_sessions: row.active_clients_value,
            active_session_source: row.active_clients_source,
            weekly_burn_rate_confidence: run_rate_confidence_label(row.weekly_burn_rate_confidence),
            hard_block_reason: hard_block_reason_json(row),
            short_salvage: Some(row.short_salvage),
            long_salvage: Some(row.long_salvage),
            salvage_tie_key: None,
            routing_reason: routing_reason_json(row.routing_reason),
            preferred_next: row.preferred_next,
            reset_credits_available: row.reset_credits_available_value,
            active_clients: row.active_clients_value,
            active_clients_source: row.active_clients_source,
            updated: row.updated.clone(),
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
    burn_rate_basis_points_per_hour: Option<u32>,
    projected_exhaustion_unix_seconds: Option<u64>,
}

impl JsonRunRateEstimate {
    fn unknown() -> Self {
        Self {
            confidence: "unknown",
            burn_rate_percent_per_hour: None,
            burn_rate_basis_points_per_hour: None,
            projected_exhaustion_unix_seconds: None,
        }
    }

    fn from_estimate(estimate: QuotaRunRateEstimate, now_unix_seconds: u64) -> Self {
        Self {
            confidence: run_rate_confidence_label(estimate.confidence()),
            burn_rate_percent_per_hour: estimate.burn_rate_percent_per_hour(),
            burn_rate_basis_points_per_hour: estimate.burn_rate_basis_points_per_hour(),
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
    guard_deficit_percent: Option<u32>,
    surplus_percent: Option<u32>,
    contributed_to_salvage: bool,
    run_rate: JsonRunRateEstimate,
}

impl JsonQuotaWindow {
    fn from_window(window: &DisplayQuotaWindow, now_unix_seconds: u64) -> Self {
        let (guard_deficit_percent, surplus_percent) =
            window_pressure_and_surplus(window, now_unix_seconds);
        Self {
            window_seconds: window.window_seconds,
            status: quota_window_status_json(window.status),
            remaining_headroom: window_known_headroom(window),
            reset_unix_seconds: window.reset_unix_seconds,
            observed_unix_seconds: Some(window.observed_unix_seconds),
            effective: window.effective,
            guard_deficit_percent,
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
        SelectedPool::LastResort => "last_resort",
        SelectedPool::None => "none",
    }
}

const fn selected_pool_reason_json(value: SelectedPool) -> &'static str {
    match value {
        SelectedPool::Usable => "usable_available",
        SelectedPool::Reserve => "reserve_only",
        SelectedPool::Unknown => "unknown_fallback_only",
        SelectedPool::LastResort => "last_resort_5h_guard",
        SelectedPool::None => "none_available",
    }
}

const fn availability_json(value: AccountAvailability) -> &'static str {
    match value {
        AccountAvailability::Usable => "usable",
        AccountAvailability::Reserve => "reserve",
        AccountAvailability::Retiring => "retiring",
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
        QuotaEvidenceReason::ShortWindowGuard => "short_window_guard",
        QuotaEvidenceReason::AccountDisabled => "account_disabled",
        QuotaEvidenceReason::MissingCredential => "missing_credential",
    }
}

const fn routing_reason_json(value: RoutingReason) -> &'static str {
    value.as_str()
}

fn short_guard_result_json(row: &QuotaStatusRow) -> &'static str {
    if row.routing_reason == RoutingReason::HeldShortWindowGuard
        || row.quota_evidence_reason == QuotaEvidenceReason::ShortWindowGuard
    {
        return "held";
    }
    let Some(short_window) = row
        .windows
        .iter()
        .find(|window| window.window_seconds == V1_SHORT_WINDOW_SECONDS)
    else {
        return "unknown";
    };
    match short_window.status {
        QuotaWindowStatus::Eligible | QuotaWindowStatus::Stale | QuotaWindowStatus::Ineligible => {
            "pass"
        }
        QuotaWindowStatus::Unknown => "unknown",
    }
}

fn hard_block_reason_json(row: &QuotaStatusRow) -> Option<&'static str> {
    match row.quota_evidence_reason {
        QuotaEvidenceReason::Ok => None,
        reason => Some(quota_evidence_reason_json(reason)),
    }
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
        QuotaWindowStatus::Stale => reset,
        QuotaWindowStatus::Unknown => "unknown; needs refresh".to_owned(),
        QuotaWindowStatus::Ineligible if window.remaining_headroom == 0 => reset,
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

fn telemetry_hash(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    let digest = hasher.finalize();
    digest
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
}

fn emit_quota_status_metrics(route_band: &str, rows: &[QuotaStatusRow]) {
    if !rows.iter().any(|row| row.preferred_next) {
        global::meter("codex-router")
            .u64_counter("codex_router_account_rejections_total")
            .build()
            .add(
                1,
                &[
                    KeyValue::new("account.slot", "none"),
                    KeyValue::new("route_band", route_band.to_owned()),
                    KeyValue::new("transport", "cli"),
                    KeyValue::new("selection.reason", "no_quota_candidate"),
                ],
            );
    }

    for row in rows {
        let account_hash = telemetry_hash(row.account_id.as_str());
        if row.preferred_next {
            global::meter("codex-router")
                .u64_counter("codex_router_account_selections_total")
                .build()
                .add(
                    1,
                    &[
                        KeyValue::new("account.slot", account_hash.clone()),
                        KeyValue::new("route_band", route_band.to_owned()),
                        KeyValue::new("transport", "cli"),
                        KeyValue::new("selection.reason", routing_reason_json(row.routing_reason)),
                    ],
                );
        }
        if !row.preferred_next {
            global::meter("codex-router")
                .u64_counter("codex_router_account_rejections_total")
                .build()
                .add(
                    1,
                    &[
                        KeyValue::new("account.slot", account_hash.clone()),
                        KeyValue::new("route_band", route_band.to_owned()),
                        KeyValue::new("transport", "cli"),
                        KeyValue::new("selection.reason", routing_reason_json(row.routing_reason)),
                    ],
                );
        }
        if let Some(active_clients) = row.active_clients_value {
            global::meter("codex-router")
                .u64_gauge("codex_router_active_clients")
                .build()
                .record(
                    u64::from(active_clients),
                    &[
                        KeyValue::new("account.slot", account_hash.clone()),
                        KeyValue::new("route_band", route_band.to_owned()),
                        KeyValue::new("transport", "sqlx_mirror"),
                    ],
                );
        }
        for window in &row.windows {
            global::meter("codex-router")
                .u64_gauge("codex_router_quota_remaining_bucket")
                .build()
                .record(
                    1,
                    &[
                        KeyValue::new("account.slot", account_hash.clone()),
                        KeyValue::new("route_band", route_band.to_owned()),
                        KeyValue::new(
                            "quota.window",
                            quota_window_label(window.window_seconds).to_owned(),
                        ),
                        KeyValue::new(
                            "quota.remaining_bucket",
                            quota_remaining_bucket(window.remaining_headroom),
                        ),
                    ],
                );
        }
        for (window_label, guard_deficit) in
            [("5h", row.short_pressure), ("weekly", row.long_pressure)]
        {
            global::meter("codex-router")
                .u64_gauge("codex_router_quota_guard_bucket")
                .build()
                .record(
                    1,
                    &[
                        KeyValue::new("account.slot", account_hash.clone()),
                        KeyValue::new("route_band", route_band.to_owned()),
                        KeyValue::new("quota.window", window_label),
                        KeyValue::new("quota.guard_bucket", quota_guard_bucket(guard_deficit)),
                    ],
                );
        }
    }
}

fn record_quota_refresh_metric(
    route_band: &str,
    refresh_outcome: &'static str,
    refresh_error_class: &'static str,
) {
    global::meter("codex-router")
        .u64_counter("codex_router_quota_refresh_total")
        .build()
        .add(
            1,
            &[
                KeyValue::new("route_band", route_band.to_owned()),
                KeyValue::new("refresh.outcome", refresh_outcome),
                KeyValue::new("refresh.error_class", refresh_error_class),
            ],
        );
}

fn quota_remaining_bucket(remaining_headroom: u32) -> &'static str {
    match remaining_headroom {
        0 => "empty",
        1..=4 => "lt_5",
        5..=24 => "lt_25",
        25..=49 => "lt_50",
        50..=74 => "lt_75",
        _ => "gte_75",
    }
}

fn quota_guard_bucket(guard_deficit: u32) -> &'static str {
    match guard_deficit {
        0 => "none",
        1..=24 => "low",
        25..=49 => "medium",
        50..=74 => "high",
        _ => "critical",
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
                "--no-refresh" => {
                    // Status is read-only. Keep accepting the old explicit
                    // flag so scripts can state intent without changing
                    // behavior.
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

#[cfg(test)]
mod tests {
    use codex_router_core::ids::AccountId;
    use codex_router_selection::burn_down::RoutingReason;

    use super::*;

    const NOW: u64 = 1_700_000_000;

    #[test]
    fn quota_refresh_selector_window_stale_after_uses_plan_freshness_ceiling() {
        assert_eq!(
            stale_after_unix_seconds(1_000),
            1_300,
            "selector-window last-known-good freshness must use the plan's 300s ceiling"
        );
    }

    #[test]
    fn quota_status_selection_uses_projected_run_rate_like_runtime_selector() {
        let fast_burning_account = account("acct_fast", "fast");
        let slow_burning_account = account("acct_slow", "slow");
        let fast_burning_input = burn_down_input_from_display_windows(
            &fast_burning_account,
            &[
                display_window(
                    V1_SHORT_WINDOW_SECONDS,
                    50,
                    NOW + V1_SHORT_WINDOW_SECONDS,
                    QuotaRunRateEstimate::with_rate(QuotaRunRateConfidence::Normal, 80, 50),
                ),
                display_window(
                    V1_WEEKLY_WINDOW_SECONDS,
                    80,
                    NOW + V1_WEEKLY_WINDOW_SECONDS,
                    QuotaRunRateEstimate::unknown(),
                ),
            ],
            NOW,
        );
        let slow_burning_input = burn_down_input_from_display_windows(
            &slow_burning_account,
            &[
                display_window(
                    V1_SHORT_WINDOW_SECONDS,
                    50,
                    NOW + V1_SHORT_WINDOW_SECONDS,
                    QuotaRunRateEstimate::with_rate(QuotaRunRateConfidence::Normal, 1, 50),
                ),
                display_window(
                    V1_WEEKLY_WINDOW_SECONDS,
                    80,
                    NOW + V1_WEEKLY_WINDOW_SECONDS,
                    QuotaRunRateEstimate::unknown(),
                ),
            ],
            NOW,
        );

        let assessment = assess_route_band(BurnDownRouteBandAssessmentInput::new(
            RouteBand::Responses,
            NOW,
            vec![fast_burning_input, slow_burning_input],
        ));

        assert_eq!(
            assessment.preferred_next().map(AccountId::as_str),
            Some("acct_slow")
        );
        let Some(slow_burning_assessment) = assessment
            .accounts()
            .iter()
            .find(|account| account.account_id().as_str() == "acct_slow")
        else {
            panic!("slow-burning account should be assessed");
        };
        assert!(matches!(
            slow_burning_assessment.routing_reason(),
            RoutingReason::PreferredProjectedBurn | RoutingReason::PreferredSafestQuota
        ));
    }

    #[test]
    fn quota_status_formats_subpercent_burn_with_runout() {
        let estimate = QuotaRunRateEstimate::with_rate_basis_points_per_hour(
            QuotaRunRateConfidence::Normal,
            45,
            6,
        );

        assert_eq!(
            format_run_rate_estimate(estimate, NOW),
            "normal burn 0.45%/h; runout in 13h 20m"
        );
    }

    #[test]
    fn quota_status_connection_rate_distinguishes_aggregate_only_burn() {
        let aggregate_only_snapshot = QuotaPaceSnapshot {
            remaining_headroom: 55,
            reset_unix_seconds: Some(NOW + V1_WEEKLY_WINDOW_SECONDS),
            projected_exhaustion_unix_seconds: Some(NOW + 36 * 60 * 60),
            projected_candidate_burn_basis_points_per_hour: Some(141),
            aggregate_burn_basis_points_per_hour: Some(141),
            per_connection_burn_basis_points_per_hour: None,
            confidence: QuotaRunRateConfidence::Low,
        };

        assert_eq!(
            quota_connection_rate_summary(Some(aggregate_only_snapshot)),
            "not attributed (low)"
        );
    }

    #[test]
    fn quota_status_json_exposes_subpercent_burn_basis_points() {
        let estimate = QuotaRunRateEstimate::with_rate_basis_points_per_hour(
            QuotaRunRateConfidence::Normal,
            45,
            6,
        );

        let json = serde_json::to_value(JsonRunRateEstimate::from_estimate(estimate, NOW))
            .unwrap_or_else(|error| panic!("run-rate JSON should serialize: {error}"));

        assert_eq!(json["burn_rate_percent_per_hour"], 0);
        assert_eq!(json["burn_rate_basis_points_per_hour"], 45);
        assert!(json["projected_exhaustion_unix_seconds"].is_number());
    }

    #[test]
    fn quota_status_sample_confidence_uses_15_minute_display_boundary() {
        assert_eq!(
            sample_metadata_from_observed_windows(&[NOW - 899], NOW).confidence,
            SampleConfidence::Fresh
        );
        assert_eq!(
            sample_metadata_from_observed_windows(&[NOW - 900], NOW).confidence,
            SampleConfidence::Fresh
        );
        assert_eq!(
            sample_metadata_from_observed_windows(&[NOW - 901], NOW).confidence,
            SampleConfidence::Stale
        );
    }

    #[test]
    fn quota_status_sample_confidence_uses_displayed_value_window_age() {
        let windows = vec![
            DisplayQuotaWindow {
                observed_unix_seconds: NOW - 30,
                ..display_window(
                    V1_SHORT_WINDOW_SECONDS,
                    20,
                    NOW + V1_SHORT_WINDOW_SECONDS,
                    QuotaRunRateEstimate::unknown(),
                )
            },
            DisplayQuotaWindow {
                observed_unix_seconds: NOW - 901,
                ..display_window(
                    V1_WEEKLY_WINDOW_SECONDS,
                    70,
                    NOW + V1_WEEKLY_WINDOW_SECONDS,
                    QuotaRunRateEstimate::unknown(),
                )
            },
            DisplayQuotaWindow {
                status: QuotaWindowStatus::Unknown,
                observed_unix_seconds: NOW - 3_600,
                ..display_window(
                    V1_WEEKLY_WINDOW_SECONDS * 2,
                    0,
                    NOW + V1_WEEKLY_WINDOW_SECONDS,
                    QuotaRunRateEstimate::unknown(),
                )
            },
        ];

        let sample = sample_metadata_from_display_windows(&windows, NOW);

        assert_eq!(sample.confidence, SampleConfidence::Stale);
        assert_eq!(sample.age_seconds, Some(901));
        assert_eq!(sample.semantic_label, "sample stale");
    }

    #[test]
    fn quota_status_row_sample_uses_only_weekly_window_age() {
        let mut report = quota_capture_report();
        let row = report
            .rows
            .get_mut(0)
            .unwrap_or_else(|| panic!("capture report should include a selected row"));
        for window in &mut row.windows {
            if window.window_seconds == V1_SHORT_WINDOW_SECONDS {
                window.observed_unix_seconds = NOW - 901;
            } else if window.window_seconds == V1_WEEKLY_WINDOW_SECONDS {
                window.observed_unix_seconds = NOW - 30;
            }
        }

        let view_model = quota_status_view_model(&report, report.rows(), 120);
        let rendered_row = view_model
            .rows
            .first()
            .unwrap_or_else(|| panic!("quota view model should include a row"));
        let selected = view_model
            .selected
            .as_ref()
            .unwrap_or_else(|| panic!("quota view model should include selected details"));

        assert_eq!(
            rendered_row.sample_metadata.confidence,
            SampleConfidence::Fresh
        );
        assert_eq!(rendered_row.sample_metadata.age_seconds, Some(30));
        assert_eq!(selected.sample_metadata.confidence, SampleConfidence::Stale);
        assert_eq!(selected.sample_metadata.age_seconds, Some(901));
    }

    #[test]
    fn quota_status_reset_pace_classifies_thresholds() {
        for (multiple_basis_points, expected_state) in [
            (79, ResetPaceState::UnderBurning),
            (80, ResetPaceState::Healthy),
            (100, ResetPaceState::Healthy),
            (120, ResetPaceState::Healthy),
            (121, ResetPaceState::OverBurning),
        ] {
            let view_model =
                reset_pace_view_model_from_multiple_basis_points(Some(multiple_basis_points));

            assert_eq!(
                view_model.state, expected_state,
                "{multiple_basis_points} basis points should classify correctly"
            );
        }
    }

    #[test]
    fn quota_status_reset_pace_meter_fills_from_center_by_direction() {
        for (multiple_basis_points, expected_meter) in [
            (9, "■■■■■■■│□□□□□□□"),
            (25, "□□■■■■■│□□□□□□□"),
            (50, "□□□□■■■│□□□□□□□"),
            (79, "□□□□□□■│□□□□□□□"),
            (80, "□□□□□□□│□□□□□□□"),
            (100, "□□□□□□□│□□□□□□□"),
            (120, "□□□□□□□│□□□□□□□"),
            (121, "□□□□□□□│■□□□□□□"),
            (150, "□□□□□□□│■■■□□□□"),
            (200, "□□□□□□□│■■■■■■■"),
        ] {
            let view_model =
                reset_pace_view_model_from_multiple_basis_points(Some(multiple_basis_points));

            assert_eq!(
                reset_pace_meter_text(&view_model),
                expected_meter,
                "{multiple_basis_points} reset-pace basis points should fill from the center in the matching direction"
            );
            assert_eq!(
                reset_pace_meter_text(&view_model).chars().count(),
                15,
                "reset-pace meter must always replace fixed slots, not add glyphs"
            );
        }
    }

    #[test]
    fn quota_status_reset_pace_over_meter_uses_window_reset_denominator() {
        for (reset_seconds, projected_exhaustion_seconds, expected_meter) in [
            (
                V1_WEEKLY_WINDOW_SECONDS,
                2 * 24 * 60 * 60,
                "□□□□□□□│■■■■■□□",
            ),
            (V1_SHORT_WINDOW_SECONDS, 2 * 60 * 60, "□□□□□□□│■■■■■□□"),
        ] {
            let snapshot = QuotaPaceSnapshot {
                remaining_headroom: 10,
                reset_unix_seconds: Some(NOW + reset_seconds),
                projected_exhaustion_unix_seconds: Some(NOW + projected_exhaustion_seconds),
                projected_candidate_burn_basis_points_per_hour: Some(300),
                aggregate_burn_basis_points_per_hour: Some(300),
                per_connection_burn_basis_points_per_hour: None,
                confidence: QuotaRunRateConfidence::Low,
            };

            let view_model = reset_pace_view_model_from_snapshot(Some(snapshot), NOW);

            assert_eq!(
                reset_pace_meter_text(&view_model),
                expected_meter,
                "over-pace meter should normalize early runout by this window's reset time"
            );
        }
    }

    #[test]
    fn quota_status_reset_pace_over_two_x_shows_runout_impact() {
        let snapshot = QuotaPaceSnapshot {
            remaining_headroom: 10,
            reset_unix_seconds: Some(NOW + 10 * 60 * 60),
            projected_exhaustion_unix_seconds: Some(NOW + 3 * 60 * 60),
            projected_candidate_burn_basis_points_per_hour: Some(300),
            aggregate_burn_basis_points_per_hour: Some(300),
            per_connection_burn_basis_points_per_hour: None,
            confidence: QuotaRunRateConfidence::Low,
        };

        let view_model = reset_pace_view_model_from_snapshot(Some(snapshot), NOW);

        assert_eq!(view_model.multiple_label, "3.00x reset pace");
        assert_eq!(view_model.impact_label, Some("runs out 3h".to_owned()));
        assert_eq!(plain_reset_pace_summary(&view_model), "runs out 3h");
    }

    #[test]
    fn quota_status_reset_pace_at_two_x_keeps_multiplier_label() {
        let snapshot = QuotaPaceSnapshot {
            remaining_headroom: 10,
            reset_unix_seconds: Some(NOW + 10 * 60 * 60),
            projected_exhaustion_unix_seconds: Some(NOW + 5 * 60 * 60),
            projected_candidate_burn_basis_points_per_hour: Some(200),
            aggregate_burn_basis_points_per_hour: Some(200),
            per_connection_burn_basis_points_per_hour: None,
            confidence: QuotaRunRateConfidence::Low,
        };

        let view_model = reset_pace_view_model_from_snapshot(Some(snapshot), NOW);

        assert_eq!(view_model.multiple_label, "2.00x reset pace");
        assert_eq!(view_model.impact_label, None);
    }

    #[test]
    fn quota_status_reset_pace_unavailable_has_marker_meter() {
        let view_model = reset_pace_view_model_from_multiple_basis_points(None);

        assert_eq!(view_model.state, ResetPaceState::Unavailable);
        assert_eq!(view_model.semantic_label, "burn unavailable");
        assert_eq!(view_model.meter_left_segments.filled, 0);
        assert_eq!(view_model.meter_right_segments.filled, 0);
        assert_eq!(view_model.meter_left_segments.empty, 7);
        assert_eq!(view_model.meter_right_segments.empty, 7);
        assert_eq!(view_model.center_marker, '│');
        assert!(view_model.unavailable_reason.is_some());
    }

    #[test]
    fn quota_status_short_reset_pace_collects_data_for_insufficient_snapshot_samples() {
        let snapshot = QuotaPaceSnapshot {
            remaining_headroom: 99,
            reset_unix_seconds: Some(NOW + V1_SHORT_WINDOW_SECONDS),
            projected_exhaustion_unix_seconds: None,
            projected_candidate_burn_basis_points_per_hour: None,
            aggregate_burn_basis_points_per_hour: None,
            per_connection_burn_basis_points_per_hour: None,
            confidence: QuotaRunRateConfidence::Insufficient,
        };

        let view_model = short_reset_pace_view_model_from_snapshot(Some(snapshot), NOW);

        assert_eq!(view_model.state, ResetPaceState::Unavailable);
        assert_eq!(view_model.semantic_label, "collecting data");
        assert_eq!(reset_pace_meter_text(&view_model), "□□□□□□□│□□□□□□□");
    }

    #[test]
    fn quota_status_display_reset_pace_requires_three_recent_samples() {
        let reset_unix_seconds = NOW + V1_WEEKLY_WINDOW_SECONDS;
        let observations = [
            QuotaRunRateObservation::new(NOW - 899, reset_unix_seconds, 50),
            QuotaRunRateObservation::new(NOW - 600, reset_unix_seconds, 48),
        ];

        let display_estimate = display_quota_run_rate_estimate(
            V1_WEEKLY_WINDOW_SECONDS,
            NOW,
            reset_unix_seconds,
            &observations,
        );
        let routing_authority_estimate = QuotaRunRateEstimator::new(
            DEFAULT_REFRESH_STALE_AFTER_GRACE_SECONDS,
        )
        .estimate(NOW, reset_unix_seconds, &observations);

        assert_eq!(
            display_estimate.confidence(),
            QuotaRunRateConfidence::Insufficient
        );
        assert!(display_estimate.burn_rate_basis_points_per_hour().is_none());
        assert_eq!(
            routing_authority_estimate.confidence(),
            QuotaRunRateConfidence::Stale,
            "runtime authority must still go stale at the persisted 300s boundary"
        );
    }

    #[test]
    fn quota_status_display_burn_uses_recent_window_and_sample_confidence() {
        let reset_unix_seconds = NOW + V1_WEEKLY_WINDOW_SECONDS;
        let observations = [
            QuotaRunRateObservation::new(NOW - 20_000, reset_unix_seconds, 100),
            QuotaRunRateObservation::new(NOW - 19_000, reset_unix_seconds, 50),
            QuotaRunRateObservation::new(NOW - 3_000, reset_unix_seconds, 50),
            QuotaRunRateObservation::new(NOW - 1_800, reset_unix_seconds, 49),
            QuotaRunRateObservation::new(NOW - 900, reset_unix_seconds, 48),
            QuotaRunRateObservation::new(NOW - 300, reset_unix_seconds, 47),
        ];

        let estimate = display_quota_run_rate_estimate(
            V1_WEEKLY_WINDOW_SECONDS,
            NOW,
            reset_unix_seconds,
            &observations,
        );

        assert_eq!(estimate.confidence(), QuotaRunRateConfidence::Low);
        assert_eq!(estimate.burn_rate_basis_points_per_hour(), Some(400));
    }

    #[test]
    fn quota_status_display_burn_requires_five_recent_samples_for_normal_confidence() {
        let reset_unix_seconds = NOW + V1_WEEKLY_WINDOW_SECONDS;
        let four_observations = [
            QuotaRunRateObservation::new(NOW - 2_700, reset_unix_seconds, 50),
            QuotaRunRateObservation::new(NOW - 1_800, reset_unix_seconds, 49),
            QuotaRunRateObservation::new(NOW - 900, reset_unix_seconds, 48),
            QuotaRunRateObservation::new(NOW, reset_unix_seconds, 47),
        ];
        let five_observations = [
            QuotaRunRateObservation::new(NOW - 3_600, reset_unix_seconds, 51),
            QuotaRunRateObservation::new(NOW - 2_700, reset_unix_seconds, 50),
            QuotaRunRateObservation::new(NOW - 1_800, reset_unix_seconds, 49),
            QuotaRunRateObservation::new(NOW - 900, reset_unix_seconds, 48),
            QuotaRunRateObservation::new(NOW, reset_unix_seconds, 47),
        ];

        let four_sample_estimate = display_quota_run_rate_estimate(
            V1_WEEKLY_WINDOW_SECONDS,
            NOW,
            reset_unix_seconds,
            &four_observations,
        );
        let five_sample_estimate = display_quota_run_rate_estimate(
            V1_WEEKLY_WINDOW_SECONDS,
            NOW,
            reset_unix_seconds,
            &five_observations,
        );

        assert_eq!(
            four_sample_estimate.confidence(),
            QuotaRunRateConfidence::Low
        );
        assert_eq!(
            five_sample_estimate.confidence(),
            QuotaRunRateConfidence::Normal
        );
    }

    #[test]
    fn quota_status_display_burn_uses_all_recent_samples() {
        let reset_unix_seconds = NOW + V1_WEEKLY_WINDOW_SECONDS;
        let observations = [
            QuotaRunRateObservation::new(NOW - 9_000, reset_unix_seconds, 80),
            QuotaRunRateObservation::new(NOW - 3_600, reset_unix_seconds, 54),
            QuotaRunRateObservation::new(NOW - 2_700, reset_unix_seconds, 53),
            QuotaRunRateObservation::new(NOW - 1_800, reset_unix_seconds, 52),
            QuotaRunRateObservation::new(NOW - 900, reset_unix_seconds, 51),
            QuotaRunRateObservation::new(NOW, reset_unix_seconds, 50),
        ];

        let estimate = display_quota_run_rate_estimate(
            V1_WEEKLY_WINDOW_SECONDS,
            NOW,
            reset_unix_seconds,
            &observations,
        );

        assert_eq!(estimate.confidence(), QuotaRunRateConfidence::Normal);
        assert_eq!(
            estimate.burn_rate_basis_points_per_hour(),
            Some(1_200),
            "display burn should use every sample inside the recent lookback, not only the newest five samples"
        );
    }

    #[test]
    fn quota_status_display_reset_pace_uses_display_estimate_when_projection_is_stale() {
        let windows = vec![display_window(
            V1_WEEKLY_WINDOW_SECONDS,
            50,
            NOW + V1_WEEKLY_WINDOW_SECONDS,
            QuotaRunRateEstimate::with_rate_basis_points_per_hour(
                QuotaRunRateConfidence::Low,
                2_000,
                50,
            ),
        )];
        let stale_projected_weekly_window =
            QuotaWindowFact::new(V1_WEEKLY_WINDOW_SECONDS, QuotaWindowStatus::Stale)
                .with_remaining_headroom(50)
                .with_reset_unix_seconds(NOW + V1_WEEKLY_WINDOW_SECONDS)
                .with_observed_unix_seconds(NOW - 301)
                .with_burn_rate_confidence(QuotaRunRateConfidence::Stale);

        let snapshot = quota_pace_snapshot(&windows, Some(&stale_projected_weekly_window), NOW)
            .unwrap_or_else(|| panic!("weekly display window should produce pace snapshot"));

        assert_eq!(snapshot.aggregate_burn_basis_points_per_hour, Some(2_000));
        assert_eq!(
            snapshot.projected_candidate_burn_basis_points_per_hour,
            Some(2_000)
        );
        assert_eq!(snapshot.confidence, QuotaRunRateConfidence::Low);
    }

    #[test]
    fn quota_status_shared_dto_carries_sample_and_reset_pace_without_string_parsing() {
        let report = quota_capture_report();

        let view_model = quota_status_view_model(&report, report.rows(), 120);
        let row = view_model
            .rows
            .first()
            .unwrap_or_else(|| panic!("quota view model should include an account row"));

        assert_eq!(row.sample_metadata.confidence, SampleConfidence::Fresh);
        assert_eq!(row.sample_metadata.semantic_label, "sample fresh");
        assert_ne!(row.reset_pace.state, ResetPaceState::Unavailable);
        assert!(
            row.reset_pace.multiple_label.contains("reset pace"),
            "reset pace should be carried as typed row metadata, not rebuilt from safe-pace strings"
        );
        assert!(
            row.burn_meter.contains('│'),
            "row burn meter should use the same center-out reset-pace meter as the visible reset pace"
        );
        assert!(
            !row.weekly_pace.contains("safe pace"),
            "legacy safe-pace copy must not survive in the shared DTO"
        );
    }

    #[test]
    fn quota_status_selected_details_carry_5h_reset_pace_from_short_window() {
        let mut report = quota_capture_report();
        let selected_row = report
            .rows
            .first_mut()
            .unwrap_or_else(|| panic!("capture report should include selected row"));
        let short_window = selected_row
            .windows
            .iter_mut()
            .find(|window| window.window_seconds == V1_SHORT_WINDOW_SECONDS)
            .unwrap_or_else(|| panic!("selected row should include a short window"));
        short_window.run_rate_estimate = QuotaRunRateEstimate::with_rate_basis_points_per_hour(
            QuotaRunRateConfidence::Low,
            5_000,
            short_window.remaining_headroom,
        );

        let view_model = quota_status_view_model(&report, report.rows(), 120);
        let selected = view_model
            .selected
            .as_ref()
            .unwrap_or_else(|| panic!("quota view model should include selected details"));

        assert_eq!(selected.short_reset_pace.state, ResetPaceState::OverBurning);
        assert!(
            selected
                .short_reset_pace
                .impact_label
                .as_deref()
                .is_some_and(|label| label.starts_with("runs out ")),
            "5h reset pace should carry its own runout impact: {:?}",
            selected.short_reset_pace
        );
    }

    #[test]
    fn quota_status_width_contract_preserves_layout() {
        let report = quota_capture_report();

        for width in [48, 72, 90, 120] {
            let mut output = Vec::new();
            must_ok(write_quota_table(&mut output, &report, Some(width)));
            let text = must_ok(String::from_utf8(output));
            assert_quota_capture_width_contract(width, &text);
        }

        let blocked_report = blocked_quota_capture_report();
        let mut output = Vec::new();
        must_ok(write_quota_table(&mut output, &blocked_report, Some(80)));
        let text = must_ok(String::from_utf8(output));
        assert!(
            text.contains("responses -> none    no usable accounts"),
            "blocked capture should expose compact no-selection route state:\n{text}"
        );
        assert!(
            text.lines().all(|line| line.chars().count() <= 80),
            "blocked quota capture overflowed:\n{text}"
        );
    }

    #[test]
    fn quota_status_empty_windows_keep_weekly_bar_and_show_exhausted_reset_pace() {
        let mut report = blocked_quota_capture_report();
        for row in &mut report.rows {
            for window in &mut row.windows {
                window.status = QuotaWindowStatus::Ineligible;
            }
        }
        let mut output = Vec::new();

        must_ok(write_quota_table(&mut output, &report, Some(120)));
        let text = must_ok(String::from_utf8(output));

        assert!(
            text.contains("░░░░░░░░░░ 0% left, reset 7d"),
            "depleted weekly quota should keep its quota bar and reset hint:\n{text}"
        );
        assert!(text.contains("Exhausted"), "{text}");
        assert!(
            !text.contains("🅇  Exhausted"),
            "depleted reset pace should not include the icon marker:\n{text}"
        );
        assert!(
            !text.contains("runs out now"),
            "depleted reset pace should not show old runout copy:\n{text}"
        );
    }

    #[test]
    fn quota_status_terminal_color_keeps_exhausted_red() {
        let report = blocked_quota_capture_report();
        let mut output = Vec::new();

        must_ok(write_quota_table_with_style(
            &mut output,
            &report,
            Some(120),
            QuotaTableStyle::TerminalColor,
        ));
        let text = must_ok(String::from_utf8(output));

        assert!(
            text.contains("\u{1b}[38;5;9mExhausted"),
            "exhausted quota label should keep the red over-burning color:\n{text:?}"
        );
    }

    #[test]
    fn quota_status_table_separates_quota_bars_from_burn_bars() {
        let report = quota_capture_report();
        let mut output = Vec::new();

        must_ok(write_quota_table(&mut output, &report, Some(120)));
        let text = must_ok(String::from_utf8(output));

        assert!(
            text.contains("Pace"),
            "quota table should label the main-list forecast column:\n{text}"
        );
        assert!(
            text.contains("  Account"),
            "account header should reserve selector-marker space:\n{text}"
        );
        assert!(
            text.contains("Quota windows") && text.contains("Reset pace"),
            "selected account details should separate quota windows from reset pace:\n{text}"
        );
        assert!(
            text.contains("%/h") && text.contains("%/h/conn"),
            "quota table should expose total and per-connection rate units:\n{text}"
        );
        assert!(
            text.contains("weekly")
                && text.contains("5h")
                && text.contains("█")
                && text.contains("% left, reset"),
            "main account rows should show both quota windows with quota bars:\n{text}"
        );
        assert!(
            text.contains("weekly") && text.contains("5h") && text.contains("reset pace"),
            "quota table should show the selected reset pace as an explicit block meter:\n{text}"
        );
        assert!(
            text.contains("reset pace"),
            "main account rows should show the weekly pace meter:\n{text}"
        );
        assert!(
            !text.contains("current [")
                && !text.contains("safe pace")
                && !text.contains("ahead to reset")
                && !text.contains("safe pace unknown"),
            "quota table should not use legacy burn/safe-pace copy:\n{text}"
        );
    }

    #[test]
    fn quota_status_table_shows_stale_values_with_sample_marker_without_refresh_filler() {
        let mut report = quota_capture_report();
        let row = report
            .rows
            .get_mut(0)
            .unwrap_or_else(|| panic!("capture report should include a selected row"));
        for window in &mut row.windows {
            window.status = QuotaWindowStatus::Stale;
            window.observed_unix_seconds = NOW - 901;
        }
        row.short_window = format_window_cell(&row.windows, V1_SHORT_WINDOW_SECONDS, NOW, true);
        row.weekly_window = format_window_cell(&row.windows, V1_WEEKLY_WINDOW_SECONDS, NOW, true);
        row.freshness = QuotaEvidenceFreshness::Stale;

        let mut output = Vec::new();
        must_ok(write_quota_table(&mut output, &report, Some(120)));
        let text = must_ok(String::from_utf8(output));

        assert!(text.contains("█") && text.contains("% left"), "{text}");
        assert!(text.contains("sample stale 15m 1s"), "{text}");
        assert!(
            !text.contains("needs refresh"),
            "stale value-bearing status output should show values and mark sample stale once:\n{text}"
        );
    }

    #[test]
    fn quota_status_view_model_route_line_compacts_reason_and_burn_rate() {
        let report = quota_capture_report();
        let view_model = quota_status_view_model(&report, report.rows(), 120);

        assert_eq!(
            view_model.route_line, "responses -> ssdev    safest quota    burn 0.1%/h",
            "route line should identify the selected account, reason, burn rate, and limiting window without a second header line"
        );
        assert!(view_model.why_line.is_empty());
    }

    #[test]
    fn quota_status_view_model_reports_serving_clients_from_active_mirror() {
        let report = quota_capture_report();
        let view_model = quota_status_view_model(&report, report.rows(), 120);

        assert_eq!(view_model.serving_clients, Some(5));
    }

    #[test]
    fn quota_status_table_can_emit_terminal_color() {
        let report = quota_capture_report();
        let mut output = Vec::new();

        must_ok(write_quota_table_with_style(
            &mut output,
            &report,
            Some(120),
            QuotaTableStyle::TerminalColor,
        ));
        let text = must_ok(String::from_utf8(output));

        assert!(
            text.contains("\x1b["),
            "quota table should emit ANSI styling:\n{text:?}"
        );
        assert!(
            text.contains("\x1b[38;5;11m") && text.contains("pace under"),
            "quota pace should emit state color:\n{text:?}"
        );
        assert!(
            !text.contains("\x1b[32m"),
            "quota status should avoid the old mixed green/yellow status palette:\n{text:?}"
        );
        assert!(
            !text.contains("\x1b[48;2;58;70;122m"),
            "quota colors should not use the old blue selected-row background:\n{text:?}"
        );
    }

    #[test]
    #[ignore = "writes visual quota capture artifacts for design review"]
    fn quota_status_capture_artifacts_for_design_review() {
        let capture_dir = capture_dir();

        for case in QuotaCaptureDesignCase::ALL {
            let report = quota_capture_case_report(case);
            for width in [48, 160] {
                let mut output = Vec::new();
                must_ok(write_quota_table(&mut output, &report, Some(width)));
                let text = must_ok(String::from_utf8(output));
                let mut ansi_output = Vec::new();
                must_ok(write_quota_table_with_style(
                    &mut ansi_output,
                    &report,
                    Some(width),
                    QuotaTableStyle::TerminalColor,
                ));
                let ansi_text = must_ok(String::from_utf8(ansi_output));
                write_capture_pair_with_svg_text(
                    &capture_dir,
                    &format!("{}-{width}", case.file_stem()),
                    &text,
                    &ansi_text,
                );
            }
        }
    }

    #[test]
    fn quota_status_telemetry_contract_uses_scrubbed_low_cardinality_labels() {
        let source = include_str!("quota.rs");
        let Some(before_trace_event_name) = source
            .split("\"codex_router.quota_status_selection\"")
            .next()
        else {
            panic!("quota status tracing event should have a stable event name");
        };
        let Some(trace_event) = before_trace_event_name.rsplit("tracing::info!(").next() else {
            panic!("quota status tracing event should exist");
        };
        let Some(after_function_name) = source.split("fn emit_quota_status_metrics").nth(1) else {
            panic!("emit_quota_status_metrics helper should exist");
        };
        let Some(metrics_helper) = after_function_name
            .split("fn record_quota_refresh_metric")
            .next()
        else {
            panic!("quota status metrics helper should precede refresh metric helper");
        };

        for required_label in [
            "account.slot",
            "route_band",
            "transport",
            "selection.reason",
            "quota.window",
            "quota.remaining_bucket",
            "quota.guard_bucket",
        ] {
            assert!(
                metrics_helper.contains(required_label),
                "quota status telemetry must include {required_label}"
            );
        }
        for required_trace_label in [
            "route_band",
            "selected_pool",
            "selection.reason",
            "preferred.account_hash",
            "active_client.source",
        ] {
            assert!(
                trace_event.contains(required_trace_label),
                "quota status tracing attributes must include low-cardinality {required_trace_label}"
            );
        }
        for forbidden_label in [
            "account.id",
            "account.label",
            "reservation.id",
            "payload",
            "token",
            "sample.age_seconds",
            "sample.age_text",
            "provider.error",
        ] {
            assert!(
                !metrics_helper.contains(forbidden_label),
                "quota status telemetry must not include {forbidden_label}"
            );
            assert!(
                !trace_event.contains(forbidden_label),
                "quota status tracing attributes must not include {forbidden_label}"
            );
        }
    }

    struct QuotaCaptureRowFixture {
        account_id_value: &'static str,
        account_label: &'static str,
        preferred_next: bool,
        short_remaining: u32,
        weekly_remaining: u32,
        freshness: QuotaEvidenceFreshness,
        availability: AccountAvailability,
        routing_reason: RoutingReason,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum QuotaCaptureDesignCase {
        FreshHealthy,
        StaleUnder,
        DegradedOver,
        UnavailableBurn,
    }

    impl QuotaCaptureDesignCase {
        const ALL: [Self; 4] = [
            Self::FreshHealthy,
            Self::StaleUnder,
            Self::DegradedOver,
            Self::UnavailableBurn,
        ];

        const fn file_stem(self) -> &'static str {
            match self {
                Self::FreshHealthy => "fresh-healthy",
                Self::StaleUnder => "stale-under",
                Self::DegradedOver => "degraded-over",
                Self::UnavailableBurn => "unavailable-burn",
            }
        }
    }

    fn quota_capture_row(fixture: QuotaCaptureRowFixture) -> QuotaStatusRow {
        let windows = vec![
            display_window(
                V1_SHORT_WINDOW_SECONDS,
                fixture.short_remaining,
                NOW + V1_SHORT_WINDOW_SECONDS,
                QuotaRunRateEstimate::unknown(),
            ),
            display_window(
                V1_WEEKLY_WINDOW_SECONDS,
                fixture.weekly_remaining,
                NOW + V1_WEEKLY_WINDOW_SECONDS,
                QuotaRunRateEstimate::unknown(),
            ),
        ];
        let quota_evidence_reason = if fixture.freshness == QuotaEvidenceFreshness::Stale {
            QuotaEvidenceReason::WindowExhausted
        } else if fixture.routing_reason == RoutingReason::HeldShortWindowGuard {
            QuotaEvidenceReason::ShortWindowGuard
        } else {
            QuotaEvidenceReason::Ok
        };

        QuotaStatusRow {
            account_id: account_id(fixture.account_id_value),
            account_label: fixture.account_label.to_owned(),
            account_status: "enabled".to_owned(),
            short_window: format_window_cell(&windows, V1_SHORT_WINDOW_SECONDS, NOW, false),
            weekly_window: format_window_cell(&windows, V1_WEEKLY_WINDOW_SECONDS, NOW, false),
            pace: "history unknown".to_owned(),
            burn: "quota guard 5h 0% / weekly 8%".to_owned(),
            updated: if fixture.freshness == QuotaEvidenceFreshness::Stale {
                "failed 42m ago: network".to_owned()
            } else {
                "ok 14s ago".to_owned()
            },
            active_clients: "0 clients\nmirror <= 2h".to_owned(),
            active_clients_value: Some(
                if fixture.routing_reason == RoutingReason::HeldShortWindowGuard {
                    5
                } else {
                    0
                },
            ),
            active_clients_source: "sqlx_mirror",
            reset_credits_available: "2 available".to_owned(),
            reset_credits_available_value: Some(2),
            routing: format_routing_reason(fixture.routing_reason).to_owned(),
            next_use: format_next_use_for_capture(fixture.routing_reason).to_owned(),
            weekly_pace: Some(QuotaPaceSnapshot {
                remaining_headroom: fixture.weekly_remaining,
                reset_unix_seconds: Some(NOW + V1_WEEKLY_WINDOW_SECONDS),
                projected_exhaustion_unix_seconds: Some(
                    NOW + u64::from(fixture.weekly_remaining)
                        .saturating_mul(100)
                        .saturating_mul(3_600)
                        / 10,
                ),
                projected_candidate_burn_basis_points_per_hour: Some(10),
                aggregate_burn_basis_points_per_hour: Some(8),
                per_connection_burn_basis_points_per_hour: Some(5),
                confidence: QuotaRunRateConfidence::Normal,
            }),
            windows,
            availability: fixture.availability,
            freshness: fixture.freshness,
            routing_exclusion: RoutingExclusion::None,
            quota_evidence_reason,
            routing_reason: fixture.routing_reason,
            preferred_next: fixture.preferred_next,
            short_pressure: 0,
            long_pressure: 8,
            short_salvage: fixture.short_remaining,
            long_salvage: fixture.weekly_remaining,
            limiting_window: None,
            weekly_survival_margin_basis_points: None,
            weekly_projected_exhaustion_unix_seconds: None,
            weekly_burn_rate_confidence: QuotaRunRateConfidence::Unknown,
        }
    }

    fn quota_capture_report() -> QuotaStatusReport {
        QuotaStatusReport {
            app_version: env!("CARGO_PKG_VERSION").to_owned(),
            route_band: "responses".to_owned(),
            selected_pool: SelectedPool::Usable,
            preferred_next_account_id: Some(account_id("acct_ssdev")),
            selection_projection_source: SelectionProjectionSource::SqlxProjection,
            now_unix_seconds: NOW,
            rows: vec![
                quota_capture_row(QuotaCaptureRowFixture {
                    account_id_value: "acct_ssdev",
                    account_label: "ssdev",
                    preferred_next: true,
                    short_remaining: 99,
                    weekly_remaining: 83,
                    freshness: QuotaEvidenceFreshness::Fresh,
                    availability: AccountAvailability::Usable,
                    routing_reason: RoutingReason::PreferredSafestQuota,
                }),
                quota_capture_row(QuotaCaptureRowFixture {
                    account_id_value: "acct_askluna",
                    account_label: "askluna",
                    preferred_next: false,
                    short_remaining: 100,
                    weekly_remaining: 99,
                    freshness: QuotaEvidenceFreshness::Fresh,
                    availability: AccountAvailability::Usable,
                    routing_reason: RoutingReason::AvailableSamePool,
                }),
                quota_capture_row(QuotaCaptureRowFixture {
                    account_id_value: "acct_matches",
                    account_label: "matches",
                    preferred_next: false,
                    short_remaining: 94,
                    weekly_remaining: 94,
                    freshness: QuotaEvidenceFreshness::Fresh,
                    availability: AccountAvailability::Reserve,
                    routing_reason: RoutingReason::HeldShortWindowGuard,
                }),
                quota_capture_row(QuotaCaptureRowFixture {
                    account_id_value: "acct_legacy",
                    account_label: "legacy",
                    preferred_next: false,
                    short_remaining: 0,
                    weekly_remaining: 0,
                    freshness: QuotaEvidenceFreshness::Stale,
                    availability: AccountAvailability::Blocked,
                    routing_reason: RoutingReason::BlockedWindowExhausted,
                }),
            ],
        }
    }

    fn quota_capture_case_report(case: QuotaCaptureDesignCase) -> QuotaStatusReport {
        let mut report = quota_capture_report();
        match case {
            QuotaCaptureDesignCase::FreshHealthy => {
                let selected_row = report
                    .rows
                    .get_mut(0)
                    .unwrap_or_else(|| panic!("capture report should include selected row"));
                selected_row.weekly_pace = selected_row.weekly_pace.map(|mut pace| {
                    pace.projected_candidate_burn_basis_points_per_hour = Some(49);
                    pace.aggregate_burn_basis_points_per_hour = Some(49);
                    pace
                });
            }
            QuotaCaptureDesignCase::StaleUnder => {
                let selected_row = report
                    .rows
                    .get_mut(0)
                    .unwrap_or_else(|| panic!("capture report should include selected row"));
                selected_row.freshness = QuotaEvidenceFreshness::Stale;
                for window in &mut selected_row.windows {
                    window.status = QuotaWindowStatus::Stale;
                    window.observed_unix_seconds = NOW - 901;
                }
                selected_row.updated = "failed 15m 1s ago: network".to_owned();
                selected_row.weekly_pace = selected_row.weekly_pace.map(|mut pace| {
                    pace.projected_candidate_burn_basis_points_per_hour = Some(10);
                    pace.aggregate_burn_basis_points_per_hour = Some(10);
                    pace
                });
            }
            QuotaCaptureDesignCase::DegradedOver => {
                report.selection_projection_source =
                    SelectionProjectionSource::DisplayWindowsFallback;
                report.preferred_next_account_id = None;
                for row in &mut report.rows {
                    row.preferred_next = false;
                }
                let selected_row = report
                    .rows
                    .get_mut(0)
                    .unwrap_or_else(|| panic!("capture report should include selected row"));
                selected_row.weekly_pace = selected_row.weekly_pace.map(|mut pace| {
                    pace.projected_candidate_burn_basis_points_per_hour = Some(70);
                    pace.aggregate_burn_basis_points_per_hour = Some(70);
                    pace
                });
            }
            QuotaCaptureDesignCase::UnavailableBurn => {
                let selected_row = report
                    .rows
                    .get_mut(0)
                    .unwrap_or_else(|| panic!("capture report should include selected row"));
                selected_row.weekly_pace = None;
            }
        }
        report
    }

    fn blocked_quota_capture_report() -> QuotaStatusReport {
        QuotaStatusReport {
            app_version: env!("CARGO_PKG_VERSION").to_owned(),
            route_band: "responses".to_owned(),
            selected_pool: SelectedPool::None,
            preferred_next_account_id: None,
            selection_projection_source: SelectionProjectionSource::SqlxProjection,
            now_unix_seconds: NOW,
            rows: vec![
                quota_capture_row(QuotaCaptureRowFixture {
                    account_id_value: "acct_ssdev",
                    account_label: "ssdev",
                    preferred_next: false,
                    short_remaining: 0,
                    weekly_remaining: 0,
                    freshness: QuotaEvidenceFreshness::Fresh,
                    availability: AccountAvailability::Blocked,
                    routing_reason: RoutingReason::BlockedWindowExhausted,
                }),
                quota_capture_row(QuotaCaptureRowFixture {
                    account_id_value: "acct_legacy",
                    account_label: "legacy",
                    preferred_next: false,
                    short_remaining: 0,
                    weekly_remaining: 0,
                    freshness: QuotaEvidenceFreshness::Stale,
                    availability: AccountAvailability::Blocked,
                    routing_reason: RoutingReason::BlockedWindowExhausted,
                }),
            ],
        }
    }

    fn assert_quota_capture_width_contract(width: usize, text: &str) {
        assert!(
            text.lines().all(|line| line.chars().count() <= width),
            "quota capture width {width} overflowed:\n{text}"
        );
        assert!(
            text.contains('╭') && text.contains('╰') && text.contains("  Account"),
            "quota capture should render boxed quota blocks:\n{text}"
        );
        if width == 72 {
            for account_label in ["ssdev", "askluna", "matches", "legacy"] {
                assert!(
                    text.lines().any(|line| {
                        line.contains(account_label)
                            && line.starts_with('│')
                            && !line.contains("responses ->")
                    }),
                    "quota capture should include {account_label}:\n{text}"
                );
                assert!(
                    text.contains("weekly") && text.contains("left, reset"),
                    "quota capture width 72 should preserve weekly reset facts for {account_label}:\n{text}"
                );
            }
            assert!(
                !text.contains("..."),
                "quota capture width 72 should avoid clipping normal account rows:\n{text}"
            );
        }
        if width == 90 {
            for reason in ["safest quota", "same pool", "5h guard", "quota empty"] {
                assert!(
                    text.contains(reason),
                    "quota capture width 90 should preserve readable reasons, missing {reason}:\n{text}"
                );
            }
            assert!(
                !text.contains("..."),
                "quota capture width 90 should not clip table cells:\n{text}"
            );
        }
    }

    fn format_next_use_for_capture(reason: RoutingReason) -> &'static str {
        match reason {
            RoutingReason::PreferredNearResetDrainable
            | RoutingReason::PreferredNearResetControlledDrain
            | RoutingReason::PreferredWeeklyHealthier
            | RoutingReason::PreferredWeeklyResetSoon
            | RoutingReason::PreferredShortResetSoon
            | RoutingReason::PreferredProjectedBurn
            | RoutingReason::PreferredSafestQuota
            | RoutingReason::PreferredLastResortShortWindowGuard => "preferred by quota",
            RoutingReason::AvailableSamePool => "available by quota",
            RoutingReason::HeldReserve
            | RoutingReason::HeldUnknown
            | RoutingReason::HeldShortWindowGuard => "held by quota",
            RoutingReason::UnknownFallbackPreferred | RoutingReason::UnknownFallbackAvailable => {
                "fallback by quota"
            }
            RoutingReason::RetiringNearZero => "retiring",
            RoutingReason::ExcludedDisabled
            | RoutingReason::ExcludedMissingCredential
            | RoutingReason::BlockedWindowExhausted
            | RoutingReason::BlockedWindowIneligible => "blocked",
        }
    }

    fn capture_dir() -> PathBuf {
        let dir = std::env::var_os("CODEX_ROUTER_CAPTURE_DIR").map_or_else(
            || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tmp/ux-proof/production"),
            PathBuf::from,
        );
        must_ok(std::fs::create_dir_all(&dir));
        dir
    }

    fn write_capture_pair_with_svg_text(dir: &Path, name: &str, text: &str, svg_text: &str) {
        must_ok(std::fs::write(dir.join(format!("{name}.txt")), text));
        must_ok(std::fs::write(dir.join(format!("{name}.ansi")), svg_text));
        must_ok(std::fs::write(
            dir.join(format!("{name}.svg")),
            terminal_svg(name, svg_text),
        ));
    }

    fn terminal_svg(title: &str, text: &str) -> String {
        let lines = text.lines().collect::<Vec<_>>();
        let width = lines
            .iter()
            .map(|line| ansi_visible_text(line).chars().count())
            .max()
            .unwrap_or(1);
        let height = lines.len().max(1);
        let pixel_width = width * 9 + 32;
        let pixel_height = height * 18 + 34;
        let mut svg = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{pixel_width}\" height=\"{pixel_height}\" viewBox=\"0 0 {pixel_width} {pixel_height}\"><rect width=\"100%\" height=\"100%\" fill=\"#111318\"/>"
        );
        for (index, line) in lines.iter().enumerate() {
            let selected_background = line.contains("\x1b[48;2;58;70;122m");
            if line.contains('*') || line.contains("[blocked]") || selected_background {
                let y = 36 + index * 18;
                let (x, rect_width) = if selected_background {
                    (
                        34,
                        ((width.saturating_sub(4) as f64) * 8.4).round() as usize,
                    )
                } else {
                    (8, pixel_width.saturating_sub(16))
                };
                svg.push_str(&format!(
                    "<rect x=\"{x}\" y=\"{}\" width=\"{rect_width}\" height=\"18\" fill=\"#2d333b\"/>",
                    y.saturating_sub(14),
                ));
            }
        }
        svg.push_str(&svg_text(16, 24, "#e6edf3", title));
        for (index, line) in lines.iter().enumerate() {
            let y = 44 + index * 18;
            svg.push_str(&svg_line_text(16, y, &ansi_svg_segments(line)));
        }
        svg.push_str("</svg>");
        svg
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    struct SvgTextSegment {
        color: &'static str,
        text: String,
    }

    fn ansi_visible_text(line: &str) -> String {
        ansi_svg_segments(line)
            .into_iter()
            .map(|segment| segment.text)
            .collect::<String>()
    }

    fn ansi_svg_segments(line: &str) -> Vec<SvgTextSegment> {
        let mut segments = Vec::new();
        let mut current_text = String::new();
        let mut current_color = "#e6edf3";
        let mut characters = line.chars().peekable();
        while let Some(character) = characters.next() {
            if character == '\x1b' && characters.peek() == Some(&'[') {
                characters.next();
                let mut sequence = String::new();
                for sequence_character in characters.by_ref() {
                    sequence.push(sequence_character);
                    if sequence_character.is_ascii_alphabetic() {
                        break;
                    }
                }
                if sequence.ends_with('m') {
                    if !current_text.is_empty() {
                        segments.push(SvgTextSegment {
                            color: current_color,
                            text: current_text,
                        });
                        current_text = String::new();
                    }
                    current_color = ansi_sgr_color(&sequence);
                }
                continue;
            }
            current_text.push(character);
        }
        if !current_text.is_empty() {
            segments.push(SvgTextSegment {
                color: current_color,
                text: current_text,
            });
        }
        segments
    }

    fn ansi_sgr_color(sequence: &str) -> &'static str {
        match sequence.trim_end_matches('m') {
            "0" => "#e6edf3",
            "32" => "#7ee787",
            "33" => "#ffe75c",
            "36" => "#8ae8f0",
            "90" => "#8b949e",
            "48;2;58;70;122" => "#e6edf3",
            _ => "#e6edf3",
        }
    }

    fn svg_text(x: usize, y: usize, color: &str, text: &str) -> String {
        format!(
            "<text x=\"{x}\" y=\"{y}\" xml:space=\"preserve\" font-family=\"SFMono-Regular, Menlo, Consolas, monospace\" font-size=\"14\" fill=\"{color}\">{}</text>",
            escape_xml(text)
        )
    }

    fn svg_line_text(x: usize, y: usize, segments: &[SvgTextSegment]) -> String {
        let mut text = format!(
            "<text x=\"{x}\" y=\"{y}\" xml:space=\"preserve\" font-family=\"SFMono-Regular, Menlo, Consolas, monospace\" font-size=\"14\">"
        );
        for segment in segments {
            text.push_str(&format!(
                "<tspan fill=\"{}\">{}</tspan>",
                segment.color,
                escape_xml(&segment.text)
            ));
        }
        text.push_str("</text>");
        text
    }

    fn escape_xml(value: &str) -> String {
        value
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    }

    fn must_ok<T, E: std::fmt::Display>(result: Result<T, E>) -> T {
        match result {
            Ok(value) => value,
            Err(error) => panic!("expected Ok, got error: {error}"),
        }
    }

    fn account(account_id: &str, label: &str) -> AccountRecord {
        AccountRecord::new(test_account_id(account_id), label, AccountStatus::Enabled)
            .with_active_credential_generation(1)
    }

    fn account_id(value: &str) -> AccountId {
        test_account_id(value)
    }

    fn test_account_id(value: &str) -> AccountId {
        match AccountId::new(value) {
            Ok(account_id) => account_id,
            Err(error) => panic!("test account id is valid: {error}"),
        }
    }

    fn display_window(
        window_seconds: u64,
        remaining_headroom: u32,
        reset_unix_seconds: u64,
        run_rate_estimate: QuotaRunRateEstimate,
    ) -> DisplayQuotaWindow {
        DisplayQuotaWindow {
            window_seconds,
            status: QuotaWindowStatus::Eligible,
            remaining_headroom,
            reset_unix_seconds: Some(reset_unix_seconds),
            observed_unix_seconds: NOW,
            effective: window_seconds == V1_SHORT_WINDOW_SECONDS,
            run_rate_estimate,
        }
    }
}

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

fn current_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

mod background_refresh_worker;
mod refresh_command;
mod refresh_history;
mod refresh_provider;
mod refresh_service;

pub(crate) use background_refresh_worker::*;
pub(crate) use refresh_command::*;
use refresh_history::*;
pub(crate) use refresh_provider::*;
pub(crate) use refresh_service::*;

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

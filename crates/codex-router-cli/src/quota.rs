//! Quota status commands backed by router-owned SQLite state.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use codex_router_auth::live_quota::AdditionalRateLimit;
use codex_router_auth::live_quota::DEFAULT_CHATGPT_BACKEND_BASE_URL;
use codex_router_auth::live_quota::LiveQuotaClient;
use codex_router_auth::live_quota::LiveQuotaError;
use codex_router_auth::live_quota::QuotaEndpointPolicy;
use codex_router_auth::live_quota::UsageAuth;
use codex_router_auth::live_quota::UsageResponse;
use codex_router_auth::live_quota::UsageWindow;
use codex_router_auth::live_quota::WindowPair;
use codex_router_auth::live_quota::usage_window_remaining_percent;
use codex_router_core::ids::AccountId;
use codex_router_secret_store::account_tokens::upstream_access_token_key;
use codex_router_secret_store::file_backend::FileSecretStore;
use codex_router_secret_store::file_backend::SecretStore;
use codex_router_secret_store::model::SecretStoreError;
use codex_router_state::account::AccountRecord;
use codex_router_state::account::AccountStatus;
use codex_router_state::quota_snapshot::PersistedQuotaSnapshot;
use codex_router_state::quota_snapshot::PersistedQuotaStatusRow;
use codex_router_state::quota_snapshot::QuotaSnapshotSource;
use codex_router_state::quota_snapshot::QuotaStatusState;
use codex_router_state::repositories::AccountStateRepository;
use codex_router_state::repositories::QuotaStatusRepository;
use codex_router_state::sqlite::SqliteStateStore;
use codex_router_state::sqlite::StateStoreError;
use comfy_table::Table;
use comfy_table::presets::ASCII_MARKDOWN;
use thiserror::Error;

use crate::ArgumentParser;
use crate::CliError;
use crate::RouterRootPaths;
use crate::current_unix_seconds;
use crate::parse_u64_option;

const DEFAULT_QUOTA_STATUS_MAX_AGE_SECONDS: u64 = 300;

/// Returns the default provider quota base URL.
pub(crate) fn default_quota_base_url() -> String {
    DEFAULT_CHATGPT_BACKEND_BASE_URL.to_owned()
}

/// Quota command namespace.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum QuotaCommand {
    /// Render persisted quota status.
    Status(QuotaStatusCommand),
    /// Refresh persisted quota status from the provider.
    Refresh(QuotaRefreshCommand),
}

impl QuotaCommand {
    pub(crate) fn parse(parser: &mut ArgumentParser) -> Result<Self, CliError> {
        let Some(command) = parser.next_string()? else {
            return Err(CliError::MissingCommand {
                command: "quota".to_owned(),
            });
        };

        match command.as_str() {
            "status" => Ok(Self::Status(QuotaStatusCommand::parse(parser)?)),
            "refresh" => Ok(Self::Refresh(QuotaRefreshCommand::parse(parser)?)),
            unknown => Err(CliError::UnknownCommand {
                command: format!("quota {unknown}"),
            }),
        }
    }
}

/// `quota refresh` command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct QuotaRefreshCommand {
    router_root: PathBuf,
    account: Option<String>,
    base_url: String,
    allow_insecure_quota_base_url: bool,
    timeout_seconds: u64,
    now_unix_seconds: u64,
}

/// Runtime config for quota refresh, shared by manual and background refresh.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct QuotaRefreshRunConfig {
    /// Router-owned root.
    pub(crate) router_root: PathBuf,
    /// Optional account selector.
    pub(crate) account: Option<String>,
    /// Provider quota base URL.
    pub(crate) base_url: String,
    /// Whether loopback HTTP quota endpoints are allowed for local tests.
    pub(crate) allow_insecure_quota_base_url: bool,
    /// Provider request timeout.
    pub(crate) timeout_seconds: u64,
    /// Observation timestamp.
    pub(crate) now_unix_seconds: u64,
}

impl QuotaRefreshCommand {
    fn parse(parser: &mut ArgumentParser) -> Result<Self, CliError> {
        let mut router_root = None;
        let mut account = None;
        let mut base_url = DEFAULT_CHATGPT_BACKEND_BASE_URL.to_owned();
        let mut allow_insecure_quota_base_url = false;
        let mut timeout_seconds = 30_u64;
        let mut now_unix_seconds = None;

        while let Some(argument) = parser.next_string()? {
            match argument.as_str() {
                "--router-root" => {
                    router_root = Some(PathBuf::from(parser.next_required_value("--router-root")?));
                }
                "--account" => {
                    account = Some(parser.next_required_value("--account")?);
                }
                "--base-url" => {
                    base_url = parser.next_required_value("--base-url")?;
                }
                "--allow-insecure-quota-base-url" => {
                    allow_insecure_quota_base_url = true;
                }
                "--timeout-seconds" => {
                    let value = parser.next_required_value("--timeout-seconds")?;
                    timeout_seconds = parse_u64_option("--timeout-seconds", &value)?;
                }
                "--now-unix-seconds" => {
                    let value = parser.next_required_value("--now-unix-seconds")?;
                    now_unix_seconds = Some(parse_u64_option("--now-unix-seconds", &value)?);
                }
                unknown => {
                    return Err(CliError::UnknownOption {
                        option: unknown.to_owned(),
                    });
                }
            }
        }

        Ok(Self {
            router_root: router_root.ok_or(CliError::MissingOption {
                option: "--router-root",
            })?,
            account,
            base_url,
            allow_insecure_quota_base_url,
            timeout_seconds,
            now_unix_seconds: now_unix_seconds.map_or_else(current_unix_seconds, Ok)?,
        })
    }
}

/// `quota status` command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct QuotaStatusCommand {
    router_root: PathBuf,
    output_format: QuotaStatusOutputFormat,
    all_limits: bool,
    now_unix_seconds: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum QuotaStatusOutputFormat {
    #[default]
    Table,
    Plain,
}

impl QuotaStatusCommand {
    fn parse(parser: &mut ArgumentParser) -> Result<Self, CliError> {
        let mut router_root = None;
        let mut output_format = QuotaStatusOutputFormat::Table;
        let mut all_limits = false;
        let mut now_unix_seconds = None;

        while let Some(argument) = parser.next_string()? {
            match argument.as_str() {
                "--router-root" => {
                    router_root = Some(PathBuf::from(parser.next_required_value("--router-root")?));
                }
                "--all-limits" => {
                    all_limits = true;
                }
                "--format" => {
                    let value = parser.next_required_value("--format")?;
                    output_format = parse_quota_status_output_format(value.as_str())?;
                }
                "--now-unix-seconds" => {
                    let value = parser.next_required_value("--now-unix-seconds")?;
                    now_unix_seconds = Some(parse_u64_option("--now-unix-seconds", &value)?);
                }
                unknown => {
                    return Err(CliError::UnknownOption {
                        option: unknown.to_owned(),
                    });
                }
            }
        }

        Ok(Self {
            router_root: router_root.ok_or(CliError::MissingOption {
                option: "--router-root",
            })?,
            output_format,
            all_limits,
            now_unix_seconds: now_unix_seconds.map_or_else(current_unix_seconds, Ok)?,
        })
    }
}

fn parse_quota_status_output_format(value: &str) -> Result<QuotaStatusOutputFormat, CliError> {
    match value {
        "table" => Ok(QuotaStatusOutputFormat::Table),
        "plain" => Ok(QuotaStatusOutputFormat::Plain),
        "json" => Err(CliError::UnknownOption {
            option: "--format json is not implemented for quota status".to_owned(),
        }),
        unknown => Err(CliError::UnknownOption {
            option: format!("--format {unknown}"),
        }),
    }
}

/// Quota command failure.
#[derive(Debug, Error)]
pub enum QuotaCommandError {
    /// State store failed.
    #[error(transparent)]
    State(#[from] StateStoreError),
    /// Secret store failed.
    #[error(transparent)]
    SecretStore(#[from] SecretStoreError),
    /// Live quota request failed.
    #[error(transparent)]
    LiveQuota(#[from] LiveQuotaError),
    /// Account selector matched no enabled account.
    #[error("enabled account not found")]
    AccountNotFound,
    /// Account selector matched more than one enabled account.
    #[error("account selector is ambiguous")]
    AmbiguousAccount,
    /// One or more account refreshes failed after visible failure rows were persisted.
    #[error("quota refresh failed for {failed_accounts} account(s); run quota status for details")]
    RefreshFailed {
        /// Failed account count.
        failed_accounts: usize,
    },
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
        QuotaCommand::Status(command) => status(stdout, command),
        QuotaCommand::Refresh(command) => refresh(stdout, command.into_run_config()),
    }
}

impl QuotaRefreshCommand {
    fn into_run_config(self) -> QuotaRefreshRunConfig {
        QuotaRefreshRunConfig {
            router_root: self.router_root,
            account: self.account,
            base_url: self.base_url,
            allow_insecure_quota_base_url: self.allow_insecure_quota_base_url,
            timeout_seconds: self.timeout_seconds,
            now_unix_seconds: self.now_unix_seconds,
        }
    }
}

/// Refreshes router-owned quota state.
pub(crate) fn refresh_quota_state(
    stdout: &mut impl Write,
    config: QuotaRefreshRunConfig,
) -> Result<(), QuotaCommandError> {
    refresh(stdout, config)
}

/// Validates refresh endpoint policy without performing provider I/O.
pub(crate) fn validate_quota_refresh_endpoint(
    base_url: &str,
    allow_insecure_quota_base_url: bool,
    timeout_seconds: u64,
) -> Result<(), QuotaCommandError> {
    let endpoint_policy = quota_endpoint_policy(allow_insecure_quota_base_url);
    let _client = LiveQuotaClient::new_with_timeout_and_policy(
        base_url.to_owned(),
        Some(Duration::from_secs(timeout_seconds)),
        endpoint_policy,
    )?;
    Ok(())
}

fn refresh(
    stdout: &mut impl Write,
    config: QuotaRefreshRunConfig,
) -> Result<(), QuotaCommandError> {
    let paths = RouterRootPaths::new(config.router_root);
    let state_store = SqliteStateStore::open(&paths.state_db)?;
    let secret_store = FileSecretStore::open(&paths.secret_root)?;
    let accounts = refresh_accounts(&state_store, config.account.as_deref())?;
    let endpoint_policy = quota_endpoint_policy(config.allow_insecure_quota_base_url);
    let client = LiveQuotaClient::new_with_timeout_and_policy(
        config.base_url,
        Some(Duration::from_secs(config.timeout_seconds)),
        endpoint_policy,
    )?;

    let mut failed_accounts = 0_usize;
    for account in accounts {
        let access_key = upstream_access_token_key(account.account_id())?;
        let access_token = match secret_store.read_secret(&access_key) {
            Ok(access_token) => access_token,
            Err(_error) => {
                failed_accounts += 1;
                persist_account_refresh_failure(
                    &state_store,
                    account.account_id(),
                    config.now_unix_seconds,
                    "credential unavailable",
                )?;
                writeln!(
                    stdout,
                    "failed: {} reason=credential-unavailable",
                    account.account_id().as_str()
                )
                .map_err(QuotaCommandError::Stdout)?;
                continue;
            }
        };
        let auth = match UsageAuth::from_access_token(access_token.expose_secret().to_owned()) {
            Ok(auth) => auth,
            Err(_error) => {
                failed_accounts += 1;
                persist_account_refresh_failure(
                    &state_store,
                    account.account_id(),
                    config.now_unix_seconds,
                    "credential unavailable",
                )?;
                writeln!(
                    stdout,
                    "failed: {} reason=credential-unavailable",
                    account.account_id().as_str()
                )
                .map_err(QuotaCommandError::Stdout)?;
                continue;
            }
        };
        let usage = match client.fetch(&auth) {
            Ok(usage) => usage,
            Err(_error) => {
                failed_accounts += 1;
                persist_account_refresh_failure(
                    &state_store,
                    account.account_id(),
                    config.now_unix_seconds,
                    "provider quota refresh failed",
                )?;
                writeln!(
                    stdout,
                    "failed: {} reason=provider-quota-refresh-failed",
                    account.account_id().as_str()
                )
                .map_err(QuotaCommandError::Stdout)?;
                continue;
            }
        };
        let refresh_result =
            normalize_usage_response(account.account_id(), &usage, config.now_unix_seconds);
        let mut account_had_failed_route = false;
        for route in refresh_result.routes {
            if route
                .status_rows
                .iter()
                .any(|row| row.status() == QuotaStatusState::Failed)
            {
                account_had_failed_route = true;
            }
            QuotaStatusRepository::replace_route_quota_state(
                &state_store,
                &route.snapshot,
                &route.status_rows,
            )?;
            writeln!(
                stdout,
                "refreshed: {} route={} rows={}",
                account.account_id().as_str(),
                route.snapshot.route_band(),
                route.status_rows.len()
            )
            .map_err(QuotaCommandError::Stdout)?;
        }
        if account_had_failed_route {
            failed_accounts += 1;
            writeln!(
                stdout,
                "failed: {} reason=provider-quota-refresh-failed",
                account.account_id().as_str()
            )
            .map_err(QuotaCommandError::Stdout)?;
        }
    }

    if failed_accounts > 0 {
        return Err(QuotaCommandError::RefreshFailed { failed_accounts });
    }

    Ok(())
}

fn persist_account_refresh_failure(
    state_store: &SqliteStateStore,
    account_id: &AccountId,
    observed_unix_seconds: u64,
    failure_message: &str,
) -> Result<(), StateStoreError> {
    for route_band in std::iter::once("responses")
        .chain(RESPONSE_QUOTA_ROUTE_ALIASES.iter().copied())
        .chain(std::iter::once("code_review"))
    {
        let route = failed_route_quota(
            account_id,
            route_band,
            "refresh",
            failure_message,
            observed_unix_seconds,
        );
        QuotaStatusRepository::replace_route_quota_state(
            state_store,
            &route.snapshot,
            &route.status_rows,
        )?;
    }

    Ok(())
}

fn refresh_accounts(
    state_store: &SqliteStateStore,
    selector: Option<&str>,
) -> Result<Vec<AccountRecord>, QuotaCommandError> {
    let enabled_accounts: Vec<AccountRecord> = AccountStateRepository::list_accounts(state_store)?
        .into_iter()
        .filter(|account| account.status() == AccountStatus::Enabled)
        .collect();
    let Some(selector) = selector else {
        return Ok(enabled_accounts);
    };

    let matches: Vec<AccountRecord> = enabled_accounts
        .into_iter()
        .filter(|account| account.account_id().as_str() == selector || account.label() == selector)
        .collect();
    match matches.as_slice() {
        [] => Err(QuotaCommandError::AccountNotFound),
        [account] => Ok(vec![account.clone()]),
        _ => Err(QuotaCommandError::AmbiguousAccount),
    }
}

fn status(stdout: &mut impl Write, command: QuotaStatusCommand) -> Result<(), QuotaCommandError> {
    let paths = RouterRootPaths::new(command.router_root);
    let state_store = SqliteStateStore::open_existing_read_only(&paths.state_db)?;
    let accounts = AccountStateRepository::list_accounts(&state_store)?;
    let labels = account_labels_by_id(&accounts);
    let rows = QuotaStatusRepository::list_status_rows(&state_store)?;
    let rows = status_rows_with_unknown_accounts(&accounts, &rows, command.now_unix_seconds);
    let rendered_rows = visible_status_rows(&rows, command.all_limits);
    match command.output_format {
        QuotaStatusOutputFormat::Table => {
            let table =
                render_quota_status_table(&rendered_rows, &labels, command.now_unix_seconds);
            writeln!(stdout, "{table}").map_err(QuotaCommandError::Stdout)
        }
        QuotaStatusOutputFormat::Plain => {
            write_quota_status_plain(stdout, &rendered_rows, &labels, command.now_unix_seconds)
                .map_err(QuotaCommandError::Stdout)
        }
    }
}

struct NormalizedRefreshResult {
    routes: Vec<NormalizedRouteQuota>,
}

struct NormalizedRouteQuota {
    snapshot: PersistedQuotaSnapshot,
    status_rows: Vec<PersistedQuotaStatusRow>,
}

fn normalize_usage_response(
    account_id: &AccountId,
    usage: &UsageResponse,
    observed_unix_seconds: u64,
) -> NormalizedRefreshResult {
    let mut routes = Vec::new();
    if let Some(mut route) = normalize_window_pair(
        account_id,
        "responses",
        "rate_limit",
        usage.rate_limit.as_ref(),
        observed_unix_seconds,
    ) {
        for additional in &usage.additional_rate_limits {
            append_additional_rows(
                account_id,
                "responses",
                additional,
                observed_unix_seconds,
                &mut route.status_rows,
            );
        }
        for route_band in RESPONSE_QUOTA_ROUTE_ALIASES {
            routes.push(alias_route_quota(&route, route_band));
        }
        routes.push(route);
    } else {
        let route = failed_route_quota(
            account_id,
            "responses",
            "rate_limit",
            "provider quota missing usable windows",
            observed_unix_seconds,
        );
        for route_band in RESPONSE_QUOTA_ROUTE_ALIASES {
            routes.push(alias_route_quota(&route, route_band));
        }
        routes.push(route);
    }
    if let Some(route) = normalize_window_pair(
        account_id,
        "code_review",
        "code_review",
        usage.code_review_rate_limit.as_ref(),
        observed_unix_seconds,
    ) {
        routes.push(route);
    } else {
        routes.push(failed_route_quota(
            account_id,
            "code_review",
            "code_review",
            "provider quota missing usable windows",
            observed_unix_seconds,
        ));
    }

    NormalizedRefreshResult { routes }
}

const RESPONSE_QUOTA_ROUTE_ALIASES: &[&str] =
    &["models", "memories_trace_summarize", "responses_compact"];

fn quota_endpoint_policy(allow_insecure_quota_base_url: bool) -> QuotaEndpointPolicy {
    if allow_insecure_quota_base_url {
        QuotaEndpointPolicy::AllowLoopbackForTesting
    } else {
        QuotaEndpointPolicy::ProviderOnly
    }
}

fn alias_route_quota(route: &NormalizedRouteQuota, route_band: &str) -> NormalizedRouteQuota {
    let mut snapshot =
        PersistedQuotaSnapshot::new(route.snapshot.account_id().clone(), route.snapshot.source())
            .with_observed_unix_seconds(route.snapshot.observed_unix_seconds())
            .with_route_band(route_band, route.snapshot.remaining_headroom())
            .with_stale_penalty(route.snapshot.stale_penalty());
    if let Some(reset_unix_seconds) = route.snapshot.reset_unix_seconds() {
        snapshot = snapshot.with_reset_unix_seconds(reset_unix_seconds);
    }

    let status_rows = route
        .status_rows
        .iter()
        .map(|row| clone_status_row_for_route_band(row, route_band))
        .collect();

    NormalizedRouteQuota {
        snapshot,
        status_rows,
    }
}

fn clone_status_row_for_route_band(
    row: &PersistedQuotaStatusRow,
    route_band: &str,
) -> PersistedQuotaStatusRow {
    let mut cloned = PersistedQuotaStatusRow::new(
        row.account_id().clone(),
        row.source(),
        route_band,
        row.family(),
        row.window_label(),
    )
    .with_observed_unix_seconds(row.observed_unix_seconds())
    .with_status(row.status())
    .with_remaining_headroom(row.remaining_headroom())
    .with_effective(row.effective());
    if let Some(used_percent) = row.used_percent() {
        cloned = cloned.with_used_percent(used_percent);
    }
    if let Some(reset_unix_seconds) = row.reset_unix_seconds() {
        cloned = cloned.with_reset_unix_seconds(reset_unix_seconds);
    }
    if let Some(limit_window_seconds) = row.limit_window_seconds() {
        cloned = cloned.with_limit_window_seconds(limit_window_seconds);
    }
    if let Some(failure_message) = row.failure_message() {
        cloned = cloned.with_failure(
            failure_message,
            row.failure_unix_seconds()
                .unwrap_or(row.observed_unix_seconds()),
        );
    }
    cloned
}

fn failed_route_quota(
    account_id: &AccountId,
    route_band: &str,
    family: &str,
    failure_message: &str,
    observed_unix_seconds: u64,
) -> NormalizedRouteQuota {
    let snapshot =
        PersistedQuotaSnapshot::new(account_id.clone(), QuotaSnapshotSource::OpenAiEndpoint)
            .with_observed_unix_seconds(observed_unix_seconds)
            .with_route_band(route_band, 0)
            .with_stale_penalty(true);
    let row = PersistedQuotaStatusRow::new(
        account_id.clone(),
        QuotaSnapshotSource::OpenAiEndpoint,
        route_band,
        family,
        "failure",
    )
    .with_observed_unix_seconds(observed_unix_seconds)
    .with_status(QuotaStatusState::Failed)
    .with_remaining_headroom(0)
    .with_effective(true)
    .with_failure(failure_message, observed_unix_seconds);

    NormalizedRouteQuota {
        snapshot,
        status_rows: vec![row],
    }
}

pub(crate) fn failed_status_row(
    account_id: &AccountId,
    route_band: &str,
    family: &str,
    failure_message: &str,
    observed_unix_seconds: u64,
) -> PersistedQuotaStatusRow {
    PersistedQuotaStatusRow::new(
        account_id.clone(),
        QuotaSnapshotSource::OpenAiEndpoint,
        route_band,
        family,
        "failure",
    )
    .with_observed_unix_seconds(observed_unix_seconds)
    .with_status(QuotaStatusState::Failed)
    .with_remaining_headroom(0)
    .with_effective(true)
    .with_failure(failure_message, observed_unix_seconds)
}

pub(crate) fn status_rows_from_usage_response(
    account_id: &AccountId,
    usage: &UsageResponse,
    observed_unix_seconds: u64,
) -> Vec<PersistedQuotaStatusRow> {
    normalize_usage_response(account_id, usage, observed_unix_seconds)
        .routes
        .into_iter()
        .flat_map(|route| route.status_rows)
        .collect()
}

fn normalize_window_pair(
    account_id: &AccountId,
    route_band: &str,
    family: &str,
    pair: Option<&WindowPair>,
    observed_unix_seconds: u64,
) -> Option<NormalizedRouteQuota> {
    let pair = pair?;
    let bottleneck_window = bottleneck_window_for_routing(pair, observed_unix_seconds);
    let remaining_headroom = bottleneck_window
        .and_then(remaining_headroom_for_routing_window)
        .unwrap_or(0);
    let mut status_rows = Vec::new();
    append_window_row(
        account_id,
        route_band,
        family,
        pair.primary_window.as_ref(),
        observed_unix_seconds,
        false,
        &mut status_rows,
    );
    append_window_row(
        account_id,
        route_band,
        family,
        pair.secondary_window.as_ref(),
        observed_unix_seconds,
        false,
        &mut status_rows,
    );
    append_window_row(
        account_id,
        route_band,
        family,
        bottleneck_window,
        observed_unix_seconds,
        true,
        &mut status_rows,
    );
    if bottleneck_window.is_none() {
        status_rows.push(
            PersistedQuotaStatusRow::new(
                account_id.clone(),
                QuotaSnapshotSource::OpenAiEndpoint,
                route_band,
                family,
                "effective",
            )
            .with_observed_unix_seconds(observed_unix_seconds)
            .with_status(QuotaStatusState::Failed)
            .with_remaining_headroom(0)
            .with_effective(true)
            .with_failure(
                "provider quota missing usable windows",
                observed_unix_seconds,
            ),
        );
    }

    let mut snapshot =
        PersistedQuotaSnapshot::new(account_id.clone(), QuotaSnapshotSource::OpenAiEndpoint)
            .with_observed_unix_seconds(observed_unix_seconds)
            .with_route_band(route_band, remaining_headroom)
            .with_stale_penalty(remaining_headroom == 0);
    if let Some(reset_at) = bottleneck_window
        .and_then(|window| window.reset_at)
        .and_then(|reset_at| u64::try_from(reset_at).ok())
    {
        snapshot = snapshot.with_reset_unix_seconds(reset_at);
    }

    Some(NormalizedRouteQuota {
        snapshot,
        status_rows,
    })
}

fn append_additional_rows(
    account_id: &AccountId,
    route_band: &str,
    additional: &AdditionalRateLimit,
    observed_unix_seconds: u64,
    status_rows: &mut Vec<PersistedQuotaStatusRow>,
) {
    let family = additional_limit_label(additional);
    let Some(pair) = additional.rate_limit.as_ref() else {
        return;
    };
    append_window_row(
        account_id,
        route_band,
        &family,
        pair.primary_window.as_ref(),
        observed_unix_seconds,
        false,
        status_rows,
    );
    append_window_row(
        account_id,
        route_band,
        &family,
        pair.secondary_window.as_ref(),
        observed_unix_seconds,
        false,
        status_rows,
    );
}

fn append_window_row(
    account_id: &AccountId,
    route_band: &str,
    family: &str,
    window: Option<&UsageWindow>,
    observed_unix_seconds: u64,
    effective: bool,
    status_rows: &mut Vec<PersistedQuotaStatusRow>,
) {
    let Some(window) = window else {
        return;
    };
    let used_percent = window.used_percent.and_then(valid_percent_i64_to_u32);
    let window_is_usable = usable_routing_window(window, observed_unix_seconds);
    let remaining_headroom = if window_is_usable {
        remaining_headroom_for_routing_window(window).unwrap_or(0)
    } else {
        0
    };
    let mut row = PersistedQuotaStatusRow::new(
        account_id.clone(),
        QuotaSnapshotSource::OpenAiEndpoint,
        route_band,
        family,
        window_label(window),
    )
    .with_observed_unix_seconds(observed_unix_seconds)
    .with_status(if window_is_usable {
        QuotaStatusState::Fresh
    } else {
        QuotaStatusState::Failed
    })
    .with_remaining_headroom(remaining_headroom)
    .with_effective(effective);
    if let Some(used_percent) = used_percent {
        row = row.with_used_percent(used_percent);
    }
    if let Some(reset_at) = window.reset_at.and_then(|value| u64::try_from(value).ok()) {
        row = row.with_reset_unix_seconds(reset_at);
    }
    if let Some(limit_window_seconds) = window
        .limit_window_seconds
        .and_then(|value| u64::try_from(value).ok())
    {
        row = row.with_limit_window_seconds(limit_window_seconds);
    }
    if !window_is_usable {
        row = row.with_failure("provider quota window invalid", observed_unix_seconds);
    }
    status_rows.push(row);
}

fn bottleneck_window_for_routing(
    pair: &WindowPair,
    observed_unix_seconds: u64,
) -> Option<&UsageWindow> {
    let primary = pair
        .primary_window
        .as_ref()
        .filter(|window| usable_routing_window(window, observed_unix_seconds));
    let secondary = pair
        .secondary_window
        .as_ref()
        .filter(|window| usable_routing_window(window, observed_unix_seconds));
    match (primary, secondary) {
        (Some(primary), Some(secondary)) => {
            let primary_remaining = remaining_headroom_for_routing_window(primary)?;
            let secondary_remaining = remaining_headroom_for_routing_window(secondary)?;
            if primary_remaining < secondary_remaining {
                Some(primary)
            } else if secondary_remaining < primary_remaining {
                Some(secondary)
            } else {
                earliest_reset_window(primary, secondary)
            }
        }
        (Some(primary), None) => Some(primary),
        (None, Some(secondary)) => Some(secondary),
        (None, None) => None,
    }
}

fn usable_routing_window(window: &UsageWindow, observed_unix_seconds: u64) -> bool {
    window
        .used_percent
        .and_then(valid_percent_i64_to_u32)
        .is_some()
        && window
            .reset_at
            .and_then(|reset_at| u64::try_from(reset_at).ok())
            .is_some_and(|reset_at| reset_at > observed_unix_seconds)
        && window
            .limit_window_seconds
            .and_then(|seconds| u64::try_from(seconds).ok())
            .is_some_and(|seconds| seconds > 0)
}

fn remaining_headroom_for_routing_window(window: &UsageWindow) -> Option<u32> {
    usage_window_remaining_percent(Some(window)).and_then(valid_percent_i64_to_u32)
}

fn earliest_reset_window<'a>(
    left: &'a UsageWindow,
    right: &'a UsageWindow,
) -> Option<&'a UsageWindow> {
    match (left.reset_at, right.reset_at) {
        (Some(left_reset), Some(right_reset)) if right_reset < left_reset => Some(right),
        (Some(_), Some(_)) | (Some(_), None) | (None, None) => Some(left),
        (None, Some(_)) => Some(right),
    }
}

fn valid_percent_i64_to_u32(value: i64) -> Option<u32> {
    let parsed = u32::try_from(value).ok()?;
    if parsed <= 100 { Some(parsed) } else { None }
}

fn additional_limit_label(additional: &AdditionalRateLimit) -> String {
    additional
        .limit_name
        .as_deref()
        .or(additional.metered_feature.as_deref())
        .map(sanitize_label)
        .unwrap_or_else(|| "additional".to_owned())
}

fn sanitize_label(label: &str) -> String {
    let sanitized = label
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    sanitized.chars().take(64).collect()
}

fn window_label(window: &UsageWindow) -> String {
    match window.limit_window_seconds {
        Some(18_000) => "5h".to_owned(),
        Some(86_400) => "daily".to_owned(),
        Some(604_800) => "weekly".to_owned(),
        Some(2_592_000) => "monthly".to_owned(),
        Some(seconds) if seconds > 0 => format!("{seconds}s"),
        _ => "window".to_owned(),
    }
}

fn account_labels_by_id(accounts: &[AccountRecord]) -> BTreeMap<String, String> {
    accounts
        .iter()
        .map(|account| {
            (
                account.account_id().as_str().to_owned(),
                account.label().to_owned(),
            )
        })
        .collect()
}

fn status_rows_with_unknown_accounts(
    accounts: &[AccountRecord],
    rows: &[PersistedQuotaStatusRow],
    now_unix_seconds: u64,
) -> Vec<PersistedQuotaStatusRow> {
    let mut expanded_rows = rows.to_vec();
    for account in accounts {
        if account.status() != AccountStatus::Enabled {
            continue;
        }
        if rows
            .iter()
            .any(|row| row.account_id() == account.account_id())
        {
            continue;
        }
        expanded_rows.push(
            PersistedQuotaStatusRow::new(
                account.account_id().clone(),
                QuotaSnapshotSource::OpenAiEndpoint,
                "responses",
                "refresh",
                "unknown",
            )
            .with_observed_unix_seconds(now_unix_seconds)
            .with_status(QuotaStatusState::Unknown)
            .with_remaining_headroom(0)
            .with_effective(true)
            .with_failure("not refreshed", now_unix_seconds),
        );
    }

    expanded_rows
}

pub(crate) fn visible_status_rows(
    rows: &[PersistedQuotaStatusRow],
    all_limits: bool,
) -> Vec<PersistedQuotaStatusRow> {
    if all_limits {
        return rows.to_vec();
    }

    let effective_rows: Vec<PersistedQuotaStatusRow> =
        rows.iter().filter(|row| row.effective()).cloned().collect();
    if effective_rows.is_empty() {
        rows.to_vec()
    } else {
        effective_rows
    }
}

pub(crate) fn render_quota_status_table(
    rows: &[PersistedQuotaStatusRow],
    labels: &BTreeMap<String, String>,
    now_unix_seconds: u64,
) -> Table {
    let mut table = Table::new();
    table.load_preset(ASCII_MARKDOWN);
    table.set_header([
        "Account", "Route", "Status", "Headroom", "Window", "Reset", "Pace", "Runout", "Notes",
    ]);

    for row in rows {
        table.add_row([
            account_label(row, labels),
            row.route_band().to_owned(),
            status_label(display_status(row, now_unix_seconds)).to_owned(),
            format_headroom(row.remaining_headroom()),
            row.window_label().to_owned(),
            format_reset(row.reset_unix_seconds(), now_unix_seconds),
            format_pace(row, now_unix_seconds),
            format_runout(row, now_unix_seconds),
            format_notes(row),
        ]);
    }

    table
}

pub(crate) fn write_quota_status_plain(
    stdout: &mut impl Write,
    rows: &[PersistedQuotaStatusRow],
    labels: &BTreeMap<String, String>,
    now_unix_seconds: u64,
) -> Result<(), std::io::Error> {
    for row in rows {
        writeln!(
            stdout,
            "account={} route={} status={} headroom={} window={} reset={} pace={} runout={} notes={}",
            encode_plain_value(account_label(row, labels).as_str()),
            encode_plain_value(row.route_band()),
            encode_plain_value(status_label(display_status(row, now_unix_seconds))),
            row.remaining_headroom(),
            encode_plain_value(row.window_label()),
            encode_plain_value(format_reset(row.reset_unix_seconds(), now_unix_seconds).as_str()),
            encode_plain_value(format_pace(row, now_unix_seconds).as_str()),
            encode_plain_value(format_runout(row, now_unix_seconds).as_str()),
            encode_plain_value(format_notes(row).as_str()),
        )?;
    }

    Ok(())
}

fn encode_plain_value(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push(hex_digit(byte >> 4));
            encoded.push(hex_digit(byte & 0x0f));
        }
    }
    encoded
}

fn hex_digit(value: u8) -> char {
    match value {
        0..=9 => char::from(b'0' + value),
        10..=15 => char::from(b'A' + value - 10),
        _ => panic!("hex digit nibble must be <= 15"),
    }
}

fn account_label(row: &PersistedQuotaStatusRow, labels: &BTreeMap<String, String>) -> String {
    let label = labels
        .get(row.account_id().as_str())
        .cloned()
        .unwrap_or_else(|| row.account_id().as_str().to_owned());
    sanitize_display_text(label.as_str())
}

fn sanitize_display_text(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

fn status_label(status: QuotaStatusState) -> &'static str {
    match status {
        QuotaStatusState::Unknown => "unknown",
        QuotaStatusState::Fresh => "fresh",
        QuotaStatusState::Stale => "stale",
        QuotaStatusState::Failed => "failed",
    }
}

fn display_status(row: &PersistedQuotaStatusRow, now_unix_seconds: u64) -> QuotaStatusState {
    match row.status() {
        QuotaStatusState::Fresh if row.observed_unix_seconds() > now_unix_seconds => {
            QuotaStatusState::Unknown
        }
        QuotaStatusState::Fresh
            if row
                .observed_unix_seconds()
                .saturating_add(DEFAULT_QUOTA_STATUS_MAX_AGE_SECONDS)
                < now_unix_seconds =>
        {
            QuotaStatusState::Stale
        }
        status => status,
    }
}

fn format_headroom(remaining_headroom: u32) -> String {
    let filled = remaining_headroom.min(100) / 10;
    let empty = 10_u32.saturating_sub(filled);
    format!(
        "{}% [{}{}]",
        remaining_headroom.min(100),
        "#".repeat(filled as usize),
        "-".repeat(empty as usize)
    )
}

fn format_reset(reset_unix_seconds: Option<u64>, now_unix_seconds: u64) -> String {
    let Some(reset_unix_seconds) = reset_unix_seconds else {
        return "unknown".to_owned();
    };
    format_duration_from_now(reset_unix_seconds as i64 - now_unix_seconds as i64)
}

fn format_pace(row: &PersistedQuotaStatusRow, now_unix_seconds: u64) -> String {
    let Some(delta) = pace_delta_percent(row, now_unix_seconds) else {
        return "unknown".to_owned();
    };

    match delta.cmp(&0) {
        std::cmp::Ordering::Greater => format!("burn +{delta}%"),
        std::cmp::Ordering::Less => format!("save {}%", delta.saturating_abs()),
        std::cmp::Ordering::Equal => "steady".to_owned(),
    }
}

fn format_runout(row: &PersistedQuotaStatusRow, now_unix_seconds: u64) -> String {
    let (Some(used_percent), Some(reset_at), Some(limit_window_seconds)) = (
        row.used_percent(),
        row.reset_unix_seconds(),
        row.limit_window_seconds(),
    ) else {
        return "unknown".to_owned();
    };
    if limit_window_seconds == 0 {
        return "unknown".to_owned();
    }
    if used_percent >= 100 {
        return "now".to_owned();
    }
    let window_start = reset_at.saturating_sub(limit_window_seconds);
    let elapsed_seconds = now_unix_seconds.saturating_sub(window_start);
    if used_percent == 0 || elapsed_seconds == 0 {
        return "unknown".to_owned();
    }
    let seconds_to_runout = u64::from(100_u32.saturating_sub(used_percent))
        .saturating_mul(elapsed_seconds)
        / u64::from(used_percent);
    let runout = now_unix_seconds.saturating_add(seconds_to_runout);
    if runout > reset_at {
        return "after reset".to_owned();
    }

    format!(
        "in {}",
        format_duration_from_now(runout as i64 - now_unix_seconds as i64)
    )
}

fn format_notes(row: &PersistedQuotaStatusRow) -> String {
    if let Some(failure_message) = row.failure_message() {
        return failure_message.to_owned();
    }
    if row.effective() {
        return "effective".to_owned();
    }
    if !matches!(row.family(), "rate_limit" | "code_review") {
        return row.family().to_owned();
    }
    String::new()
}

fn pace_delta_percent(row: &PersistedQuotaStatusRow, now_unix_seconds: u64) -> Option<i64> {
    let used_percent = i64::from(row.used_percent()?);
    let reset_at = row.reset_unix_seconds()?;
    let limit_window_seconds = row.limit_window_seconds()?;
    if limit_window_seconds == 0 {
        return None;
    }
    let window_start = reset_at.saturating_sub(limit_window_seconds);
    let elapsed_seconds = now_unix_seconds
        .saturating_sub(window_start)
        .min(limit_window_seconds);
    let expected_used_percent = elapsed_seconds.saturating_mul(100) / limit_window_seconds;
    Some(used_percent - expected_used_percent as i64)
}

fn format_duration_from_now(seconds: i64) -> String {
    if seconds < 0 {
        return "elapsed".to_owned();
    }
    let days = seconds / 86_400;
    let hours = (seconds % 86_400) / 3_600;
    let minutes = (seconds % 3_600) / 60;
    let remaining_seconds = seconds % 60;
    if days > 0 {
        return format!("{days}d {hours}h");
    }
    if hours > 0 {
        return format!("{hours}h {minutes}m");
    }
    if minutes > 0 {
        return format!("{minutes}m {remaining_seconds}s");
    }
    format!("{remaining_seconds}s")
}

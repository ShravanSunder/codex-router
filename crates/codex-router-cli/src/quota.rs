//! Quota command glue for persisted router-owned quota state.

use std::io::Write;
use std::path::PathBuf;

use codex_router_auth::live_quota::DEFAULT_CHATGPT_BACKEND_BASE_URL;
use codex_router_state::account::AccountRecord;
use codex_router_state::quota_snapshot::PersistedQuotaSnapshot;
use codex_router_state::repositories::AccountStateRepository;
use codex_router_state::repositories::QuotaSnapshotRepository;
use codex_router_state::sqlite::SqliteStateStore;
use codex_router_state::sqlite::StateStoreError;
use comfy_table::Table;
use comfy_table::presets::UTF8_FULL;
use thiserror::Error;

use crate::ArgumentParser;
use crate::CliError;

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
        } => render_quota_status(stdout, router_root, format, all_limits),
        QuotaCommand::Refresh {
            router_root,
            base_url,
        } => refresh_quota(stdout, router_root, base_url),
    }
}

fn refresh_quota(
    _stdout: &mut impl Write,
    _router_root: PathBuf,
    base_url: String,
) -> Result<(), QuotaCommandError> {
    if !is_allowed_quota_refresh_base_url(&base_url) {
        return Err(QuotaCommandError::DisallowedBaseUrl { base_url });
    }

    Err(QuotaCommandError::RefreshNotImplemented)
}

fn is_allowed_quota_refresh_base_url(base_url: &str) -> bool {
    let trimmed = base_url.trim_end_matches('/');
    trimmed == DEFAULT_CHATGPT_BACKEND_BASE_URL
        || trimmed == "https://chatgpt.com"
        || trimmed.starts_with("https://chatgpt.com/")
}

fn render_quota_status(
    stdout: &mut impl Write,
    router_root: PathBuf,
    format: QuotaStatusFormat,
    all_limits: bool,
) -> Result<(), QuotaCommandError> {
    let state = SqliteStateStore::open(&router_root.join("state.sqlite"))?;
    let accounts = AccountStateRepository::list_accounts(&state)?;
    let rows = quota_status_rows(&state, &accounts, all_limits)?;
    match format {
        QuotaStatusFormat::Table => write_quota_table(stdout, &rows),
        QuotaStatusFormat::Plain => write_quota_plain(stdout, &rows),
    }
}

fn quota_status_rows(
    state: &SqliteStateStore,
    accounts: &[AccountRecord],
    all_limits: bool,
) -> Result<Vec<QuotaStatusRow>, QuotaCommandError> {
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
                rows.push(QuotaStatusRow::from_snapshot(account, &snapshot));
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
        "remaining",
        "reset",
        "stale",
        "source",
    ]);
    for row in rows {
        table.add_row([
            row.account_label.as_str(),
            row.account_id.as_str(),
            row.account_status.as_str(),
            row.route_band.as_str(),
            row.remaining_headroom.as_str(),
            row.reset.as_str(),
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
        "account\taccount_id\tstatus\troute_band\tremaining\treset\tstale\tsource"
    )
    .map_err(QuotaCommandError::Stdout)?;
    for row in rows {
        writeln!(
            stdout,
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            row.account_label,
            row.account_id,
            row.account_status,
            row.route_band,
            row.remaining_headroom,
            row.reset,
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
    remaining_headroom: String,
    reset: String,
    stale: String,
    source: String,
}

impl QuotaStatusRow {
    fn from_snapshot(account: &AccountRecord, snapshot: &PersistedQuotaSnapshot) -> Self {
        let reset = snapshot
            .reset_unix_seconds()
            .map_or_else(|| "-".to_owned(), |reset| reset.to_string());
        Self {
            account_label: account.label().to_owned(),
            account_id: account.account_id().as_str().to_owned(),
            account_status: account.status().as_str().to_owned(),
            route_band: snapshot.route_band().to_owned(),
            remaining_headroom: snapshot.remaining_headroom().to_string(),
            reset,
            stale: snapshot.stale_penalty().to_string(),
            source: snapshot.source().as_str().to_owned(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct QuotaStatusOptions {
    router_root: Option<PathBuf>,
    format: QuotaStatusFormat,
    all_limits: bool,
}

impl Default for QuotaStatusOptions {
    fn default() -> Self {
        Self {
            router_root: None,
            format: QuotaStatusFormat::Table,
            all_limits: false,
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

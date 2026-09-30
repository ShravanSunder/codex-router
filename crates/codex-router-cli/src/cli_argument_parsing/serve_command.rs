//! Serve command options and their validated runtime defaults.

use super::super::CliError;
use super::super::DEFAULT_CHATGPT_BACKEND_BASE_URL;
use super::super::DEFAULT_MAX_SNAPSHOT_AGE_SECONDS;
use super::super::DEFAULT_PROFILE_PORT;
use super::super::DEFAULT_QUOTA_REFRESH_INTERVAL_SECONDS;
use super::super::DEFAULT_SESSION_PIN_IDLE_TTL_SECONDS;
use super::super::default_router_root;
use super::ArgumentParser;
use super::parse_nonzero_u64_option;
use super::parse_port;
use super::parse_u64_option;
use super::parse_usize_option;
use codex_router_core::route_profile::ClaudeFiveHourReservePercent;
use codex_router_core::route_profile::DEFAULT_CLAUDE_FIVE_HOUR_RESERVE_PERCENT;
use std::num::NonZeroU64;
use std::path::PathBuf;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ServeCommand {
    pub(crate) listen_host: String,
    pub(crate) port: u16,
    pub(crate) state_db: PathBuf,
    pub(crate) secret_root: PathBuf,
    pub(crate) upstream_base_url: String,
    pub(crate) now_unix_seconds: Option<u64>,
    pub(crate) max_snapshot_age_seconds: u64,
    pub(crate) session_pin_idle_ttl_seconds: u64,
    pub(crate) quota_refresh_interval_seconds: u64,
    pub(crate) claude_five_hour_reserve_percent: ClaudeFiveHourReservePercent,
    pub(crate) background_quota_refresh_enabled: bool,
    pub(crate) require_local_token: bool,
    pub(crate) max_connections: usize,
    pub(crate) audit_file: Option<PathBuf>,
    pub(crate) websocket_registry_report_file: Option<PathBuf>,
}

impl ServeCommand {
    pub(super) fn parse(parser: &mut ArgumentParser) -> Result<Self, CliError> {
        let options = ServeCommandOptions::parse(parser)?;
        let listen_host = options
            .listen_host
            .unwrap_or_else(|| "127.0.0.1".to_owned());
        let port = options.port.unwrap_or(DEFAULT_PROFILE_PORT);
        let router_root = default_router_root()?;
        let state_db = options
            .state_db
            .unwrap_or_else(|| router_root.join("state.sqlite"));
        let secret_root = options
            .secret_root
            .unwrap_or_else(|| router_root.join("secrets"));
        let upstream_base_url = options
            .upstream_base_url
            .unwrap_or_else(|| DEFAULT_CHATGPT_BACKEND_BASE_URL.to_owned());

        Ok(Self {
            listen_host,
            port,
            state_db,
            secret_root,
            upstream_base_url,
            now_unix_seconds: options.now_unix_seconds,
            max_snapshot_age_seconds: options
                .max_snapshot_age_seconds
                .unwrap_or(DEFAULT_MAX_SNAPSHOT_AGE_SECONDS),
            session_pin_idle_ttl_seconds: options
                .session_pin_idle_ttl_seconds
                .map(NonZeroU64::get)
                .unwrap_or(DEFAULT_SESSION_PIN_IDLE_TTL_SECONDS),
            quota_refresh_interval_seconds: options
                .quota_refresh_interval_seconds
                .unwrap_or(DEFAULT_QUOTA_REFRESH_INTERVAL_SECONDS),
            claude_five_hour_reserve_percent: options
                .claude_five_hour_reserve_percent
                .unwrap_or(DEFAULT_CLAUDE_FIVE_HOUR_RESERVE_PERCENT),
            background_quota_refresh_enabled: !options.disable_background_quota_refresh,
            require_local_token: options.require_local_token,
            max_connections: options.max_connections.unwrap_or(usize::MAX),
            audit_file: options.audit_file,
            websocket_registry_report_file: options.websocket_registry_report_file,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ServeCommandOptions {
    listen_host: Option<String>,
    port: Option<u16>,
    state_db: Option<PathBuf>,
    secret_root: Option<PathBuf>,
    upstream_base_url: Option<String>,
    now_unix_seconds: Option<u64>,
    max_snapshot_age_seconds: Option<u64>,
    session_pin_idle_ttl_seconds: Option<NonZeroU64>,
    quota_refresh_interval_seconds: Option<u64>,
    claude_five_hour_reserve_percent: Option<ClaudeFiveHourReservePercent>,
    disable_background_quota_refresh: bool,
    require_local_token: bool,
    max_connections: Option<usize>,
    audit_file: Option<PathBuf>,
    websocket_registry_report_file: Option<PathBuf>,
}

impl ServeCommandOptions {
    fn parse(parser: &mut ArgumentParser) -> Result<Self, CliError> {
        let mut options = Self {
            listen_host: None,
            port: None,
            state_db: None,
            secret_root: None,
            upstream_base_url: None,
            now_unix_seconds: None,
            max_snapshot_age_seconds: None,
            session_pin_idle_ttl_seconds: None,
            quota_refresh_interval_seconds: None,
            claude_five_hour_reserve_percent: None,
            disable_background_quota_refresh: false,
            require_local_token: false,
            max_connections: None,
            audit_file: None,
            websocket_registry_report_file: None,
        };

        while let Some(argument) = parser.next_string()? {
            match argument.as_str() {
                "--listen-host" => {
                    options.listen_host = Some(parser.next_required_value("--listen-host")?);
                }
                "--port" => {
                    let value = parser.next_required_value("--port")?;
                    options.port = Some(parse_port(&value)?);
                }
                "--state-db" => {
                    let value = parser.next_required_value("--state-db")?;
                    options.state_db = Some(PathBuf::from(value));
                }
                "--secret-root" => {
                    let value = parser.next_required_value("--secret-root")?;
                    options.secret_root = Some(PathBuf::from(value));
                }
                "--upstream-base-url" => {
                    options.upstream_base_url =
                        Some(parser.next_required_value("--upstream-base-url")?);
                }
                "--now-unix-seconds" => {
                    let value = parser.next_required_value("--now-unix-seconds")?;
                    options.now_unix_seconds =
                        Some(parse_u64_option("--now-unix-seconds", &value)?);
                }
                "--max-snapshot-age-seconds" => {
                    let value = parser.next_required_value("--max-snapshot-age-seconds")?;
                    options.max_snapshot_age_seconds =
                        Some(parse_u64_option("--max-snapshot-age-seconds", &value)?);
                }
                "--session-pin-idle-ttl-seconds" => {
                    let value = parser.next_required_value("--session-pin-idle-ttl-seconds")?;
                    options.session_pin_idle_ttl_seconds = Some(parse_nonzero_u64_option(
                        "--session-pin-idle-ttl-seconds",
                        &value,
                    )?);
                }
                "--quota-refresh-interval-seconds" => {
                    let value = parser.next_required_value("--quota-refresh-interval-seconds")?;
                    options.quota_refresh_interval_seconds = Some(parse_u64_option(
                        "--quota-refresh-interval-seconds",
                        &value,
                    )?);
                }
                "--claude-five-hour-reserve-percent" => {
                    let value = parser.next_required_value("--claude-five-hour-reserve-percent")?;
                    options.claude_five_hour_reserve_percent =
                        Some(parse_claude_five_hour_reserve_percent(
                            "--claude-five-hour-reserve-percent",
                            &value,
                        )?);
                }
                "--disable-background-quota-refresh" => {
                    options.disable_background_quota_refresh = true;
                }
                "--require-local-token" => {
                    options.require_local_token = true;
                }
                "--max-connections" => {
                    let value = parser.next_required_value("--max-connections")?;
                    options.max_connections =
                        Some(parse_usize_option("--max-connections", &value)?);
                }
                "--audit-file" => {
                    let value = parser.next_required_value("--audit-file")?;
                    options.audit_file = Some(PathBuf::from(value));
                }
                "--websocket-registry-report-file" => {
                    let value = parser.next_required_value("--websocket-registry-report-file")?;
                    options.websocket_registry_report_file = Some(PathBuf::from(value));
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
}

fn parse_claude_five_hour_reserve_percent(
    option: &'static str,
    value: &str,
) -> Result<ClaudeFiveHourReservePercent, CliError> {
    let percent = value
        .parse::<u8>()
        .map_err(|_| CliError::InvalidNumericOption {
            option,
            value: value.to_owned(),
        })?;
    ClaudeFiveHourReservePercent::new(percent).ok_or_else(|| CliError::NumericOptionOutOfRange {
        option,
        value: value.to_owned(),
        minimum: 1,
        maximum: 99,
    })
}

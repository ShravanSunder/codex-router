//! Live upstream diagnostic commands.

use std::fs;
use std::io::Write;
use std::path::PathBuf;

use codex_router_auth::live_quota::DEFAULT_CHATGPT_BACKEND_BASE_URL;
use codex_router_auth::live_quota::LiveQuotaClient;
use codex_router_auth::live_quota::LiveQuotaError;
use codex_router_auth::live_quota::QuotaEndpointPolicy;
use codex_router_auth::live_quota::UsageResponse;
use codex_router_core::ids::AccountId;

use crate::ArgumentParser;
use crate::CliError;
use crate::current_unix_seconds;
use crate::parse_u64_option;
use crate::quota;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum LiveCommand {
    Quota(LiveQuotaCommand),
}

impl LiveCommand {
    pub(crate) fn parse(parser: &mut ArgumentParser) -> Result<Self, CliError> {
        let Some(command) = parser.next_string()? else {
            return Err(CliError::MissingCommand {
                command: "live".to_owned(),
            });
        };
        match command.as_str() {
            "quota" => Ok(Self::Quota(LiveQuotaCommand::parse(parser)?)),
            unknown => Err(CliError::UnknownCommand {
                command: format!("live {unknown}"),
            }),
        }
    }
}

pub(crate) fn run_live_command(
    stdout: &mut impl Write,
    command: LiveCommand,
) -> Result<(), CliError> {
    match command {
        LiveCommand::Quota(command) => run_live_quota_command(stdout, command),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LiveQuotaCommand {
    auth_json: Option<PathBuf>,
    profiles_root: Option<PathBuf>,
    profile_label: Option<String>,
    base_url: String,
    allow_insecure_quota_base_url: bool,
    output_format: LiveQuotaOutputFormat,
    all_limits: bool,
    now_unix_seconds: Option<i64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct LiveQuotaProfile {
    label: String,
    auth_json_path: PathBuf,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum LiveQuotaOutputFormat {
    #[default]
    Plain,
    Table,
}

impl LiveQuotaCommand {
    fn parse(parser: &mut ArgumentParser) -> Result<Self, CliError> {
        let mut command = Self {
            auth_json: None,
            profiles_root: None,
            profile_label: None,
            base_url: DEFAULT_CHATGPT_BACKEND_BASE_URL.to_owned(),
            allow_insecure_quota_base_url: false,
            output_format: LiveQuotaOutputFormat::Plain,
            all_limits: false,
            now_unix_seconds: None,
        };

        while let Some(argument) = parser.next_string()? {
            match argument.as_str() {
                "--auth-json" => {
                    command.auth_json =
                        Some(PathBuf::from(parser.next_required_value("--auth-json")?));
                }
                "--profiles-root" => {
                    command.profiles_root = Some(PathBuf::from(
                        parser.next_required_value("--profiles-root")?,
                    ));
                }
                "--profile-label" => {
                    command.profile_label = Some(parser.next_required_value("--profile-label")?);
                }
                "--base-url" => {
                    command.base_url = parser.next_required_value("--base-url")?;
                }
                "--allow-insecure-quota-base-url" => {
                    command.allow_insecure_quota_base_url = true;
                }
                "--format" => {
                    let value = parser.next_required_value("--format")?;
                    command.output_format = parse_live_quota_output_format(value.as_str())?;
                }
                "--all-limits" => {
                    command.all_limits = true;
                }
                "--now-unix-seconds" => {
                    let value = parser.next_required_value("--now-unix-seconds")?;
                    let parsed = parse_u64_option("--now-unix-seconds", value.as_str())?;
                    command.now_unix_seconds = Some(i64::try_from(parsed).map_err(|_| {
                        CliError::InvalidNumericOption {
                            option: "--now-unix-seconds",
                            value,
                        }
                    })?);
                }
                unknown => {
                    return Err(CliError::UnknownOption {
                        option: unknown.to_owned(),
                    });
                }
            }
        }

        if command.auth_json.is_some() == command.profiles_root.is_some() {
            return Err(CliError::LiveQuotaSourceRequired);
        }

        Ok(command)
    }

    fn profiles(&self) -> Result<Vec<LiveQuotaProfile>, CliError> {
        if let Some(auth_json_path) = &self.auth_json {
            return Ok(vec![LiveQuotaProfile {
                label: self
                    .profile_label
                    .clone()
                    .unwrap_or_else(|| "auth-json".to_owned()),
                auth_json_path: auth_json_path.clone(),
            }]);
        }

        let profiles_root = self
            .profiles_root
            .as_ref()
            .ok_or(CliError::LiveQuotaSourceRequired)?;
        let entries =
            fs::read_dir(profiles_root).map_err(|error| CliError::LiveQuotaProfileRead {
                message: error.to_string(),
            })?;
        let mut profiles = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| CliError::LiveQuotaProfileRead {
                message: error.to_string(),
            })?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Some(label) = path
                .file_name()
                .and_then(|file_name| file_name.to_str())
                .map(str::to_owned)
            else {
                continue;
            };
            if label.starts_with(".login-") {
                continue;
            }
            let auth_json_path = path.join("auth.json");
            if auth_json_path.exists() {
                profiles.push(LiveQuotaProfile {
                    label,
                    auth_json_path,
                });
            }
        }
        profiles.sort_by(|left, right| left.label.cmp(&right.label));
        Ok(profiles)
    }
}

fn run_live_quota_command(
    stdout: &mut impl Write,
    command: LiveQuotaCommand,
) -> Result<(), CliError> {
    let endpoint_policy = if command.allow_insecure_quota_base_url {
        QuotaEndpointPolicy::AllowLoopbackForTesting
    } else {
        QuotaEndpointPolicy::ProviderOnly
    };
    let client = LiveQuotaClient::new_with_timeout_and_policy(
        command.base_url.as_str(),
        None,
        endpoint_policy,
    )?;
    let profiles = command.profiles()?;
    if profiles.is_empty() {
        return Err(CliError::NoLiveQuotaProfiles);
    }
    let now_unix_seconds = match command.now_unix_seconds {
        Some(now_unix_seconds) => now_unix_seconds,
        None => {
            i64::try_from(current_unix_seconds()?).map_err(|_| CliError::InvalidNumericOption {
                option: "--now-unix-seconds",
                value: "system clock overflow".to_owned(),
            })?
        }
    };
    let mut results = Vec::new();
    for profile in profiles {
        let result = client.fetch_from_auth_json(&profile.auth_json_path);
        results.push((profile.label, result));
    }
    let now_unix_seconds =
        u64::try_from(now_unix_seconds).map_err(|_| CliError::InvalidNumericOption {
            option: "--now-unix-seconds",
            value: "negative timestamp".to_owned(),
        })?;
    let rendered = live_results_to_quota_status_rows(&results, now_unix_seconds)?;
    let visible_rows = quota::visible_status_rows(&rendered.rows, command.all_limits);
    match command.output_format {
        LiveQuotaOutputFormat::Plain => {
            quota::write_quota_status_plain(
                stdout,
                &visible_rows,
                &rendered.labels,
                now_unix_seconds,
            )
            .map_err(CliError::Stdout)?;
        }
        LiveQuotaOutputFormat::Table => {
            let table =
                quota::render_quota_status_table(&visible_rows, &rendered.labels, now_unix_seconds);
            writeln!(stdout, "{table}").map_err(CliError::Stdout)?;
        }
    }
    Ok(())
}

fn parse_live_quota_output_format(value: &str) -> Result<LiveQuotaOutputFormat, CliError> {
    match value {
        "plain" => Ok(LiveQuotaOutputFormat::Plain),
        "table" => Ok(LiveQuotaOutputFormat::Table),
        unknown => Err(CliError::UnknownOption {
            option: format!("--format {unknown}"),
        }),
    }
}

struct LiveRenderedQuotaRows {
    rows: Vec<codex_router_state::quota_snapshot::PersistedQuotaStatusRow>,
    labels: std::collections::BTreeMap<String, String>,
}

fn live_results_to_quota_status_rows(
    results: &[(String, Result<UsageResponse, LiveQuotaError>)],
    now_unix_seconds: u64,
) -> Result<LiveRenderedQuotaRows, CliError> {
    let mut rows = Vec::new();
    let mut labels = std::collections::BTreeMap::new();
    for (index, (_label, result)) in results.iter().enumerate() {
        let account_id = AccountId::new(format!("live_{index}")).map_err(|_| {
            CliError::InvalidNumericOption {
                option: "live-account-id",
                value: index.to_string(),
            }
        })?;
        labels.insert(
            account_id.as_str().to_owned(),
            format!("profile-{}", index + 1),
        );
        match result {
            Ok(usage) => rows.extend(quota::status_rows_from_usage_response(
                &account_id,
                usage,
                now_unix_seconds,
            )),
            Err(error) => rows.push(quota::failed_status_row(
                &account_id,
                "responses",
                "live",
                live_quota_error_label(error),
                now_unix_seconds,
            )),
        }
    }

    Ok(LiveRenderedQuotaRows { rows, labels })
}

fn live_quota_error_label(error: &LiveQuotaError) -> &'static str {
    match error {
        LiveQuotaError::ReadAuth { .. } => "read_auth",
        LiveQuotaError::ParseAuth { .. } => "parse_auth",
        LiveQuotaError::ApiKeyAuth => "api_key_auth_not_quota_compatible",
        LiveQuotaError::MissingAccessToken => "missing_access_token",
        LiveQuotaError::Request { .. } => "request_failed",
        LiveQuotaError::ProviderStatus { .. } => "provider_status",
        LiveQuotaError::ResponseJson { .. } => "response_json",
        LiveQuotaError::InvalidBaseUrl { .. } => "invalid_base_url",
        LiveQuotaError::DisallowedBaseUrl => "disallowed_base_url",
    }
}

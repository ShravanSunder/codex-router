//! Live upstream diagnostic commands.

use std::fs;
use std::io::Write;
use std::path::PathBuf;

use codex_router_auth::live_quota::DEFAULT_CHATGPT_BACKEND_BASE_URL;
use codex_router_auth::live_quota::LiveQuotaClient;
use codex_router_auth::live_quota::LiveQuotaError;
use codex_router_auth::live_quota::UsageResponse;
use codex_router_auth::live_quota::UsageWindow;

use crate::ArgumentParser;
use crate::CliError;
use crate::quota::is_allowed_quota_refresh_base_url;

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
    dry_run: bool,
    approve_network_account_use: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct LiveQuotaProfile {
    label: String,
    auth_json_path: PathBuf,
}

impl LiveQuotaCommand {
    fn parse(parser: &mut ArgumentParser) -> Result<Self, CliError> {
        let mut command = Self {
            auth_json: None,
            profiles_root: None,
            profile_label: None,
            base_url: DEFAULT_CHATGPT_BACKEND_BASE_URL.to_owned(),
            dry_run: false,
            approve_network_account_use: false,
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
                "--dry-run" => {
                    command.dry_run = true;
                }
                "--approve-network-account-use" => {
                    command.approve_network_account_use = true;
                }
                "--approve-live-generation" => {
                    // Accepted as a forward-compatible explicit gate; this
                    // diagnostic command never performs live generation.
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
    let profiles = command.profiles()?;
    if profiles.is_empty() {
        return Err(CliError::NoLiveQuotaProfiles);
    }
    if command.dry_run {
        for profile in profiles {
            write_live_quota_dry_run_profile(stdout, &profile.label).map_err(CliError::Stdout)?;
        }
        return Ok(());
    }
    if !command.approve_network_account_use {
        return Err(CliError::LiveQuotaApprovalRequired);
    }
    if !is_allowed_quota_refresh_base_url(&command.base_url)
        && !test_allowed_live_quota_base_url(&command.base_url)
    {
        return Err(CliError::LiveQuotaDisallowedBaseUrl {
            base_url: command.base_url,
        });
    }
    let client = LiveQuotaClient::new(command.base_url.as_str())?;
    for profile in profiles {
        let result = client.fetch_from_auth_json(&profile.auth_json_path);
        write_live_quota_profile(stdout, &profile.label, result.as_ref())
            .map_err(CliError::Stdout)?;
    }
    Ok(())
}

#[cfg(test)]
fn test_allowed_live_quota_base_url(base_url: &str) -> bool {
    base_url.starts_with("http://127.0.0.1:") || base_url.starts_with("http://[::1]:")
}

#[cfg(not(test))]
const fn test_allowed_live_quota_base_url(_base_url: &str) -> bool {
    false
}

fn write_live_quota_dry_run_profile(
    stdout: &mut impl Write,
    label: &str,
) -> Result<(), std::io::Error> {
    writeln!(stdout, "profile: {label}")?;
    writeln!(stdout, "status: dry-run")?;
    writeln!(stdout, "network: not-run")?;
    writeln!(stdout, "generation: not-run")?;
    writeln!(stdout)
}

fn write_live_quota_profile(
    stdout: &mut impl Write,
    label: &str,
    result: Result<&UsageResponse, &LiveQuotaError>,
) -> Result<(), std::io::Error> {
    writeln!(stdout, "profile: {label}")?;
    match result {
        Ok(usage) => {
            writeln!(stdout, "auth: chatgpt-oauth")?;
            writeln!(stdout, "status: ok")?;
            write_usage_pair(stdout, "rate_limit", usage.rate_limit.as_ref())?;
            writeln!(
                stdout,
                "code_review_rate_limit_present: {}",
                usage.code_review_rate_limit.is_some()
            )?;
            writeln!(
                stdout,
                "additional_rate_limit_count: {}",
                usage.additional_rate_limits.len()
            )?;
        }
        Err(error) => {
            writeln!(stdout, "status: error")?;
            writeln!(stdout, "error: {}", live_quota_error_label(error))?;
        }
    }
    writeln!(stdout)
}

fn write_usage_pair(
    stdout: &mut impl Write,
    name: &str,
    pair: Option<&codex_router_auth::live_quota::WindowPair>,
) -> Result<(), std::io::Error> {
    if let Some(pair) = pair {
        write_usage_window(
            stdout,
            &format!("{name}.primary"),
            pair.primary_window.as_ref(),
        )?;
        write_usage_window(
            stdout,
            &format!("{name}.secondary"),
            pair.secondary_window.as_ref(),
        )?;
    } else {
        writeln!(stdout, "{name}: missing")?;
    }
    Ok(())
}

fn write_usage_window(
    stdout: &mut impl Write,
    name: &str,
    window: Option<&UsageWindow>,
) -> Result<(), std::io::Error> {
    let Some(window) = window else {
        writeln!(stdout, "{name}: missing")?;
        return Ok(());
    };
    let remaining_percent = window
        .used_percent
        .map(|used_percent| 100_i64.saturating_sub(used_percent).max(0));
    writeln!(
        stdout,
        "{name}: remaining_percent={} reset_at_present={} limit_window_seconds={}",
        format_optional_i64(remaining_percent),
        window.reset_at.is_some(),
        format_optional_i64(window.limit_window_seconds)
    )
}

fn format_optional_i64(value: Option<i64>) -> String {
    value.map_or_else(|| "unknown".to_owned(), |value| value.to_string())
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
    }
}

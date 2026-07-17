use super::*;

pub(super) struct QuotaStatusOptions {
    pub(super) router_root: Option<PathBuf>,
    pub(super) format: QuotaStatusFormat,
    pub(super) all_limits: bool,
    pub(super) now_unix_seconds: u64,
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
    pub(super) fn parse(parser: &mut ArgumentParser) -> Result<Self, CliError> {
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

    pub(super) fn router_root(&self) -> Result<PathBuf, CliError> {
        router_root_or_default(self.router_root.clone())
    }
}

pub(super) fn parse_quota_status_format(value: &str) -> Result<QuotaStatusFormat, CliError> {
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
pub(super) struct QuotaRefreshOptions {
    pub(super) router_root: Option<PathBuf>,
    pub(super) base_url: String,
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
    pub(super) fn parse(parser: &mut ArgumentParser) -> Result<Self, CliError> {
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

    pub(super) fn router_root(&self) -> Result<PathBuf, CliError> {
        router_root_or_default(self.router_root.clone())
    }
}

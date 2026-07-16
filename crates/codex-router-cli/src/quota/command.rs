use super::*;

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
    /// A quota command was passed to the synchronous CLI dispatcher.
    #[error("quota commands require the async CLI dispatcher")]
    AsyncDispatchRequired,
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
    /// Failed to initialize the serve-owned background refresh executor.
    #[error("failed to initialize quota history runtime: {0}")]
    BackgroundWorkerInitialization(std::io::Error),
    /// Stdout write failed.
    #[error("failed to write stdout: {0}")]
    Stdout(std::io::Error),
}

/// Follow-up composition requested by a quota command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum QuotaCommandDispatch {
    /// Command completed entirely inside the quota command family.
    Complete,
    /// The legacy standalone reset composition must run at the CLI boundary.
    LegacyReset { router_root: PathBuf },
}

/// Runs every quota command under the process-owned async runtime.
pub(crate) async fn run_quota_command(
    stdout: &mut impl Write,
    command: QuotaCommand,
    stdin_is_terminal: bool,
    stdout_is_terminal: bool,
    stdout_terminal_width: Option<usize>,
) -> Result<QuotaCommandDispatch, QuotaCommandError> {
    match command {
        QuotaCommand::Help(help_text) => {
            stdout
                .write_all(help_text.as_bytes())
                .map_err(QuotaCommandError::Stdout)?;
            Ok(QuotaCommandDispatch::Complete)
        }
        QuotaCommand::Status {
            router_root,
            format,
            all_limits,
            now_unix_seconds,
        } => {
            if should_run_interactive_quota(format, stdin_is_terminal, stdout_is_terminal) {
                render_interactive_quota_status(
                    router_root,
                    stdout_terminal_width,
                    all_limits,
                    now_unix_seconds,
                )
                .await?;
            } else {
                render_quota_status(
                    stdout,
                    router_root,
                    format,
                    stdout_is_terminal,
                    stdout_terminal_width,
                    all_limits,
                    now_unix_seconds,
                )
                .await?;
            }
            Ok(QuotaCommandDispatch::Complete)
        }
        QuotaCommand::Refresh {
            router_root,
            base_url,
        } => {
            refresh_quota(stdout, router_root, base_url).await?;
            Ok(QuotaCommandDispatch::Complete)
        }
        QuotaCommand::Reset { router_root } => {
            Ok(QuotaCommandDispatch::LegacyReset { router_root })
        }
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

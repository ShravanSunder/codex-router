//! Public CLI error vocabulary; formatting is independent of dispatch.
use crate::credential_upkeep_worker::CredentialUpkeepStartError;
use crate::{
    account::AccountCommandError, host_command::HostCommandError, profile::ProfileWriteError,
    quota::QuotaCommandError, token::TokenCommandError,
};
use codex_router_proxy::{
    server::{LoopbackRouterRuntimeError, ServerBindError},
    upstream::{ClaudeUpstreamEndpointError, UpstreamEndpointError},
};
use std::ffi::OsString;
use thiserror::Error;

/// CLI execution failure.
#[derive(Debug, Error)]
pub enum CliError {
    /// Background OAuth upkeep could not start.
    #[error(transparent)]
    CredentialUpkeep(#[from] CredentialUpkeepStartError),
    /// Encrypted credential store could not be opened for this process.
    #[error("encrypted credential store could not be opened")]
    CredentialStoreOpen,
    /// Command name is unknown.
    #[error("unknown command: {command}")]
    UnknownCommand {
        /// Unknown command.
        command: String,
    },

    /// A nested command is missing.
    #[error("missing command after {command}")]
    MissingCommand {
        /// Parent command.
        command: String,
    },

    /// An option is unknown.
    #[error("unknown option: {option}")]
    UnknownOption {
        /// Unknown option.
        option: String,
    },

    /// A required option is missing.
    #[error("missing required option: {option}")]
    MissingOption {
        /// Missing option.
        option: &'static str,
    },

    /// HOME is unavailable for the default router root.
    #[error("HOME is not set; pass --router-root <path>")]
    HomeDirectoryUnavailable,

    /// A required option value is missing.
    #[error("missing value for option: {option}")]
    MissingOptionValue {
        /// Option missing its value.
        option: &'static str,
    },

    /// Port is invalid.
    #[error("invalid profile port: {value}")]
    InvalidPort {
        /// Raw port value.
        value: String,
    },

    /// Shell is invalid.
    #[error("invalid shell: {value}")]
    InvalidShell {
        /// Raw shell value.
        value: String,
    },

    /// Numeric option is invalid.
    #[error("invalid numeric value for {option}: {value}")]
    InvalidNumericOption {
        /// Option name.
        option: &'static str,
        /// Raw option value.
        value: String,
    },

    /// Numeric option must be nonzero.
    #[error("value for {option} must be greater than zero: {value}")]
    ZeroNumericOption {
        /// Option name.
        option: &'static str,
        /// Raw option value.
        value: String,
    },
    /// Percentage option is outside its inclusive supported range.
    #[error("value for {option} must be between {minimum} and {maximum}: {value}")]
    NumericOptionOutOfRange {
        /// Option name.
        option: &'static str,
        /// Raw option value.
        value: String,
        /// Lowest accepted value.
        minimum: u8,
        /// Highest accepted value.
        maximum: u8,
    },
    /// CLI argument is not UTF-8.
    #[error("non-UTF-8 CLI argument: {value:?}")]
    NonUtf8Argument {
        /// Raw argument.
        value: OsString,
    },

    /// Profile write failed.
    #[error(transparent)]
    ProfileWrite(#[from] ProfileWriteError),

    /// Token command failed.
    #[error(transparent)]
    Token(#[from] TokenCommandError),
    /// Account command failed.
    #[error(transparent)]
    Account(#[from] AccountCommandError),
    /// Quota command failed.
    #[error(transparent)]
    Quota(#[from] QuotaCommandError),
    /// Live quota command needs exactly one source.
    #[error("live quota requires --profiles-root")]
    LiveQuotaSourceRequired,
    /// Live quota profile discovery failed.
    #[error("failed to read live quota profiles: {message}")]
    LiveQuotaProfileRead {
        /// Redacted message.
        message: String,
    },
    /// Live quota command found no profiles.
    #[error("no live quota profiles found")]
    NoLiveQuotaProfiles,
    /// Live quota command needs explicit approval before network/account use.
    #[error("live quota requires --approve-network-account-use or --dry-run")]
    LiveQuotaApprovalRequired,
    /// Live quota base URL is not one of the allowlisted provider URLs.
    #[error("live quota base URL is not allowed: {base_url}")]
    LiveQuotaDisallowedBaseUrl {
        /// Rejected base URL.
        base_url: String,
    },
    /// Live quota request failed.
    #[error(transparent)]
    LiveQuota(#[from] codex_router_auth::live_quota::LiveQuotaError),

    /// Host command failed to parse.
    #[error("{message}")]
    HostParse {
        /// Clap-rendered parse error.
        message: String,
    },

    /// Shared host command failed.
    #[error(transparent)]
    Host(#[from] HostCommandError),

    /// Host commands use the process's existing Tokio runtime.
    #[error("host command requires native async dispatch")]
    HostRequiresAsyncDispatch,

    /// Serve uses the process's existing Tokio runtime.
    #[error("serve command requires native async dispatch")]
    ServeRequiresAsyncDispatch,

    /// Loopback bind failed.
    #[error(transparent)]
    Bind(#[from] ServerBindError),

    /// Upstream endpoint was invalid.
    #[error(transparent)]
    UpstreamEndpoint(#[from] UpstreamEndpointError),

    /// Debug-only Claude upstream endpoint was invalid or lacked isolation.
    #[error(
        "debug Claude upstream override from CODEX_ROUTER_DEBUG_CLAUDE_UPSTREAM_BASE_URL (option --debug-claude-upstream-base-url) failed: {0}"
    )]
    ClaudeUpstreamEndpoint(#[from] ClaudeUpstreamEndpointError),

    /// Router runtime failed.
    #[error(transparent)]
    Runtime(#[from] LoopbackRouterRuntimeError),

    /// WebSocket registry report failed to render.
    #[error("failed to render websocket registry report: {0}")]
    WebSocketRegistryReportRender(serde_json::Error),

    /// WebSocket registry report failed to write.
    #[error("failed to write websocket registry report {path}: {source}")]
    WebSocketRegistryReportWrite {
        /// Destination path.
        path: String,
        /// Source error.
        source: std::io::Error,
    },

    /// Role-owned lifecycle failure; the CLI retains its original typed meaning.
    #[error(transparent)]
    ProxyLifecycle(agent_proxy_services::ProxyActivationError),

    /// Stdout write failed.
    #[error("failed to write stdout: {0}")]
    Stdout(std::io::Error),

    /// Stderr write failed.
    #[error("failed to write stderr: {0}")]
    Stderr(std::io::Error),
}

impl From<agent_proxy_services::quota::QuotaRefreshError> for CliError {
    fn from(error: agent_proxy_services::quota::QuotaRefreshError) -> Self {
        Self::from(crate::quota::QuotaCommandError::from(error))
    }
}

impl From<agent_proxy_services::ProxyPreparationError> for CliError {
    fn from(error: agent_proxy_services::ProxyPreparationError) -> Self {
        match error {
            agent_proxy_services::ProxyPreparationError::Core(error) => Self::from(error),
            agent_proxy_services::ProxyPreparationError::Token(error) => Self::from(error),
            agent_proxy_services::ProxyPreparationError::TokenStore(error) => {
                Self::from(TokenCommandError::SecretStore(error))
            }
            agent_proxy_services::ProxyPreparationError::Affinity(error) => Self::from(
                LoopbackRouterRuntimeError::CredentialResources(error.into()),
            ),
            agent_proxy_services::ProxyPreparationError::Schema(error) => {
                Self::from(LoopbackRouterRuntimeError::SchemaPreparation(error))
            }
            agent_proxy_services::ProxyPreparationError::StateInspection(error) => {
                Self::from(LoopbackRouterRuntimeError::StateInspection(error))
            }
            _ => Self::CredentialStoreOpen,
        }
    }
}
impl From<agent_proxy_services::ProxyActivationError> for CliError {
    fn from(error: agent_proxy_services::ProxyActivationError) -> Self {
        match error {
            error @ agent_proxy_services::ProxyActivationError::LifecycleUnavailable => {
                Self::ProxyLifecycle(error)
            }
            agent_proxy_services::ProxyActivationError::Core(error) => Self::from(error),
            agent_proxy_services::ProxyActivationError::Upkeep(error) => Self::from(error),
            agent_proxy_services::ProxyActivationError::Quota(error) => Self::from(error),
            agent_proxy_services::ProxyActivationError::StartupAnnouncement(error) => {
                Self::Stdout(error)
            }
            agent_proxy_services::ProxyActivationError::ServingTask(error) => Self::from(
                codex_router_proxy::server::LoopbackRouterRuntimeError::ConnectionJoin(error),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::CliError;
    use codex_router_proxy::upstream::ClaudeUpstreamEndpointError;

    #[test]
    fn debug_claude_endpoint_errors_name_the_setting_and_keep_values_redacted() {
        let invalid_url = CliError::from(ClaudeUpstreamEndpointError::InvalidBaseUrl);
        let invalid_url_message = invalid_url.to_string();
        assert!(invalid_url_message.contains("--debug-claude-upstream-base-url"));
        assert!(invalid_url_message.contains("CODEX_ROUTER_DEBUG_CLAUDE_UPSTREAM_BASE_URL"));
        assert!(invalid_url_message.contains("absolute HTTP(S) base URL"));

        let missing_isolation = CliError::from(ClaudeUpstreamEndpointError::DebugIsolationRequired);
        let missing_isolation_message = missing_isolation.to_string();
        assert!(missing_isolation_message.contains("--debug-claude-upstream-base-url"));
        assert!(missing_isolation_message.contains("CODEX_ROUTER_DEBUG_CLAUDE_UPSTREAM_BASE_URL"));
        assert!(missing_isolation_message.contains("requires debug isolation"));

        let supplied_value = "http://user:token@127.0.0.1:18888?secret=value";
        assert!(!invalid_url_message.contains(supplied_value));
        assert!(!missing_isolation_message.contains(supplied_value));
    }
}

//! Public CLI error vocabulary; formatting is independent of dispatch.
use crate::{
    account::AccountCommandError, host_command::HostCommandError, profile::ProfileWriteError,
    quota::QuotaCommandError, token::TokenCommandError,
};
use codex_router_proxy::{
    server::{LoopbackRouterRuntimeError, ServerBindError},
    upstream::UpstreamEndpointError,
};
use std::ffi::OsString;
use thiserror::Error;

/// CLI execution failure.
#[derive(Debug, Error)]
pub enum CliError {
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
    #[error("live quota requires exactly one of --auth-json or --profiles-root")]
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

    /// Loopback bind failed.
    #[error(transparent)]
    Bind(#[from] ServerBindError),

    /// Upstream endpoint was invalid.
    #[error(transparent)]
    UpstreamEndpoint(#[from] UpstreamEndpointError),

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

    /// Stdout write failed.
    #[error("failed to write stdout: {0}")]
    Stdout(std::io::Error),

    /// Stderr write failed.
    #[error("failed to write stderr: {0}")]
    Stderr(std::io::Error),
}

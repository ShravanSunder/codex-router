//! Error types shared by core primitives.

use thiserror::Error;

/// Configuration validation failure.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ConfigError {
    /// A listener attempted to bind outside loopback.
    #[error("non-loopback listener values are not supported in v1: {listen_host}")]
    NonLoopbackListenHost {
        /// Rejected listener host.
        listen_host: String,
    },
}

/// Identifier validation failure.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum IdError {
    /// Empty identifiers are not useful as durable keys.
    #[error("identifier must not be empty")]
    Empty,
}

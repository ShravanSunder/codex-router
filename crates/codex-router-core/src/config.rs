//! Router configuration model.

use std::path::PathBuf;

use serde::Deserialize;

use crate::error::ConfigError;

/// Top-level router configuration.
#[derive(Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RouterConfig {
    /// Local server settings.
    pub server: ServerConfig,
    /// Local router authentication settings.
    pub auth: LocalAuthConfig,
    /// Audit output settings.
    pub audit: AuditConfig,
}

impl RouterConfig {
    /// Test helper for listener validation.
    #[must_use]
    pub fn for_test(listen_host: &str) -> Self {
        Self {
            server: ServerConfig {
                listen_host: listen_host.to_owned(),
                port: 8787,
                router_root: PathBuf::from("/tmp/codex-router-test"),
            },
            auth: LocalAuthConfig {
                local_token_env: "CODEX_ROUTER_TOKEN".to_owned(),
            },
            audit: AuditConfig {
                sink: AuditSink::File,
            },
        }
    }

    /// Validates config invariants that cannot be expressed by TOML shape.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if !is_loopback_host(&self.server.listen_host) {
            return Err(ConfigError::NonLoopbackListenHost {
                listen_host: self.server.listen_host.clone(),
            });
        }

        Ok(())
    }
}

/// Local server settings.
#[derive(Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    /// Host interface to bind.
    pub listen_host: String,
    /// TCP port to bind.
    pub port: u16,
    /// Router-owned state root.
    pub router_root: PathBuf,
}

/// Local router authentication settings.
#[derive(Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LocalAuthConfig {
    /// Environment variable Codex uses for `X-Codex-Router-Token`.
    pub local_token_env: String,
}

/// Audit output settings.
#[derive(Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AuditConfig {
    /// Audit sink.
    pub sink: AuditSink,
}

/// Supported audit sinks.
#[derive(Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum AuditSink {
    /// Router-private file under the router root.
    File,
}

fn is_loopback_host(listen_host: &str) -> bool {
    matches!(listen_host, "127.0.0.1" | "localhost" | "::1")
}

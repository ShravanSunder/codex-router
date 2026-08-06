//! Already-resolved paths and endpoints consumed by the host runtime.

use std::net::SocketAddr;
use std::path::Path;
use std::path::PathBuf;

/// Router-root-owned coordination artifacts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostCoordinationPaths {
    operator_socket: PathBuf,
    instance_lock: PathBuf,
}

impl HostCoordinationPaths {
    /// Creates coordination paths previously resolved by the CLI.
    #[must_use]
    pub const fn new(operator_socket: PathBuf, instance_lock: PathBuf) -> Self {
        Self {
            operator_socket,
            instance_lock,
        }
    }

    /// Returns the private operator socket path.
    #[must_use]
    pub fn operator_socket(&self) -> &Path {
        &self.operator_socket
    }

    /// Returns the stable inert lock-artifact path.
    #[must_use]
    pub fn instance_lock(&self) -> &Path {
        &self.instance_lock
    }
}

/// Validated host inputs with router-owned and Codex-owned paths kept separate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostConfig {
    coordination_paths: HostCoordinationPaths,
    router_endpoint: SocketAddr,
    app_server_socket: PathBuf,
    managed_executable: PathBuf,
}

/// Already-resolved inputs for one host runtime.
pub struct HostConfigInputs {
    /// Router-root-owned coordination artifacts.
    pub coordination_paths: HostCoordinationPaths,
    /// Configured loopback router endpoint.
    pub router_endpoint: SocketAddr,
    /// Conventional socket derived from normal Codex home.
    pub app_server_socket: PathBuf,
    /// Managed Codex executable resolved by the adapter.
    pub managed_executable: PathBuf,
}

impl HostConfig {
    /// Creates a host configuration from values already resolved by the CLI.
    #[must_use]
    pub fn new(inputs: HostConfigInputs) -> Self {
        Self {
            coordination_paths: inputs.coordination_paths,
            router_endpoint: inputs.router_endpoint,
            app_server_socket: inputs.app_server_socket,
            managed_executable: inputs.managed_executable,
        }
    }

    /// Returns router-root-owned coordination paths.
    #[must_use]
    pub const fn coordination_paths(&self) -> &HostCoordinationPaths {
        &self.coordination_paths
    }

    /// Returns the configured loopback router endpoint.
    #[must_use]
    pub const fn router_endpoint(&self) -> SocketAddr {
        self.router_endpoint
    }

    /// Returns the conventional socket derived from normal Codex home.
    #[must_use]
    pub fn app_server_socket(&self) -> &Path {
        &self.app_server_socket
    }

    /// Returns the managed Codex executable resolved by the adapter.
    #[must_use]
    pub fn managed_executable(&self) -> &Path {
        &self.managed_executable
    }
}

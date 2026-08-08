//! Validated immutable host paths, endpoints, and deadline inputs.

use std::net::SocketAddr;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use thiserror::Error;

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
    deadlines: HostDeadlines,
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
    /// Bounded startup, probe, and operator deadlines.
    pub deadlines: HostDeadlines,
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
            deadlines: inputs.deadlines,
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

    /// Returns bounded runtime deadlines.
    #[must_use]
    pub const fn deadlines(&self) -> HostDeadlines {
        self.deadlines
    }
}

/// Inputs for validated host runtime deadlines.
pub struct HostDeadlineInputs {
    /// Compatible router startup convergence.
    pub router_start: Duration,
    /// Native app-server startup convergence.
    pub app_server_start: Duration,
    /// Remote Control convergence after native readiness.
    pub remote_control: Duration,
    /// Pre-spawn native endpoint ownership inspection.
    pub endpoint_inspection: Duration,
    /// One operator request/response exchange.
    pub operator_request: Duration,
}

/// Validated finite runtime bounds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HostDeadlines {
    router_start: Duration,
    app_server_start: Duration,
    remote_control: Duration,
    endpoint_inspection: Duration,
    operator_request: Duration,
}

impl HostDeadlines {
    /// Returns accepted production startup and control bounds.
    #[must_use]
    pub const fn production() -> Self {
        Self {
            router_start: Duration::from_secs(10),
            app_server_start: Duration::from_secs(10),
            remote_control: Duration::from_secs(10),
            endpoint_inspection: Duration::from_secs(2),
            operator_request: Duration::from_secs(40),
        }
    }

    /// Validates explicit shorter fixture bounds.
    pub fn new(inputs: HostDeadlineInputs) -> Result<Self, HostDeadlineError> {
        if [
            inputs.router_start,
            inputs.app_server_start,
            inputs.remote_control,
            inputs.endpoint_inspection,
            inputs.operator_request,
        ]
        .into_iter()
        .any(|duration| duration.is_zero())
        {
            return Err(HostDeadlineError::Zero);
        }
        Ok(Self {
            router_start: inputs.router_start,
            app_server_start: inputs.app_server_start,
            remote_control: inputs.remote_control,
            endpoint_inspection: inputs.endpoint_inspection,
            operator_request: inputs.operator_request,
        })
    }

    /// Returns the composed router + native + Remote Control startup bound.
    #[must_use]
    pub fn startup_total(self) -> Duration {
        self.router_start
            .saturating_add(self.app_server_start)
            .saturating_add(self.remote_control)
    }

    pub(crate) const fn router_start(self) -> Duration {
        self.router_start
    }

    pub(crate) const fn app_server_start(self) -> Duration {
        self.app_server_start
    }

    pub(crate) const fn remote_control(self) -> Duration {
        self.remote_control
    }

    pub(crate) const fn endpoint_inspection(self) -> Duration {
        self.endpoint_inspection
    }

    pub(crate) const fn operator_request(self) -> Duration {
        self.operator_request
    }
}

/// Invalid runtime deadline configuration.
#[derive(Debug, Error)]
pub enum HostDeadlineError {
    /// Every host deadline must be finite and positive.
    #[error("host runtime deadlines must be greater than zero")]
    Zero,
}

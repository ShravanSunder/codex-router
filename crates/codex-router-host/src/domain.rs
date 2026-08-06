//! Typed lifecycle dimensions and derived hosted readiness.

use serde::Deserialize;
use serde::Serialize;

/// Lifecycle operation visible in status and terminal results.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HostOperation {
    /// Foreground startup.
    Start,
    /// Read-only status observation.
    Status,
    /// Explicit app-server restart.
    RestartAppServer,
    /// Conditional managed Codex update.
    UpdateCodex,
    /// Explicit owned-router restart.
    RestartRouter,
}

/// Current mutually exclusive host lifecycle phase.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HostPhase {
    /// Initial convergence.
    Starting,
    /// Event-driven steady operation.
    Steady,
    /// One serialized lifecycle mutation.
    Mutating {
        /// Operation holding mutation ownership.
        operation: HostOperation,
        /// Low-cardinality mutation phase.
        phase: String,
    },
    /// Foreground shutdown with no replacement permitted.
    Stopping,
}

/// Router reachability and ownership without a child handle.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RouterCondition {
    /// Compatible router reached but not owned by this host.
    ExternalReachable,
    /// Compatible retained router child reached.
    OwnedReachable,
    /// Router is starting or restarting.
    OwnedTransitioning,
    /// No compatible router is currently reachable.
    Unavailable,
}

impl RouterCondition {
    const fn is_reachable(self) -> bool {
        matches!(self, Self::ExternalReachable | Self::OwnedReachable)
    }
}

/// Native app-server condition without embedding process authority.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AppServerCondition {
    /// Startup probe sequence is in progress.
    Starting,
    /// Native endpoint completed initialization.
    NativeReady {
        /// Version reported by native initialize.
        running_version: String,
    },
    /// Exact retained child is stopping.
    Stopping,
    /// Upstream shutdown timed out and remains retained.
    ShutdownTimedOut,
    /// No app-server child or native endpoint is present.
    Absent,
    /// Startup or lifecycle convergence failed.
    Failed,
}

impl AppServerCondition {
    const fn is_native_ready(&self) -> bool {
        matches!(self, Self::NativeReady { .. })
    }
}

/// Short-lived Remote Control observation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteControlCondition {
    /// Upstream relay reports connected.
    Connected,
    /// Upstream relay remains in progress.
    Connecting,
    /// Upstream relay reports an error.
    Errored,
    /// Remote Control is disabled.
    Disabled,
    /// Native status could not be observed.
    Unavailable,
}

/// Derived hosted usability without changing lifecycle state.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HostedReadiness {
    /// Router, native app-server, and Remote Control are ready.
    Ready,
    /// Local operation is ready while Remote Control is degraded.
    LocalReadyRemoteDegraded,
    /// Router or native app-server is unavailable.
    Unavailable,
}

/// One-attempt automatic crash-recovery budget.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryBudget {
    /// The steady-state automatic recovery attempt remains available.
    Available,
    /// The attempt was consumed by an unexpected app-server exit.
    Consumed,
}

/// Running-versus-installed managed executable comparison.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutableRelation {
    /// Running and installed content identities match.
    Match,
    /// Installed content differs from the running child identity.
    Drift,
    /// One identity could not be resolved.
    Unknown,
}

/// Low-cardinality completed lifecycle result.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleOutcomeClassification {
    /// Requested terminal condition completed.
    Succeeded,
    /// Requested terminal condition failed.
    Failed,
    /// A graceful stop required upstream force escalation.
    Forced,
    /// A retained child remained after the upstream shutdown bound.
    TimedOut,
    /// A mutation was rejected because another mutation owns serialization.
    Busy,
}

/// Most recent completed lifecycle result in this host lifetime.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LifecycleOutcome {
    /// Completed operation.
    pub operation: HostOperation,
    /// Terminal classification.
    pub classification: LifecycleOutcomeClassification,
}

/// Orthogonal live status returned through the operator boundary.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HostSnapshot {
    phase: HostPhase,
    router: RouterCondition,
    app_server: AppServerCondition,
    remote_control: RemoteControlCondition,
    executable_relation: ExecutableRelation,
    recovery_budget: RecoveryBudget,
    last_lifecycle_outcome: Option<LifecycleOutcome>,
}

/// Independently owned dimensions captured in one host snapshot.
pub struct HostSnapshotDimensions {
    /// Current serialized lifecycle phase.
    pub phase: HostPhase,
    /// Router reachability and ownership.
    pub router: RouterCondition,
    /// Native app-server condition.
    pub app_server: AppServerCondition,
    /// Short-lived Remote Control observation.
    pub remote_control: RemoteControlCondition,
    /// Running-versus-installed executable relation.
    pub executable_relation: ExecutableRelation,
    /// One-attempt automatic recovery budget.
    pub recovery_budget: RecoveryBudget,
    /// Most recent completed lifecycle outcome in this host lifetime.
    pub last_lifecycle_outcome: Option<LifecycleOutcome>,
}

impl HostSnapshot {
    /// Creates a snapshot from independently owned runtime observations.
    #[must_use]
    pub fn new(dimensions: HostSnapshotDimensions) -> Self {
        Self {
            phase: dimensions.phase,
            router: dimensions.router,
            app_server: dimensions.app_server,
            remote_control: dimensions.remote_control,
            executable_relation: dimensions.executable_relation,
            recovery_budget: dimensions.recovery_budget,
            last_lifecycle_outcome: dimensions.last_lifecycle_outcome,
        }
    }

    /// Derives overall usability without mutating any component dimension.
    #[must_use]
    pub const fn hosted_readiness(&self) -> HostedReadiness {
        if !self.router.is_reachable() || !self.app_server.is_native_ready() {
            HostedReadiness::Unavailable
        } else if matches!(self.remote_control, RemoteControlCondition::Connected) {
            HostedReadiness::Ready
        } else {
            HostedReadiness::LocalReadyRemoteDegraded
        }
    }

    /// Returns the automatic crash-recovery budget.
    #[must_use]
    pub const fn recovery_budget(&self) -> RecoveryBudget {
        self.recovery_budget
    }
}

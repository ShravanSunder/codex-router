//! Lifecycle-owner retained handles and transition bookkeeping.

use super::*;

pub(super) const fn restart_lifecycle_classification(
    succeeded: bool,
    shutdown_outcome: Option<crate::ShutdownOutcome>,
) -> LifecycleOutcomeClassification {
    match shutdown_outcome {
        Some(crate::ShutdownOutcome::Killed) => LifecycleOutcomeClassification::Forced,
        Some(crate::ShutdownOutcome::TimedOutStillRunning) => {
            LifecycleOutcomeClassification::TimedOut
        }
        Some(crate::ShutdownOutcome::Graceful) | None if succeeded => {
            LifecycleOutcomeClassification::Succeeded
        }
        Some(crate::ShutdownOutcome::Graceful) | None => LifecycleOutcomeClassification::Failed,
    }
}

pub(super) struct RuntimeState {
    pub(super) phase: HostPhase,
    pub(super) router: RouterCondition,
    pub(super) router_ownership: RouterOwnership,
    pub(super) app_server: AppServerCondition,
    pub(super) remote_control: RemoteControlCondition,
    pub(super) executable_relation: ExecutableRelation,
    pub(super) router_executable_relation: crate::RouterExecutableRelation,
    pub(super) router_drift_logged: bool,
    pub(super) recovery_budget: RecoveryBudget,
    pub(super) last_lifecycle_outcome: Option<LifecycleOutcome>,
}

impl RuntimeState {
    pub(super) fn ready(router: RouterCondition, readiness: AppServerReadiness) -> Self {
        let mut state = Self {
            phase: HostPhase::Steady,
            router,
            router_ownership: if matches!(router, RouterCondition::ExternalReachable) {
                RouterOwnership::External
            } else {
                RouterOwnership::Owned
            },
            app_server: AppServerCondition::Starting,
            remote_control: RemoteControlCondition::Unavailable,
            executable_relation: ExecutableRelation::Match,
            router_executable_relation: crate::RouterExecutableRelation::Unknown {
                reason: "startup observation pending".to_owned(),
            },
            router_drift_logged: false,
            recovery_budget: RecoveryBudget::Available,
            last_lifecycle_outcome: None,
        };
        state.apply_readiness(readiness);
        state
    }

    pub(super) fn apply_readiness(&mut self, readiness: AppServerReadiness) {
        match readiness {
            AppServerReadiness::Ready { running_version } => {
                self.app_server = AppServerCondition::NativeReady { running_version };
                self.remote_control = RemoteControlCondition::Connected;
            }
            AppServerReadiness::LocalReadyRemoteDegraded {
                running_version,
                remote_control,
            } => {
                self.app_server = AppServerCondition::NativeReady { running_version };
                self.remote_control = remote_control;
            }
        }
    }

    pub(super) fn snapshot(&self) -> HostSnapshot {
        HostSnapshot::new(HostSnapshotDimensions {
            phase: self.phase.clone(),
            router: self.router,
            app_server: self.app_server.clone(),
            remote_control: self.remote_control,
            remote_control_identity: None,
            executable_relation: self.executable_relation,
            router_executable_relation: self.router_executable_relation.clone(),
            recovery_budget: self.recovery_budget,
            last_lifecycle_outcome: self.last_lifecycle_outcome.clone(),
        })
    }

    pub(super) fn observe_router_executable(
        &mut self,
        relation: crate::RouterExecutableRelation,
    ) -> bool {
        let warning = collaboration_protocol::router_build_warning(&relation);
        let should_log = if let Some(warning) = warning
            && !self.router_drift_logged
        {
            tracing::warn!(router_build_warning = %warning, "Router executable drift detected");
            self.router_drift_logged = true;
            true
        } else {
            false
        };
        self.router_executable_relation = relation;
        should_log
    }

    pub(super) fn record_lifecycle(
        &self,
        operation: HostOperation,
        result: &'static str,
        duration: std::time::Duration,
    ) {
        crate::lifecycle_telemetry::record_lifecycle(
            operation,
            result,
            duration,
            self.router,
            self.snapshot().hosted_readiness(),
            self.recovery_budget,
            self.executable_relation,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restart_classification_preserves_forced_and_timed_out_shutdowns() {
        assert_eq!(
            restart_lifecycle_classification(true, Some(crate::ShutdownOutcome::Killed)),
            LifecycleOutcomeClassification::Forced
        );
        assert_eq!(
            restart_lifecycle_classification(
                false,
                Some(crate::ShutdownOutcome::TimedOutStillRunning),
            ),
            LifecycleOutcomeClassification::TimedOut
        );
    }

    #[test]
    fn router_drift_warns_once_for_the_host_lifetime() {
        let mut state = RuntimeState::ready(
            RouterCondition::ExternalReachable,
            crate::AppServerReadiness::Ready {
                running_version: "1.2.3".to_owned(),
            },
        );
        let drift = crate::RouterExecutableRelation::Drift {
            running_version: "0.1.36".to_owned(),
            installed_version: Some("0.1.37".to_owned()),
        };
        assert!(state.observe_router_executable(drift.clone()));
        assert!(!state.observe_router_executable(drift));
    }
}

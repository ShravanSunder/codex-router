//! Lifecycle-owner retained handles and transition bookkeeping.

use super::*;

pub(super) const fn restart_lifecycle_classification(
    succeeded: bool,
    shutdown_outcome: Option<crate::ShutdownOutcome>,
) -> LifecycleOutcomeClassification {
    match shutdown_outcome {
        Some(crate::ShutdownOutcome::Forced) => LifecycleOutcomeClassification::Forced,
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
            executable_relation: self.executable_relation,
            recovery_budget: self.recovery_budget,
            last_lifecycle_outcome: self.last_lifecycle_outcome.clone(),
        })
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
            restart_lifecycle_classification(true, Some(crate::ShutdownOutcome::Forced)),
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
}

use super::runtime_serving::ConnectionFailurePolicy;
use super::*;

/// The actual reason the owner stopped polling its granted listener.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LoopbackServingStopCause {
    DeactivationRequested,
    ConnectionLimit,
    ServingFailure,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum ServingOutcomeObservation {
    Unobserved,
    Observed,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum ActorDrainProgress {
    DatabaseWrites,
    SessionAffinity,
    Maintenance,
    Completed,
}

/// Created only when the accept loop returns; retains every acquired response owner.
pub struct StoppedLoopbackServing {
    pub(super) handled_connections: usize,
    pub(super) handlers: JoinSet<Result<(), LoopbackRouterRuntimeError>>,
    pub(super) first_connection_error: Option<LoopbackRouterRuntimeError>,
    pub(super) accept_error: Option<LoopbackRouterRuntimeError>,
    pub(super) session_shutdown: CancellationToken,
    pub(super) affinity_record_tasks: TaskTracker,
    pub(super) connection_failure_policy: ConnectionFailurePolicy,
    pub(super) stop_cause: LoopbackServingStopCause,
    pub(super) outcome_observation: ServingOutcomeObservation,
    pub(super) actor_progress: ActorDrainProgress,
    pub(super) actor_join_error: Option<LoopbackRouterRuntimeError>,
}
impl StoppedLoopbackServing {
    #[must_use]
    pub fn stop_cause(&self) -> LoopbackServingStopCause {
        self.stop_cause
    }

    /// Dropping this wait retains the JoinSet, side-effect tracker and original errors.
    /// It neither closes renewal admission nor stops database actors.
    pub async fn drain_responses(&mut self, core: &LoopbackRouterRuntime) {
        if self.first_connection_error.is_some()
            || self.accept_error.is_some()
            || self.stop_cause == LoopbackServingStopCause::DeactivationRequested
        {
            self.session_shutdown.cancel();
        }
        while let Some(joined) = self.handlers.join_next().await {
            core.record_owned_connection_result(
                &mut self.first_connection_error,
                Some(joined),
                self.connection_failure_policy,
            );
        }
        self.affinity_record_tasks.close();
        self.affinity_record_tasks.wait().await;
    }

    /// Retains each real join/error before another await; re-entry resumes the same actors.
    pub async fn drain_actor_work(&mut self, core: &LoopbackRouterRuntime) {
        core.db_write_actor.request_shutdown();
        core.maintenance_actor.request_shutdown();
        if self.actor_progress == ActorDrainProgress::DatabaseWrites {
            let result = core.db_write_actor.wait_main_completed().await;
            self.retain_actor_failure(result, LoopbackActorTask::DatabaseWrites);
            self.actor_progress = ActorDrainProgress::SessionAffinity;
        }
        if self.actor_progress == ActorDrainProgress::SessionAffinity {
            let result = core.db_write_actor.wait_session_affinity_completed().await;
            self.retain_actor_failure(result, LoopbackActorTask::SessionAffinity);
            self.actor_progress = ActorDrainProgress::Maintenance;
        }
        if self.actor_progress == ActorDrainProgress::Maintenance {
            let result = core.maintenance_actor.wait_until_completed().await;
            self.retain_actor_failure(result, LoopbackActorTask::Maintenance);
            self.actor_progress = ActorDrainProgress::Completed;
        }
    }

    fn retain_actor_failure(&mut self, result: Result<(), JoinError>, actor: LoopbackActorTask) {
        if let Err(source) = result
            && self.actor_join_error.is_none()
        {
            self.actor_join_error = Some(LoopbackRouterRuntimeError::ActorJoin { actor, source });
        }
    }

    pub fn take_actor_join_error(&mut self) -> Option<LoopbackRouterRuntimeError> {
        if self.actor_progress == ActorDrainProgress::Completed {
            self.actor_join_error.take()
        } else {
            None
        }
    }

    /// Exposes the original result once, only after real response owners are terminal.
    pub fn take_serving_result(&mut self) -> Option<Result<usize, LoopbackRouterRuntimeError>> {
        if self.actor_progress != ActorDrainProgress::Completed
            || self.outcome_observation == ServingOutcomeObservation::Observed
            || !self.handlers.is_empty()
            || !self.affinity_record_tasks.is_closed()
            || !self.affinity_record_tasks.is_empty()
        {
            return None;
        }
        self.outcome_observation = ServingOutcomeObservation::Observed;
        Some(self.take_recorded_outcome())
    }

    fn take_recorded_outcome(&mut self) -> Result<usize, LoopbackRouterRuntimeError> {
        if let Some(error) = self.accept_error.take() {
            return Err(error);
        }
        match self.first_connection_error.take() {
            Some(error) => Err(error),
            None => Ok(self.handled_connections),
        }
    }
}
impl LoopbackRouterRuntime {
    /// Requests and joins the runtime-owned database and maintenance actors.
    pub async fn shutdown(&self) {
        self.db_write_actor.shutdown().await;
        self.maintenance_actor.shutdown().await;
    }

    // Existing bounded core observers remain distinct from the role's true completion path.
    pub(super) async fn finish_serving(
        &self,
        mut stopped: StoppedLoopbackServing,
    ) -> Result<usize, LoopbackRouterRuntimeError> {
        stopped.drain_responses(self).await;
        if !self
            .credential_factory
            .drain_refresh_tasks(self.credential_refresh_shutdown_drain)
            .await
        {
            tracing::warn!(
                "credential refresh drain timed out; unresolved claims remain authoritative"
            );
        }
        self.shutdown().await;
        stopped.take_recorded_outcome()
    }
}

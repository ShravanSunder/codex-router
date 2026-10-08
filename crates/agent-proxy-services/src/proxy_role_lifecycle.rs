use crate::{ProxyActivationError, ProxyRoleRuntime};
use codex_router_proxy::server::{LoopbackServingStopCause, StoppedLoopbackServing};
use tokio::task::JoinHandle;

pub(crate) enum ProxyServingLifecycle {
    Starting,
    Accepting(JoinHandle<StoppedLoopbackServing>),
    Stopped(StoppedLoopbackServing),
    Drained {
        stop_cause: LoopbackServingStopCause,
        serving_result: Option<Result<usize, ProxyActivationError>>,
        actor_join_error: Option<codex_router_proxy::server::LoopbackRouterRuntimeError>,
    },
    Failed {
        accepting_error: Option<tokio::task::JoinError>,
    },
}

/// Evidence constructed only after retaining the actual stopped acceptance owner.
#[derive(Clone, Copy, Debug)]
pub struct ProxyDeactivationReceipt {
    stop_cause: LoopbackServingStopCause,
}
impl ProxyDeactivationReceipt {
    #[must_use]
    pub fn stop_cause(&self) -> LoopbackServingStopCause {
        self.stop_cause
    }
}

/// All acquired work is terminal and joined. Original serving and worker outcomes stay separate.
#[derive(Debug)]
pub struct ProxyDrainCompletion {
    serving_result: Option<Result<usize, ProxyActivationError>>,
    worker_join_error: Option<tokio::task::JoinError>,
    actor_join_error: Option<codex_router_proxy::server::LoopbackRouterRuntimeError>,
}
impl ProxyDrainCompletion {
    /// The original serving outcome is moved once; None means a prior caller observed it.
    #[must_use]
    pub fn serving_result(&self) -> Option<&Result<usize, ProxyActivationError>> {
        self.serving_result.as_ref()
    }
    #[must_use]
    pub fn worker_join_error(&self) -> Option<&tokio::task::JoinError> {
        self.worker_join_error.as_ref()
    }
    #[must_use]
    pub fn actor_join_error(
        &self,
    ) -> Option<&codex_router_proxy::server::LoopbackRouterRuntimeError> {
        self.actor_join_error.as_ref()
    }

    /// Preserves the existing serving-error precedence for CLI wait/shutdown callers.
    pub fn into_serving_result(self) -> Option<Result<usize, ProxyActivationError>> {
        match (
            self.serving_result,
            self.actor_join_error,
            self.worker_join_error,
        ) {
            (Some(Err(error)), _, _) => Some(Err(error)),
            (_, Some(error), _) => Some(Err(ProxyActivationError::Core(error))),
            (_, _, Some(error)) => Some(Err(ProxyActivationError::ServingTask(error))),
            (result, None, None) => result,
        }
    }
}
impl ProxyRoleRuntime {
    async fn observe_stopped(&mut self) -> Result<(), ProxyActivationError> {
        if let ProxyServingLifecycle::Accepting(task) = &mut self.lifecycle {
            let joined = task.await;
            match joined {
                Ok(stopped) => self.lifecycle = ProxyServingLifecycle::Stopped(stopped),
                Err(error) => {
                    self.lifecycle = ProxyServingLifecycle::Failed {
                        accepting_error: Some(error),
                    };
                }
            }
        }
        if matches!(self.lifecycle, ProxyServingLifecycle::Failed { .. }) {
            return Err(self.finish_accepting_failure().await);
        }
        match self.lifecycle {
            ProxyServingLifecycle::Stopped(_) | ProxyServingLifecycle::Drained { .. } => Ok(()),
            _ => Err(ProxyActivationError::LifecycleUnavailable),
        }
    }

    #[cfg(test)]
    pub(crate) async fn observe_stopped_for_test(&mut self) -> Result<(), ProxyActivationError> {
        self.observe_stopped().await
    }

    /// Stops acceptance and retains the real stopped owner without waiting for held renewals.
    pub async fn deactivate(&mut self) -> Result<ProxyDeactivationReceipt, ProxyActivationError> {
        self.stop.cancel();
        self.observe_stopped().await?;
        let stop_cause = match &self.lifecycle {
            ProxyServingLifecycle::Stopped(stopped) => stopped.stop_cause(),
            ProxyServingLifecycle::Drained { stop_cause, .. } => *stop_cause,
            _ => return Err(ProxyActivationError::LifecycleUnavailable),
        };
        Ok(ProxyDeactivationReceipt { stop_cause })
    }

    fn request_worker_stops(&self) {
        self.credential_refresh_task_supervisor().close_admission();
        if let Some(worker) = &self.quota {
            worker.request_stop();
        }
        if let Some(worker) = &self.upkeep {
            worker.request_stop();
        }
        if let Some(watcher) = &self.watcher {
            watcher.request_stop();
        }
    }

    async fn join_workers(&mut self) {
        if let Some(worker) = self.quota.as_mut() {
            let result = worker.join_stopped().await;
            self.quota = None;
            if self.worker_join_error.is_none() {
                self.worker_join_error = result.err();
            }
        }
        if let Some(worker) = self.upkeep.as_mut() {
            let result = worker.join_stopped().await;
            self.upkeep = None;
            if self.worker_join_error.is_none() {
                self.worker_join_error = result.err();
            }
        }
        if let Some(watcher) = self.watcher.as_mut() {
            let result = watcher.join_stopped().await;
            self.watcher = None;
            if self.worker_join_error.is_none() {
                self.worker_join_error = result.err();
            }
        }
    }

    pub(crate) async fn cleanup_acquired(&mut self) {
        self.stop.cancel();
        self.request_worker_stops();
        self.join_workers().await;
        self.credential_refresh_task_supervisor()
            .wait_until_completed()
            .await;
        self.core.shutdown().await;
    }

    async fn finish_accepting_failure(&mut self) -> ProxyActivationError {
        if matches!(
            self.lifecycle,
            ProxyServingLifecycle::Failed {
                accepting_error: Some(_)
            }
        ) {
            // The same failure and acquired handles stay stored across a cancelled cleanup wait.
            // Failure cleanup never constructs stopped-acceptance or true-drain evidence.
            self.cleanup_acquired().await;
        }
        match &mut self.lifecycle {
            ProxyServingLifecycle::Failed { accepting_error } => accepting_error.take().map_or(
                ProxyActivationError::LifecycleUnavailable,
                ProxyActivationError::ServingTask,
            ),
            _ => ProxyActivationError::LifecycleUnavailable,
        }
    }

    /// Requires an observed stopped acceptance owner. No business-library timeout means Drained.
    pub async fn wait_drained(&mut self) -> Result<ProxyDrainCompletion, ProxyActivationError> {
        if matches!(self.lifecycle, ProxyServingLifecycle::Failed { .. }) {
            return Err(self.finish_accepting_failure().await);
        }
        let cause = match &self.lifecycle {
            ProxyServingLifecycle::Stopped(stopped) => stopped.stop_cause(),
            ProxyServingLifecycle::Drained { .. } => return Ok(self.take_completion()),
            _ => return Err(ProxyActivationError::LifecycleUnavailable),
        };
        if cause != LoopbackServingStopCause::DeactivationRequested
            && let ProxyServingLifecycle::Stopped(stopped) = &mut self.lifecycle
        {
            // A naturally accepted finite request may not have polled its renewal yet.
            stopped.drain_responses(&self.core).await;
        }
        self.request_worker_stops();
        self.join_workers().await;
        self.credential_refresh_task_supervisor()
            .wait_until_completed()
            .await;
        if let ProxyServingLifecycle::Stopped(stopped) = &mut self.lifecycle {
            stopped.drain_responses(&self.core).await;
        }
        if let ProxyServingLifecycle::Stopped(stopped) = &mut self.lifecycle {
            stopped.drain_actor_work(&self.core).await;
        }
        let serving_result = match &mut self.lifecycle {
            ProxyServingLifecycle::Stopped(stopped) => stopped.take_serving_result(),
            _ => None,
        }
        .ok_or(ProxyActivationError::LifecycleUnavailable)?;
        let actor_join_error = match &mut self.lifecycle {
            ProxyServingLifecycle::Stopped(stopped) => stopped.take_actor_join_error(),
            _ => None,
        };
        self.lifecycle = ProxyServingLifecycle::Drained {
            stop_cause: cause,
            serving_result: Some(serving_result.map_err(Into::into)),
            actor_join_error,
        };
        Ok(self.take_completion())
    }

    fn take_completion(&mut self) -> ProxyDrainCompletion {
        let serving_result = match &mut self.lifecycle {
            ProxyServingLifecycle::Drained { serving_result, .. } => serving_result.take(),
            _ => None,
        };
        let actor_join_error = match &mut self.lifecycle {
            ProxyServingLifecycle::Drained {
                actor_join_error, ..
            } => actor_join_error.take(),
            _ => None,
        };
        ProxyDrainCompletion {
            serving_result,
            actor_join_error,
            worker_join_error: self.worker_join_error.take(),
        }
    }

    pub async fn wait_serving(&mut self) -> Result<usize, ProxyActivationError> {
        self.observe_stopped().await?;
        self.wait_drained()
            .await?
            .into_serving_result()
            .unwrap_or(Ok(0))
    }

    /// Both observations retain the same handles if either wait is dropped and resumed.
    pub async fn shutdown(&mut self) -> Result<(), ProxyActivationError> {
        self.deactivate().await?;
        self.wait_drained()
            .await?
            .into_serving_result()
            .unwrap_or(Ok(0))
            .map(|_| ())
    }
}

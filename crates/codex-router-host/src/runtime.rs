//! Single-owner event loop for foreground host lifecycle authority.

use std::sync::Arc;

use thiserror::Error;
use tokio::sync::Semaphore;
use tokio::sync::mpsc;

use crate::AppServerChild;
use crate::AppServerCondition;
use crate::AppServerEndpointError;
use crate::AppServerLaunchPlan;
use crate::AppServerReadiness;
use crate::AppServerReadinessError;
use crate::AppServerShutdownError;
use crate::ChildCommandSpec;
use crate::ExecutableRelation;
use crate::HostConfig;
use crate::HostInstance;
use crate::HostOperation;
use crate::HostPhase;
use crate::HostSnapshot;
use crate::HostSnapshotDimensions;
use crate::InstanceAcquireError;
use crate::LifecycleOutcome;
use crate::LifecycleOutcomeClassification;
use crate::OperatorRequest;
use crate::ProcessGroupChild;
use crate::ProcessGroupError;
use crate::RecoveryBudget;
use crate::RemoteControlCondition;
use crate::RouterChild;
use crate::RouterCondition;
use crate::RouterOwnership;
use crate::RouterProbeError;
use crate::RouterProbeResult;
use crate::RouterShutdownError;
use crate::TerminalClassification;
use crate::probe_router;
use crate::require_unowned_app_server_endpoint;

mod lifecycle;
mod operator;
mod startup;
mod state;
mod status;
mod update_flow;

use state::RuntimeState;
use state::restart_lifecycle_classification;

const OPERATOR_QUEUE_CAPACITY: usize = 8;
const OPERATOR_CONNECTION_LIMIT: usize = 8;

/// Runtime-owned launch inputs that can be reused for bounded lifecycle work.
pub struct HostDependenciesInputs {
    /// Optional router command used only when no compatible external router exists.
    pub router_command: Option<ChildCommandSpec>,
    /// Exact managed app-server launch projection.
    pub app_server: AppServerLaunchPlan,
}

/// Validated runtime dependency projections.
pub struct HostDependencies {
    router_command: Option<ChildCommandSpec>,
    app_server: AppServerLaunchPlan,
    update_deadlines: crate::UpdateDeadlines,
    replacement_command: Option<ChildCommandSpec>,
    pre_exec_telemetry: Option<Arc<dyn PreExecTelemetry>>,
}

impl HostDependencies {
    /// Captures already-resolved runtime launch projections.
    #[must_use]
    pub fn new(inputs: HostDependenciesInputs) -> Self {
        Self {
            router_command: inputs.router_command,
            app_server: inputs.app_server,
            update_deadlines: crate::UpdateDeadlines::production(),
            replacement_command: None,
            pre_exec_telemetry: None,
        }
    }

    /// Replaces production update bounds for deterministic fixtures.
    #[must_use]
    pub const fn with_update_deadlines(mut self, deadlines: crate::UpdateDeadlines) -> Self {
        self.update_deadlines = deadlines;
        self
    }

    /// Supplies the exact current `codex-router host` foreground re-exec command.
    #[must_use]
    pub fn with_replacement_command(mut self, command: ChildCommandSpec) -> Self {
        self.replacement_command = Some(command);
        self
    }

    /// Supplies the CLI-owned provider flush adapter for changed-update exec.
    #[must_use]
    pub fn with_pre_exec_telemetry(mut self, telemetry: Arc<dyn PreExecTelemetry>) -> Self {
        self.pre_exec_telemetry = Some(telemetry);
        self
    }
}

/// Provider-specific bounded blocking work run only at the terminal exec edge.
pub trait PreExecTelemetry: Send + Sync + 'static {
    /// Flushes and shuts down existing providers without exposing runtime authority.
    fn flush_and_shutdown(&self);
}

/// Normal foreground runtime terminal reason.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostExit {
    /// SIGINT, SIGTERM, or SIGHUP requested foreground shutdown.
    Signal,
}

/// Startup or owner-loop failure.
#[derive(Debug, Error)]
pub enum HostError {
    /// Singleton authority or operator socket publication failed.
    #[error(transparent)]
    Instance(#[from] InstanceAcquireError),
    /// Router compatibility observation failed.
    #[error(transparent)]
    RouterProbe(#[from] RouterProbeError),
    /// No compatible router was available and no owned launch command was supplied.
    #[error("no compatible Codex router is available")]
    RouterUnavailable,
    /// An existing listener requires unsupported local authentication.
    #[error("the configured Codex router requires unsupported local authentication")]
    RouterAuthenticationRequired,
    /// The configured listener does not satisfy the router compatibility contract.
    #[error("the configured listener is not a compatible Codex router")]
    RouterIncompatible,
    /// Retained child spawn or observation failed.
    #[error(transparent)]
    Process(#[from] ProcessGroupError),
    /// Native app-server endpoint ownership inspection failed.
    #[error(transparent)]
    AppServerEndpoint(#[from] AppServerEndpointError),
    /// Managed app-server readiness failed.
    #[error(transparent)]
    AppServerReadiness(#[from] AppServerReadinessError),
    /// Retained app-server shutdown failed.
    #[error(transparent)]
    AppServerShutdown(#[from] AppServerShutdownError),
    /// Retained owned-router shutdown failed.
    #[error(transparent)]
    RouterShutdown(#[from] RouterShutdownError),
    /// Unix signal registration failed.
    #[error("failed registering foreground host signals: {0}")]
    Signal(#[source] std::io::Error),
    /// Same-process foreground replacement returned instead of replacing the image.
    #[error("failed replacing foreground shared Codex host: {0}")]
    Exec(#[source] std::io::Error),
}

/// Foreground host composition and its single lifecycle owner task.
pub struct HostRuntime;

impl HostRuntime {
    /// Acquires authority, converges startup, then owns all retained handles.
    pub async fn run(
        config: HostConfig,
        dependencies: HostDependencies,
    ) -> Result<HostExit, HostError> {
        let startup_started_at = tokio::time::Instant::now();
        let instance = HostInstance::acquire(config.coordination_paths().clone())?;
        Self::run_owned(config, dependencies, instance, startup_started_at).await
    }

    /// Consumes and validates singleton authority inherited across changed-update exec.
    pub async fn run_inherited(
        config: HostConfig,
        dependencies: HostDependencies,
        marker: &std::ffi::OsStr,
    ) -> Result<HostExit, HostError> {
        let startup_started_at = tokio::time::Instant::now();
        let instance =
            HostInstance::acquire_inherited(config.coordination_paths().clone(), marker)?;
        Self::run_owned(config, dependencies, instance, startup_started_at).await
    }

    /// Starts lifecycle convergence with singleton authority already acquired.
    pub async fn run_acquired(
        config: HostConfig,
        dependencies: HostDependencies,
        instance: HostInstance,
    ) -> Result<HostExit, HostError> {
        Self::run_owned(config, dependencies, instance, tokio::time::Instant::now()).await
    }

    async fn run_owned(
        config: HostConfig,
        dependencies: HostDependencies,
        instance: HostInstance,
        startup_started_at: tokio::time::Instant,
    ) -> Result<HostExit, HostError> {
        let mut interrupt =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
                .map_err(HostError::Signal)?;
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .map_err(HostError::Signal)?;
        let mut hangup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())
            .map_err(HostError::Signal)?;
        let (router_condition, mut router_child) =
            startup::start_router(&config, dependencies.router_command.as_ref()).await?;
        if let Err(endpoint_error) = require_unowned_app_server_endpoint(
            config.app_server_socket(),
            config.deadlines().endpoint_inspection(),
        )
        .await
        {
            startup::shutdown_owned_router_after_startup_failure(&mut router_child).await?;
            return Err(HostError::AppServerEndpoint(endpoint_error));
        }
        let (app_server, readiness) =
            match startup::start_app_server(&config, dependencies.app_server.clone()).await {
                Ok(started) => started,
                Err(error) => {
                    startup::shutdown_owned_router_after_startup_failure(&mut router_child).await?;
                    return Err(error);
                }
            };
        let mut state = RuntimeState::ready(router_condition, readiness);
        state.record_lifecycle(
            HostOperation::Start,
            "succeeded",
            startup_started_at.elapsed(),
        );

        let (operator_sender, mut operator_receiver) =
            mpsc::channel::<operator::OperatorWork>(OPERATOR_QUEUE_CAPACITY);
        let mut connection_tasks = tokio::task::JoinSet::new();
        let connection_permits = Arc::new(Semaphore::new(OPERATOR_CONNECTION_LIMIT));
        let mut app_server = Some(app_server);
        let mut recovery = None::<lifecycle::RecoveryFuture>;
        let mut recovery_started_at = None::<tokio::time::Instant>;
        let mut active_restart = None::<operator::ActiveAppServerRestart>;
        let mut active_router_restart = None::<operator::ActiveRouterRestart>;
        let mut active_update = None::<operator::ActiveUpdate>;
        let mut active_update_activation = None::<operator::ActiveUpdateActivation>;
        let mut active_status = None::<operator::ActiveStatusObservation>;
        let mut pending_identity = None::<codex_router_codex::ExecutableIdentityTask>;
        let mut retained_updater = None::<ProcessGroupChild>;
        loop {
            tokio::select! {
                accepted = instance.listener().accept() => {
                    if let Ok((stream, _peer)) = accepted
                        && let Ok(permit) = Arc::clone(&connection_permits).try_acquire_owned()
                    {
                        operator::spawn_operator_connection(
                            &mut connection_tasks,
                            stream,
                            operator_sender.clone(),
                            config.deadlines().operator_request(),
                            permit,
                        );
                    }
                }
                Some(work) = operator_receiver.recv() => {
                    let update_drain_active = pending_identity.is_some()
                        || retained_updater.is_some()
                        || active_status.is_some();
                    operator::handle_operator_work(work, operator::OperatorRuntimeContext {
                        state: &mut state,
                        app_server: &mut app_server,
                        router_child: &mut router_child,
                        config: &config,
                        dependencies: &dependencies,
                        active_app_server_restart: &mut active_restart,
                        active_router_restart: &mut active_router_restart,
                        active_update: &mut active_update,
                        active_status: &mut active_status,
                        update_drain_active,
                    });
                }
                completed = connection_tasks.join_next(), if !connection_tasks.is_empty() => {
                    let _completed_connection = completed;
                }
                restart_completion = lifecycle::wait_for_active_restart(&mut active_restart), if active_restart.is_some() => {
                    let Some(active) = active_restart.take() else {
                        continue;
                    };
                    app_server = restart_completion.child;
                    let classification = if restart_completion.succeeded {
                        if let Some(readiness) = restart_completion.readiness {
                            state.apply_readiness(readiness);
                        }
                        state.recovery_budget = RecoveryBudget::Available;
                        state.last_lifecycle_outcome = Some(LifecycleOutcome {
                            operation: HostOperation::RestartAppServer,
                            classification: restart_lifecycle_classification(
                                true,
                                restart_completion.shutdown_outcome,
                            ),
                        });
                        TerminalClassification::Succeeded
                    } else {
                        state.app_server = if app_server.is_some() {
                            AppServerCondition::ShutdownTimedOut
                        } else {
                            AppServerCondition::Failed
                        };
                        state.remote_control = RemoteControlCondition::Unavailable;
                        state.last_lifecycle_outcome = Some(LifecycleOutcome {
                            operation: HostOperation::RestartAppServer,
                            classification: restart_lifecycle_classification(
                                false,
                                restart_completion.shutdown_outcome,
                            ),
                        });
                        TerminalClassification::Failed
                    };
                    state.phase = HostPhase::Steady;
                    state.record_lifecycle(
                        HostOperation::RestartAppServer,
                        if restart_completion.succeeded { "succeeded" } else { "failed" },
                        active.started_at.elapsed(),
                    );
                    operator::send_terminal_response(
                        active.response,
                        OperatorRequest::RestartAppServer,
                        classification,
                        state.snapshot(),
                        restart_completion.message,
                    );
                }
                router_restart_completion = lifecycle::wait_for_active_router_restart(&mut active_router_restart), if active_router_restart.is_some() => {
                    let Some(active) = active_router_restart.take() else {
                        continue;
                    };
                    router_child = router_restart_completion.child;
                    let classification = if router_restart_completion.succeeded {
                        state.router = RouterCondition::OwnedReachable;
                        state.last_lifecycle_outcome = Some(LifecycleOutcome {
                            operation: HostOperation::RestartRouter,
                            classification: LifecycleOutcomeClassification::Succeeded,
                        });
                        TerminalClassification::Succeeded
                    } else {
                        state.router = if router_child.is_some() {
                            RouterCondition::OwnedTransitioning
                        } else {
                            RouterCondition::Unavailable
                        };
                        state.last_lifecycle_outcome = Some(LifecycleOutcome {
                            operation: HostOperation::RestartRouter,
                            classification: LifecycleOutcomeClassification::Failed,
                        });
                        TerminalClassification::Failed
                    };
                    state.phase = HostPhase::Steady;
                    state.record_lifecycle(
                        HostOperation::RestartRouter,
                        if router_restart_completion.succeeded { "succeeded" } else { "failed" },
                        active.started_at.elapsed(),
                    );
                    operator::send_terminal_response(
                        active.response,
                        OperatorRequest::RestartRouter,
                        classification,
                        state.snapshot(),
                        router_restart_completion.message,
                    );
                }
                update_preparation = lifecycle::wait_for_active_update(&mut active_update), if active_update.is_some() => {
                    let Some(active) = active_update.take() else {
                        continue;
                    };
                    update_flow::apply_preparation(update_flow::PreparationContext {
                        preparation: update_preparation,
                        active,
                        state: &mut state,
                        dependencies: &dependencies,
                        app_server: &mut app_server,
                        router: &mut router_child,
                        activation: &mut active_update_activation,
                        pending_identity: &mut pending_identity,
                        retained_updater: &mut retained_updater,
                    });
                }
                activation_completion = lifecycle::wait_for_update_activation(&mut active_update_activation), if active_update_activation.is_some() => {
                    let Some(active) = active_update_activation.take() else {
                        continue;
                    };
                    update_flow::apply_activation(update_flow::ActivationContext {
                        completion: activation_completion,
                        active,
                        state: &mut state,
                        app_server: &mut app_server,
                        router: &mut router_child,
                        dependencies: &dependencies,
                        instance: &instance,
                    }).await?;
                }
                identity_result = lifecycle::wait_for_pending_identity(&mut pending_identity), if pending_identity.is_some() => {
                    let _identity_result = identity_result;
                    pending_identity = None;
                }
                updater_result = lifecycle::wait_for_retained_updater(&mut retained_updater), if retained_updater.is_some() => {
                    let _updater_result = updater_result;
                    retained_updater = None;
                }
                status_observation = lifecycle::wait_for_status_observation(&mut active_status), if active_status.is_some() => {
                    let Some(active) = active_status.take() else {
                        continue;
                    };
                    let (snapshot, status_identity) = status_observation.snapshot(&state);
                    if pending_identity.is_none() {
                        pending_identity = status_identity;
                    }
                    let classification = match snapshot.hosted_readiness() {
                        crate::HostedReadiness::Ready => TerminalClassification::Ready,
                        crate::HostedReadiness::LocalReadyRemoteDegraded => {
                            TerminalClassification::LocalReadyRemoteDegraded
                        }
                        crate::HostedReadiness::Unavailable => TerminalClassification::Unavailable,
                    };
                    for (request, response) in active.responses {
                        operator::send_terminal_response(
                            response,
                            request,
                            classification,
                            snapshot.clone(),
                            "shared Codex host status",
                        );
                    }
                }
                exit = lifecycle::wait_for_app_server_exit(&mut app_server), if app_server.is_some() && recovery.is_none() && active_restart.is_none() => {
                    let _exit_status = exit?;
                    app_server = None;
                    state.app_server = AppServerCondition::Absent;
                    state.remote_control = RemoteControlCondition::Unavailable;
                    if state.recovery_budget == RecoveryBudget::Available
                        && active_router_restart.is_none()
                        && active_update.is_none()
                    {
                        state.recovery_budget = RecoveryBudget::Consumed;
                        state.phase = HostPhase::Mutating {
                            operation: HostOperation::RestartAppServer,
                            phase: "automatic-recovery".to_owned(),
                        };
                        state.app_server = AppServerCondition::Starting;
                        recovery_started_at = Some(tokio::time::Instant::now());
                        recovery = Some(lifecycle::recovery_future(&config, dependencies.app_server.clone()));
                    } else {
                        state.last_lifecycle_outcome = Some(LifecycleOutcome {
                            operation: HostOperation::RestartAppServer,
                            classification: LifecycleOutcomeClassification::Failed,
                        });
                    }
                }
                recovery_result = lifecycle::wait_for_recovery(&mut recovery), if recovery.is_some() => {
                    recovery = None;
                    match recovery_result {
                        Ok((recovered_child, recovered_readiness)) => {
                            app_server = Some(recovered_child);
                            state.apply_readiness(recovered_readiness);
                            state.last_lifecycle_outcome = Some(LifecycleOutcome {
                                operation: HostOperation::RestartAppServer,
                                classification: LifecycleOutcomeClassification::Succeeded,
                            });
                        }
                        Err(_error) => {
                            state.app_server = AppServerCondition::Failed;
                            state.remote_control = RemoteControlCondition::Unavailable;
                            state.last_lifecycle_outcome = Some(LifecycleOutcome {
                                operation: HostOperation::RestartAppServer,
                                classification: LifecycleOutcomeClassification::Failed,
                            });
                        }
                    }
                    state.phase = HostPhase::Steady;
                    state.record_lifecycle(
                        HostOperation::RestartAppServer,
                        if matches!(
                            state.last_lifecycle_outcome,
                            Some(LifecycleOutcome {
                                classification: LifecycleOutcomeClassification::Succeeded,
                                ..
                            })
                        ) {
                            "succeeded"
                        } else {
                            "failed"
                        },
                        recovery_started_at
                            .take()
                            .map_or(std::time::Duration::ZERO, |started_at| started_at.elapsed()),
                    );
                }
                router_exit = lifecycle::wait_for_router_exit(&mut router_child), if router_child.is_some() && active_router_restart.is_none() => {
                    let _exit_status = router_exit?;
                    router_child = None;
                    state.router = RouterCondition::Unavailable;
                    state.last_lifecycle_outcome = Some(LifecycleOutcome {
                        operation: HostOperation::RestartRouter,
                        classification: LifecycleOutcomeClassification::Failed,
                    });
                }
                _ = interrupt.recv() => {
                    state.phase = HostPhase::Stopping;
                    lifecycle::settle_for_shutdown(lifecycle::ShutdownContext {
                        activation: &mut active_update_activation,
                        active_update: &mut active_update,
                        pending_identity: &mut pending_identity,
                        retained_updater: &mut retained_updater,
                        active_app_server_restart: &mut active_restart,
                        active_router_restart: &mut active_router_restart,
                        recovery: &mut recovery,
                        app_server: &mut app_server,
                        router: &mut router_child,
                    }).await?;
                    return Ok(HostExit::Signal);
                }
                _ = terminate.recv() => {
                    state.phase = HostPhase::Stopping;
                    lifecycle::settle_for_shutdown(lifecycle::ShutdownContext {
                        activation: &mut active_update_activation,
                        active_update: &mut active_update,
                        pending_identity: &mut pending_identity,
                        retained_updater: &mut retained_updater,
                        active_app_server_restart: &mut active_restart,
                        active_router_restart: &mut active_router_restart,
                        recovery: &mut recovery,
                        app_server: &mut app_server,
                        router: &mut router_child,
                    }).await?;
                    return Ok(HostExit::Signal);
                }
                _ = hangup.recv() => {
                    state.phase = HostPhase::Stopping;
                    lifecycle::settle_for_shutdown(lifecycle::ShutdownContext {
                        activation: &mut active_update_activation,
                        active_update: &mut active_update,
                        pending_identity: &mut pending_identity,
                        retained_updater: &mut retained_updater,
                        active_app_server_restart: &mut active_restart,
                        active_router_restart: &mut active_router_restart,
                        recovery: &mut recovery,
                        app_server: &mut app_server,
                        router: &mut router_child,
                    }).await?;
                    return Ok(HostExit::Signal);
                }
            }
        }
    }
}

//! Single-owner event loop for foreground host lifecycle authority.

use std::future::Future;
use std::pin::Pin;
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
use crate::ProcessGroupError;
use crate::RecoveryBudget;
use crate::RemoteControlCondition;
use crate::RouterChild;
use crate::RouterCondition;
use crate::RouterProbeError;
use crate::RouterProbeResult;
use crate::RouterShutdownError;
use crate::TerminalClassification;
use crate::probe_router;
use crate::require_unowned_app_server_endpoint;

mod operator;
mod startup;

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
}

impl HostDependencies {
    /// Captures already-resolved runtime launch projections.
    #[must_use]
    pub fn new(inputs: HostDependenciesInputs) -> Self {
        Self {
            router_command: inputs.router_command,
            app_server: inputs.app_server,
        }
    }
}

/// Normal foreground runtime terminal reason.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostExit {
    /// SIGINT, SIGTERM, or SIGHUP requested foreground shutdown.
    Signal,
    /// A retained router child exited, so the host cannot keep serving.
    OwnedRouterExited,
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
        let mut recovery = None::<RecoveryFuture>;
        let mut recovery_started_at = None::<tokio::time::Instant>;
        let mut active_restart = None::<operator::ActiveAppServerRestart>;
        let mut active_router_restart = None::<operator::ActiveRouterRestart>;
        let mut interrupt =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
                .map_err(HostError::Signal)?;
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .map_err(HostError::Signal)?;
        let mut hangup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())
            .map_err(HostError::Signal)?;

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
                    operator::handle_operator_work(work, operator::OperatorRuntimeContext {
                        state: &mut state,
                        app_server: &mut app_server,
                        router_child: &mut router_child,
                        config: &config,
                        dependencies: &dependencies,
                        active_app_server_restart: &mut active_restart,
                        active_router_restart: &mut active_router_restart,
                    });
                }
                completed = connection_tasks.join_next(), if !connection_tasks.is_empty() => {
                    let _completed_connection = completed;
                }
                restart_completion = wait_for_active_restart(&mut active_restart), if active_restart.is_some() => {
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
                            classification: LifecycleOutcomeClassification::Succeeded,
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
                            classification: LifecycleOutcomeClassification::Failed,
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
                router_restart_completion = wait_for_active_router_restart(&mut active_router_restart), if active_router_restart.is_some() => {
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
                exit = wait_for_app_server_exit(&mut app_server), if app_server.is_some() && recovery.is_none() && active_restart.is_none() => {
                    let _exit_status = exit?;
                    app_server = None;
                    state.app_server = AppServerCondition::Absent;
                    state.remote_control = RemoteControlCondition::Unavailable;
                    if state.recovery_budget == RecoveryBudget::Available && active_router_restart.is_none() {
                        state.recovery_budget = RecoveryBudget::Consumed;
                        state.phase = HostPhase::Mutating {
                            operation: HostOperation::RestartAppServer,
                            phase: "automatic-recovery".to_owned(),
                        };
                        state.app_server = AppServerCondition::Starting;
                        recovery_started_at = Some(tokio::time::Instant::now());
                        recovery = Some(recovery_future(&config, dependencies.app_server.clone()));
                    } else {
                        state.last_lifecycle_outcome = Some(LifecycleOutcome {
                            operation: HostOperation::RestartAppServer,
                            classification: LifecycleOutcomeClassification::Failed,
                        });
                    }
                }
                recovery_result = wait_for_recovery(&mut recovery), if recovery.is_some() => {
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
                router_exit = wait_for_router_exit(&mut router_child), if router_child.is_some() && active_router_restart.is_none() => {
                    let _exit_status = router_exit?;
                    state.router = RouterCondition::Unavailable;
                    state.phase = HostPhase::Stopping;
                    settle_active_restart_for_shutdown(&mut active_restart, &mut app_server).await;
                    settle_active_router_restart_for_shutdown(&mut active_router_restart, &mut router_child).await;
                    settle_recovery_for_shutdown(&mut recovery, &mut app_server).await;
                    shutdown_retained_children(&mut app_server, &mut router_child).await?;
                    return Ok(HostExit::OwnedRouterExited);
                }
                _ = interrupt.recv() => {
                    state.phase = HostPhase::Stopping;
                    settle_active_restart_for_shutdown(&mut active_restart, &mut app_server).await;
                    settle_active_router_restart_for_shutdown(&mut active_router_restart, &mut router_child).await;
                    settle_recovery_for_shutdown(&mut recovery, &mut app_server).await;
                    shutdown_retained_children(&mut app_server, &mut router_child).await?;
                    return Ok(HostExit::Signal);
                }
                _ = terminate.recv() => {
                    state.phase = HostPhase::Stopping;
                    settle_active_restart_for_shutdown(&mut active_restart, &mut app_server).await;
                    settle_active_router_restart_for_shutdown(&mut active_router_restart, &mut router_child).await;
                    settle_recovery_for_shutdown(&mut recovery, &mut app_server).await;
                    shutdown_retained_children(&mut app_server, &mut router_child).await?;
                    return Ok(HostExit::Signal);
                }
                _ = hangup.recv() => {
                    state.phase = HostPhase::Stopping;
                    settle_active_restart_for_shutdown(&mut active_restart, &mut app_server).await;
                    settle_active_router_restart_for_shutdown(&mut active_router_restart, &mut router_child).await;
                    settle_recovery_for_shutdown(&mut recovery, &mut app_server).await;
                    shutdown_retained_children(&mut app_server, &mut router_child).await?;
                    return Ok(HostExit::Signal);
                }
            }
        }
    }
}

struct RuntimeState {
    phase: HostPhase,
    router: RouterCondition,
    app_server: AppServerCondition,
    remote_control: RemoteControlCondition,
    executable_relation: ExecutableRelation,
    recovery_budget: RecoveryBudget,
    last_lifecycle_outcome: Option<LifecycleOutcome>,
}

impl RuntimeState {
    fn ready(router: RouterCondition, readiness: AppServerReadiness) -> Self {
        let mut state = Self {
            phase: HostPhase::Steady,
            router,
            app_server: AppServerCondition::Starting,
            remote_control: RemoteControlCondition::Unavailable,
            executable_relation: ExecutableRelation::Match,
            recovery_budget: RecoveryBudget::Available,
            last_lifecycle_outcome: None,
        };
        state.apply_readiness(readiness);
        state
    }

    fn apply_readiness(&mut self, readiness: AppServerReadiness) {
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

    fn snapshot(&self) -> HostSnapshot {
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

    fn record_lifecycle(
        &self,
        operation: HostOperation,
        result: &'static str,
        duration: std::time::Duration,
    ) {
        crate::telemetry::record_lifecycle(
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

type RecoveryFuture = Pin<
    Box<
        dyn Future<Output = Result<(AppServerChild, AppServerReadiness), HostError>>
            + Send
            + 'static,
    >,
>;

fn recovery_future(config: &HostConfig, launch_plan: AppServerLaunchPlan) -> RecoveryFuture {
    let socket_path = config.app_server_socket().to_owned();
    let deadlines = config.deadlines();
    Box::pin(async move {
        require_unowned_app_server_endpoint(&socket_path, deadlines.endpoint_inspection()).await?;
        let mut child = launch_plan.spawn()?;
        let readiness = child
            .await_readiness(
                &socket_path,
                deadlines.app_server_start(),
                deadlines.remote_control(),
            )
            .await;
        match readiness {
            Ok(readiness) => Ok((child, readiness)),
            Err(readiness_error) => {
                let _shutdown_outcome = child.shutdown().await?;
                Err(HostError::AppServerReadiness(readiness_error))
            }
        }
    })
}

async fn wait_for_app_server_exit(
    app_server: &mut Option<AppServerChild>,
) -> Result<std::process::ExitStatus, ProcessGroupError> {
    match app_server.as_mut() {
        Some(child) => child.wait_for_exit().await,
        None => std::future::pending().await,
    }
}

async fn wait_for_router_exit(
    router: &mut Option<RouterChild>,
) -> Result<std::process::ExitStatus, ProcessGroupError> {
    match router.as_mut() {
        Some(child) => child.wait_for_exit().await,
        None => std::future::pending().await,
    }
}

async fn wait_for_recovery(
    recovery: &mut Option<RecoveryFuture>,
) -> Result<(AppServerChild, AppServerReadiness), HostError> {
    match recovery.as_mut() {
        Some(future) => future.await,
        None => std::future::pending().await,
    }
}

async fn wait_for_active_restart(
    active_restart: &mut Option<operator::ActiveAppServerRestart>,
) -> crate::restart::AppServerRestartCompletion {
    match active_restart.as_mut() {
        Some(active) => active.future.as_mut().await,
        None => std::future::pending().await,
    }
}

async fn wait_for_active_router_restart(
    active_restart: &mut Option<operator::ActiveRouterRestart>,
) -> crate::restart::RouterRestartCompletion {
    match active_restart.as_mut() {
        Some(active) => active.future.as_mut().await,
        None => std::future::pending().await,
    }
}

async fn shutdown_retained_children(
    app_server: &mut Option<AppServerChild>,
    router: &mut Option<RouterChild>,
) -> Result<(), HostError> {
    if let Some(child) = app_server.as_mut() {
        let _app_server_outcome = child.shutdown().await?;
    }
    if let Some(child) = router.as_mut() {
        let _router_outcome = child.shutdown().await?;
    }
    Ok(())
}

async fn settle_recovery_for_shutdown(
    recovery: &mut Option<RecoveryFuture>,
    app_server: &mut Option<AppServerChild>,
) {
    let Some(recovery_future) = recovery.take() else {
        return;
    };
    if let Ok((recovered_child, _readiness)) = recovery_future.await {
        *app_server = Some(recovered_child);
    }
}

async fn settle_active_restart_for_shutdown(
    active_restart: &mut Option<operator::ActiveAppServerRestart>,
    app_server: &mut Option<AppServerChild>,
) {
    let Some(mut active) = active_restart.take() else {
        return;
    };
    let completion = active.future.as_mut().await;
    *app_server = completion.child;
}

async fn settle_active_router_restart_for_shutdown(
    active_restart: &mut Option<operator::ActiveRouterRestart>,
    router: &mut Option<RouterChild>,
) {
    let Some(mut active) = active_restart.take() else {
        return;
    };
    let completion = active.future.as_mut().await;
    *router = completion.child;
}

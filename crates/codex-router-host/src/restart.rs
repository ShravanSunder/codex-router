//! Retained lifecycle futures for serialized explicit restart operations.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use crate::AppServerChild;
use crate::AppServerLaunchPlan;
use crate::AppServerReadiness;
use crate::ChildCommandSpec;
use crate::HostConfig;
use crate::RouterChild;
use crate::RouterProbeResult;
use crate::RouterShutdownOutcome;
use crate::ShutdownOutcome;
use crate::probe_router;
use crate::require_unowned_app_server_endpoint;

pub(crate) type AppServerRestartFuture =
    Pin<Box<dyn Future<Output = AppServerRestartCompletion> + Send + 'static>>;

pub(crate) struct AppServerRestartCompletion {
    pub(crate) child: Option<AppServerChild>,
    pub(crate) readiness: Option<AppServerReadiness>,
    pub(crate) shutdown_outcome: Option<ShutdownOutcome>,
    pub(crate) succeeded: bool,
    pub(crate) message: &'static str,
}

pub(crate) type RouterRestartFuture =
    Pin<Box<dyn Future<Output = RouterRestartCompletion> + Send + 'static>>;

pub(crate) struct RouterRestartCompletion {
    pub(crate) child: Option<RouterChild>,
    pub(crate) succeeded: bool,
    pub(crate) message: &'static str,
}

#[derive(Clone, Default)]
pub(crate) struct StopIntent(Arc<AtomicBool>);

impl StopIntent {
    pub(crate) fn request(&self) {
        self.0.store(true, Ordering::Release);
    }

    fn is_requested(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

pub(crate) fn restart_app_server(
    config: HostConfig,
    launch_plan: AppServerLaunchPlan,
    current_child: Option<AppServerChild>,
    stop_intent: StopIntent,
) -> AppServerRestartFuture {
    Box::pin(async move {
        let mut shutdown_outcome = None;
        if let Some(mut child) = current_child {
            match child.shutdown().await {
                Ok(outcome @ (ShutdownOutcome::Graceful | ShutdownOutcome::Forced)) => {
                    shutdown_outcome = Some(outcome);
                }
                Ok(ShutdownOutcome::TimedOutStillRunning) => {
                    return AppServerRestartCompletion {
                        child: Some(child),
                        readiness: None,
                        shutdown_outcome: Some(ShutdownOutcome::TimedOutStillRunning),
                        succeeded: false,
                        message: "app-server shutdown timed out; retained child requires cleanup",
                    };
                }
                Err(_error) => {
                    return AppServerRestartCompletion {
                        child: Some(child),
                        readiness: None,
                        shutdown_outcome: None,
                        succeeded: false,
                        message: "app-server shutdown failed; retained child requires cleanup",
                    };
                }
            }
        }

        if stop_intent.is_requested() {
            return AppServerRestartCompletion {
                child: None,
                readiness: None,
                shutdown_outcome,
                succeeded: false,
                message: "foreground stop cancelled app-server replacement",
            };
        }

        if require_unowned_app_server_endpoint(
            config.app_server_socket(),
            config.deadlines().endpoint_inspection(),
        )
        .await
        .is_err()
        {
            return AppServerRestartCompletion {
                child: None,
                readiness: None,
                shutdown_outcome,
                succeeded: false,
                message: "native app-server endpoint remains owned",
            };
        }

        if stop_intent.is_requested() {
            return AppServerRestartCompletion {
                child: None,
                readiness: None,
                shutdown_outcome,
                succeeded: false,
                message: "foreground stop cancelled app-server replacement",
            };
        }

        let mut replacement = match launch_plan.spawn() {
            Ok(child) => child,
            Err(_error) => {
                return AppServerRestartCompletion {
                    child: None,
                    readiness: None,
                    shutdown_outcome,
                    succeeded: false,
                    message: "replacement app-server failed to start",
                };
            }
        };
        match replacement
            .await_readiness(
                config.app_server_socket(),
                config.deadlines().app_server_start(),
                config.deadlines().remote_control(),
            )
            .await
        {
            Ok(readiness) => AppServerRestartCompletion {
                child: Some(replacement),
                readiness: Some(readiness),
                shutdown_outcome,
                succeeded: true,
                message: "app-server restarted",
            },
            Err(_error) => {
                let replacement_shutdown = replacement.shutdown().await;
                let child = match replacement_shutdown {
                    Ok(ShutdownOutcome::TimedOutStillRunning) | Err(_) => Some(replacement),
                    Ok(ShutdownOutcome::Graceful | ShutdownOutcome::Forced) => None,
                };
                AppServerRestartCompletion {
                    child,
                    readiness: None,
                    shutdown_outcome: replacement_shutdown.ok().or(shutdown_outcome),
                    succeeded: false,
                    message: "replacement app-server readiness failed",
                }
            }
        }
    })
}

pub(crate) fn restart_router(
    config: HostConfig,
    command: ChildCommandSpec,
    current_child: Option<RouterChild>,
    stop_intent: StopIntent,
) -> RouterRestartFuture {
    Box::pin(async move {
        if let Some(mut current_child) = current_child {
            match current_child.shutdown().await {
                Ok(RouterShutdownOutcome::Graceful) => {}
                Ok(RouterShutdownOutcome::TimedOutStillRunning) | Err(_) => {
                    return RouterRestartCompletion {
                        child: Some(current_child),
                        succeeded: false,
                        message: "owned router shutdown timed out or failed",
                    };
                }
            }
        }

        if stop_intent.is_requested() {
            return RouterRestartCompletion {
                child: None,
                succeeded: false,
                message: "foreground stop cancelled router replacement",
            };
        }

        let mut replacement_command = command.command();
        let mut replacement = match RouterChild::spawn(&mut replacement_command) {
            Ok(child) => child,
            Err(_error) => {
                return RouterRestartCompletion {
                    child: None,
                    succeeded: false,
                    message: "replacement router failed to start",
                };
            }
        };
        match await_compatible_router(config.router_endpoint(), config.deadlines().router_start())
            .await
        {
            Ok(RouterProbeResult::Compatible) => RouterRestartCompletion {
                child: Some(replacement),
                succeeded: true,
                message: "owned router restarted",
            },
            Ok(
                RouterProbeResult::AuthenticationRequired
                | RouterProbeResult::Incompatible
                | RouterProbeResult::Unavailable,
            )
            | Err(_) => {
                let shutdown_result = replacement.shutdown().await;
                let child = match shutdown_result {
                    Ok(RouterShutdownOutcome::TimedOutStillRunning) | Err(_) => Some(replacement),
                    Ok(RouterShutdownOutcome::Graceful) => None,
                };
                RouterRestartCompletion {
                    child,
                    succeeded: false,
                    message: "replacement router compatibility failed",
                }
            }
        }
    })
}

async fn await_compatible_router(
    endpoint: std::net::SocketAddr,
    deadline: std::time::Duration,
) -> Result<RouterProbeResult, crate::RouterProbeError> {
    let deadline_at = tokio::time::Instant::now() + deadline;
    loop {
        let now = tokio::time::Instant::now();
        if now >= deadline_at {
            return Ok(RouterProbeResult::Unavailable);
        }
        match probe_router(endpoint, deadline_at.saturating_duration_since(now)).await? {
            RouterProbeResult::Unavailable => {
                tokio::time::sleep_until(
                    deadline_at
                        .min(tokio::time::Instant::now() + std::time::Duration::from_millis(20)),
                )
                .await;
            }
            terminal => return Ok(terminal),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stop_intent_prevents_restart_replacement_spawn()
    -> Result<(), Box<dyn std::error::Error>> {
        let config = HostConfig::new(crate::HostConfigInputs {
            coordination_paths: crate::HostCoordinationPaths::new(
                std::path::PathBuf::from("/unused/operator.sock"),
                std::path::PathBuf::from("/unused/instance.lock"),
            ),
            router_endpoint: "127.0.0.1:9".parse()?,
            app_server_socket: std::path::PathBuf::from("/unused/app.sock"),
            managed_executable: std::path::PathBuf::from("/unused/codex"),
            deadlines: crate::HostDeadlines::production(),
        });
        let identity = codex_router_codex::executable_identity(&std::env::current_exe()?).await?;
        let app_plan = AppServerLaunchPlan::new(
            ChildCommandSpec::new(std::path::PathBuf::from("/must-not-spawn")),
            identity,
            "1.2.3".to_owned(),
        );
        let app_stop = StopIntent::default();
        app_stop.request();
        let app_completion = restart_app_server(config.clone(), app_plan, None, app_stop).await;
        if app_completion.child.is_some() || app_completion.succeeded {
            return Err("stop intent spawned an app-server replacement".into());
        }

        let router_stop = StopIntent::default();
        router_stop.request();
        let router_completion = restart_router(
            config,
            ChildCommandSpec::new(std::path::PathBuf::from("/must-not-spawn")),
            None,
            router_stop,
        )
        .await;
        if router_completion.child.is_some() || router_completion.succeeded {
            return Err("stop intent spawned a router replacement".into());
        }
        Ok(())
    }
}

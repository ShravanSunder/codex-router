//! Serialized explicit app-server restart and stop cancellation.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use crate::AppServerChild;
use crate::AppServerLaunchPlan;
use crate::AppServerReadiness;
use crate::HostConfig;
use crate::ShutdownOutcome;
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

#[derive(Clone, Default)]
pub(crate) struct StopIntent(Arc<AtomicBool>);

impl StopIntent {
    pub(crate) fn request(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub(crate) fn is_requested(&self) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stop_intent_prevents_app_server_replacement_spawn()
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
        let launch_plan = AppServerLaunchPlan::new(
            crate::ChildCommandSpec::new(std::path::PathBuf::from("/must-not-spawn")),
            identity,
            "1.2.3".to_owned(),
        );
        let stop_intent = StopIntent::default();
        stop_intent.request();
        let completion = restart_app_server(config, launch_plan, None, stop_intent).await;
        if completion.child.is_some() || completion.succeeded {
            return Err("stop intent spawned an app-server replacement".into());
        }
        Ok(())
    }
}

//! Serialized explicit owned-router restart.

use std::future::Future;
use std::pin::Pin;

use crate::ChildCommandSpec;
use crate::HostConfig;
use crate::RouterChild;
use crate::RouterProbeResult;
use crate::RouterShutdownOutcome;
use crate::explicit_app_server_restart::StopIntent;
use crate::probe_router;

pub(crate) type RouterRestartFuture =
    Pin<Box<dyn Future<Output = RouterRestartCompletion> + Send + 'static>>;

pub(crate) struct RouterRestartCompletion {
    pub(crate) child: Option<RouterChild>,
    pub(crate) succeeded: bool,
    pub(crate) message: &'static str,
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
    async fn stop_intent_prevents_router_replacement_spawn()
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
        let stop_intent = StopIntent::default();
        stop_intent.request();
        let completion = restart_router(
            config,
            ChildCommandSpec::new(std::path::PathBuf::from("/must-not-spawn")),
            None,
            stop_intent,
        )
        .await;
        if completion.child.is_some() || completion.succeeded {
            return Err("stop intent spawned a router replacement".into());
        }
        Ok(())
    }
}

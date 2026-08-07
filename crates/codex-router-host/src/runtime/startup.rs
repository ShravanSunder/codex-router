//! Ordered router and app-server startup with retained-child cleanup on failure.

use super::*;

pub(super) async fn start_router(
    config: &HostConfig,
    router_command: Option<&ChildCommandSpec>,
) -> Result<(RouterCondition, Option<RouterChild>), HostError> {
    match probe_router(config.router_endpoint(), config.deadlines().router_start()).await? {
        RouterProbeResult::Compatible => Ok((RouterCondition::ExternalReachable, None)),
        RouterProbeResult::AuthenticationRequired => Err(HostError::RouterAuthenticationRequired),
        RouterProbeResult::Incompatible => Err(HostError::RouterIncompatible),
        RouterProbeResult::Unavailable => {
            let command = router_command.ok_or(HostError::RouterUnavailable)?;
            let mut command = command.command();
            let mut child = RouterChild::spawn(&mut command)?;
            let probe_result = match await_router_readiness(
                config.router_endpoint(),
                config.deadlines().router_start(),
            )
            .await
            {
                Ok(result) => result,
                Err(error) => {
                    let _shutdown_outcome = child.shutdown().await?;
                    return Err(HostError::RouterProbe(error));
                }
            };
            match probe_result {
                RouterProbeResult::Compatible => Ok((RouterCondition::OwnedReachable, Some(child))),
                RouterProbeResult::AuthenticationRequired => {
                    let _shutdown_outcome = child.shutdown().await?;
                    Err(HostError::RouterAuthenticationRequired)
                }
                RouterProbeResult::Incompatible => {
                    let _shutdown_outcome = child.shutdown().await?;
                    Err(HostError::RouterIncompatible)
                }
                RouterProbeResult::Unavailable => {
                    let _shutdown_outcome = child.shutdown().await?;
                    Err(HostError::RouterUnavailable)
                }
            }
        }
    }
}

async fn await_router_readiness(
    endpoint: std::net::SocketAddr,
    deadline: std::time::Duration,
) -> Result<RouterProbeResult, RouterProbeError> {
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

pub(super) async fn start_app_server(
    config: &HostConfig,
    launch_plan: AppServerLaunchPlan,
) -> Result<(AppServerChild, AppServerReadiness), HostError> {
    let mut child = launch_plan.spawn()?;
    let readiness = child
        .await_readiness(
            config.app_server_socket(),
            config.deadlines().app_server_start(),
            config.deadlines().remote_control(),
        )
        .await;
    match readiness {
        Ok(readiness) => Ok((child, readiness)),
        Err(readiness_error) => {
            let _shutdown_outcome = child.shutdown().await?;
            Err(HostError::AppServerReadiness(readiness_error))
        }
    }
}

pub(super) async fn shutdown_owned_router_after_startup_failure(
    router: &mut Option<RouterChild>,
) -> Result<(), HostError> {
    if let Some(child) = router.as_mut() {
        let _shutdown_outcome = child.shutdown().await?;
    }
    Ok(())
}

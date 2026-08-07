//! Retained lifecycle waits and cancellation-safe foreground convergence.

use std::future::Future;
use std::pin::Pin;

use super::*;

pub(super) struct ShutdownContext<'a> {
    pub(super) activation: &'a mut Option<operator::ActiveUpdateActivation>,
    pub(super) active_update: &'a mut Option<operator::ActiveUpdate>,
    pub(super) pending_identity: &'a mut Option<codex_router_codex::ExecutableIdentityTask>,
    pub(super) retained_updater: &'a mut Option<ProcessGroupChild>,
    pub(super) active_app_server_restart: &'a mut Option<operator::ActiveAppServerRestart>,
    pub(super) active_router_restart: &'a mut Option<operator::ActiveRouterRestart>,
    pub(super) recovery: &'a mut Option<RecoveryFuture>,
    pub(super) app_server: &'a mut Option<AppServerChild>,
    pub(super) router: &'a mut Option<RouterChild>,
}

pub(super) async fn settle_for_shutdown(context: ShutdownContext<'_>) -> Result<(), HostError> {
    settle_update_activation_for_shutdown(context.activation, context.app_server, context.router)
        .await;
    settle_active_update_for_shutdown(
        context.active_update,
        context.pending_identity,
        context.retained_updater,
    )
    .await;
    settle_active_restart_for_shutdown(context.active_app_server_restart, context.app_server).await;
    settle_active_router_restart_for_shutdown(context.active_router_restart, context.router).await;
    settle_recovery_for_shutdown(context.recovery, context.app_server).await;
    shutdown_retained_children(context.app_server, context.router).await
}

pub(super) type RecoveryFuture = Pin<
    Box<
        dyn Future<Output = Result<(AppServerChild, AppServerReadiness), HostError>>
            + Send
            + 'static,
    >,
>;

pub(super) fn recovery_future(
    config: &HostConfig,
    launch_plan: AppServerLaunchPlan,
) -> RecoveryFuture {
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

pub(super) async fn wait_for_app_server_exit(
    app_server: &mut Option<AppServerChild>,
) -> Result<std::process::ExitStatus, ProcessGroupError> {
    match app_server.as_mut() {
        Some(child) => child.wait_for_exit().await,
        None => std::future::pending().await,
    }
}

pub(super) async fn wait_for_router_exit(
    router: &mut Option<RouterChild>,
) -> Result<std::process::ExitStatus, ProcessGroupError> {
    match router.as_mut() {
        Some(child) => child.wait_for_exit().await,
        None => std::future::pending().await,
    }
}

pub(super) async fn wait_for_recovery(
    recovery: &mut Option<RecoveryFuture>,
) -> Result<(AppServerChild, AppServerReadiness), HostError> {
    match recovery.as_mut() {
        Some(future) => future.await,
        None => std::future::pending().await,
    }
}

pub(super) async fn wait_for_active_restart(
    active_restart: &mut Option<operator::ActiveAppServerRestart>,
) -> crate::restart::AppServerRestartCompletion {
    match active_restart.as_mut() {
        Some(active) => active.future.as_mut().await,
        None => std::future::pending().await,
    }
}

pub(super) async fn wait_for_active_router_restart(
    active_restart: &mut Option<operator::ActiveRouterRestart>,
) -> crate::restart::RouterRestartCompletion {
    match active_restart.as_mut() {
        Some(active) => active.future.as_mut().await,
        None => std::future::pending().await,
    }
}

pub(super) async fn wait_for_active_update(
    active_update: &mut Option<operator::ActiveUpdate>,
) -> crate::update::UpdatePreparation {
    match active_update.as_mut() {
        Some(active) => active.future.as_mut().await,
        None => std::future::pending().await,
    }
}

pub(super) async fn wait_for_status_observation(
    active_status: &mut Option<operator::ActiveStatusObservation>,
) -> status::StatusObservation {
    match active_status.as_mut() {
        Some(active) => active.future.as_mut().await,
        None => std::future::pending().await,
    }
}

pub(super) async fn wait_for_update_activation(
    activation: &mut Option<operator::ActiveUpdateActivation>,
) -> crate::update::UpdateActivationCompletion {
    match activation.as_mut() {
        Some(active) => active.future.as_mut().await,
        None => std::future::pending().await,
    }
}

pub(super) async fn wait_for_pending_identity(
    pending_identity: &mut Option<codex_router_codex::ExecutableIdentityTask>,
) -> Result<codex_router_codex::ExecutableIdentity, codex_router_codex::ExecutableIdentityError> {
    match pending_identity.as_mut() {
        Some(task) => task.wait().await,
        None => std::future::pending().await,
    }
}

pub(super) async fn wait_for_retained_updater(
    retained_updater: &mut Option<ProcessGroupChild>,
) -> Result<std::process::ExitStatus, ProcessGroupError> {
    match retained_updater.as_mut() {
        Some(child) => child.wait().await,
        None => std::future::pending().await,
    }
}

pub(super) async fn shutdown_retained_children(
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

pub(super) async fn settle_recovery_for_shutdown(
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

pub(super) async fn settle_active_restart_for_shutdown(
    active_restart: &mut Option<operator::ActiveAppServerRestart>,
    app_server: &mut Option<AppServerChild>,
) {
    let Some(mut active) = active_restart.take() else {
        return;
    };
    active.stop_intent.request();
    let completion = active.future.as_mut().await;
    *app_server = completion.child;
}

pub(super) async fn settle_active_router_restart_for_shutdown(
    active_restart: &mut Option<operator::ActiveRouterRestart>,
    router: &mut Option<RouterChild>,
) {
    let Some(mut active) = active_restart.take() else {
        return;
    };
    active.stop_intent.request();
    let completion = active.future.as_mut().await;
    *router = completion.child;
}

pub(super) async fn settle_active_update_for_shutdown(
    active_update: &mut Option<operator::ActiveUpdate>,
    pending_identity: &mut Option<codex_router_codex::ExecutableIdentityTask>,
    retained_updater: &mut Option<ProcessGroupChild>,
) {
    let Some(mut active) = active_update.take() else {
        return;
    };
    if let crate::update::UpdatePreparation::Failed(failure) = active.future.as_mut().await {
        *pending_identity = failure.pending_identity;
        *retained_updater = failure.retained_updater;
    }
}

pub(super) async fn settle_update_activation_for_shutdown(
    activation: &mut Option<operator::ActiveUpdateActivation>,
    app_server: &mut Option<AppServerChild>,
    router: &mut Option<RouterChild>,
) {
    let Some(mut active) = activation.take() else {
        return;
    };
    let completion = active.future.as_mut().await;
    *app_server = completion.app_server;
    *router = completion.router;
}

pub(super) async fn flush_pre_exec_telemetry(telemetry: Option<Arc<dyn PreExecTelemetry>>) {
    let Some(telemetry) = telemetry else {
        return;
    };
    let task = tokio::task::spawn_blocking(move || telemetry.flush_and_shutdown());
    match tokio::time::timeout(std::time::Duration::from_secs(2), task).await {
        Ok(Ok(())) => {}
        Ok(Err(_)) | Err(_) => {
            tracing::warn!(
                event.name = "codex_router.host.pre_exec_telemetry_failed",
                result = "redacted_failure"
            );
        }
    }
}

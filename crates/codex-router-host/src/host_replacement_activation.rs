//! Ordered retained-child teardown shared by whole-Host replacement operations.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::AppServerChild;
use crate::RouterChild;
use crate::RouterShutdownOutcome;
use crate::ShutdownOutcome;
use crate::{HostProgress, OperatorFrame};
use tokio::sync::mpsc;

pub(crate) type HostReplacementFuture =
    Pin<Box<dyn Future<Output = HostReplacementCompletion> + Send + 'static>>;

pub(crate) struct HostReplacementCompletion {
    pub(crate) app_server: Option<AppServerChild>,
    pub(crate) router: Option<RouterChild>,
    pub(crate) app_server_shutdown: Option<ShutdownOutcome>,
    pub(crate) failure: Option<HostReplacementFailure>,
}

#[derive(Clone, Copy)]
pub(crate) enum HostReplacementFailure {
    AppServerTeardown,
    RouterTeardown,
}

pub(crate) fn activate_host_replacement(
    mut app_server: Option<AppServerChild>,
    mut router: Option<RouterChild>,
    progress: mpsc::Sender<OperatorFrame>,
    pre_exec_telemetry: Option<Arc<dyn crate::PreExecTelemetry>>,
) -> HostReplacementFuture {
    Box::pin(async move {
        let activation_started_at = std::time::Instant::now();
        crate::lifecycle_owner::flush_pre_exec_telemetry(pre_exec_telemetry).await;
        crate::record_debug_readiness_timing("preExecTelemetryDone", activation_started_at);
        let mut app_server_shutdown = None;
        if let Some(child) = app_server.as_mut() {
            let _ = progress
                .send(OperatorFrame::Progress(HostProgress::StoppingAppServer))
                .await;
            match child.shutdown().await {
                Ok(
                    outcome @ (ShutdownOutcome::Graceful
                    | ShutdownOutcome::ForcedDrain
                    | ShutdownOutcome::Killed),
                ) => {
                    app_server_shutdown = Some(outcome);
                    app_server = None;
                    crate::record_debug_readiness_timing("appServerStopped", activation_started_at);
                }
                Ok(ShutdownOutcome::TimedOutStillRunning) => {
                    return HostReplacementCompletion {
                        app_server,
                        router,
                        app_server_shutdown: Some(ShutdownOutcome::TimedOutStillRunning),
                        failure: Some(HostReplacementFailure::AppServerTeardown),
                    };
                }
                Err(_) => {
                    return HostReplacementCompletion {
                        app_server,
                        router,
                        app_server_shutdown: None,
                        failure: Some(HostReplacementFailure::AppServerTeardown),
                    };
                }
            }
        }
        if let Some(child) = router.as_mut() {
            let _ = progress
                .send(OperatorFrame::Progress(HostProgress::StoppingRouter))
                .await;
            match child.shutdown().await {
                Ok(RouterShutdownOutcome::Graceful) => {
                    router = None;
                    crate::record_debug_readiness_timing("routerStopped", activation_started_at);
                }
                Ok(RouterShutdownOutcome::TimedOutStillRunning) | Err(_) => {
                    return HostReplacementCompletion {
                        app_server,
                        router,
                        app_server_shutdown,
                        failure: Some(HostReplacementFailure::RouterTeardown),
                    };
                }
            }
        }
        let _ = progress
            .send(OperatorFrame::Progress(HostProgress::ReExecuting))
            .await;
        crate::record_debug_readiness_timing("reExecutingQueued", activation_started_at);
        HostReplacementCompletion {
            app_server,
            router,
            app_server_shutdown,
            failure: None,
        }
    })
}

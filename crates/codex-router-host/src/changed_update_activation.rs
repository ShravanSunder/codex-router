//! Ordered retained-child teardown after a changed managed-Codex update.

use std::future::Future;
use std::pin::Pin;

use crate::AppServerChild;
use crate::RouterChild;
use crate::RouterShutdownOutcome;
use crate::ShutdownOutcome;

pub(crate) type UpdateActivationFuture =
    Pin<Box<dyn Future<Output = UpdateActivationCompletion> + Send + 'static>>;

pub(crate) struct UpdateActivationCompletion {
    pub(crate) app_server: Option<AppServerChild>,
    pub(crate) router: Option<RouterChild>,
    pub(crate) app_server_shutdown: Option<ShutdownOutcome>,
    pub(crate) succeeded: bool,
    pub(crate) message: &'static str,
}

pub(crate) fn activate_changed_update(
    mut app_server: Option<AppServerChild>,
    mut router: Option<RouterChild>,
) -> UpdateActivationFuture {
    Box::pin(async move {
        let mut app_server_shutdown = None;
        if let Some(child) = app_server.as_mut() {
            match child.shutdown().await {
                Ok(outcome @ (ShutdownOutcome::Graceful | ShutdownOutcome::Forced)) => {
                    app_server_shutdown = Some(outcome);
                    app_server = None;
                }
                Ok(ShutdownOutcome::TimedOutStillRunning) => {
                    return UpdateActivationCompletion {
                        app_server,
                        router,
                        app_server_shutdown: Some(ShutdownOutcome::TimedOutStillRunning),
                        succeeded: false,
                        message: "updated Codex but app-server teardown failed",
                    };
                }
                Err(_) => {
                    return UpdateActivationCompletion {
                        app_server,
                        router,
                        app_server_shutdown: None,
                        succeeded: false,
                        message: "updated Codex but app-server teardown failed",
                    };
                }
            }
        }
        if let Some(child) = router.as_mut() {
            match child.shutdown().await {
                Ok(RouterShutdownOutcome::Graceful) => {
                    router = None;
                }
                Ok(RouterShutdownOutcome::TimedOutStillRunning) | Err(_) => {
                    return UpdateActivationCompletion {
                        app_server,
                        router,
                        app_server_shutdown,
                        succeeded: false,
                        message: "updated Codex but router teardown failed",
                    };
                }
            }
        }
        UpdateActivationCompletion {
            app_server,
            router,
            app_server_shutdown,
            succeeded: true,
            message: "updated Codex and starting replacement host",
        }
    })
}

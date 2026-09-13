//! Ordered retained-child teardown shared by whole-Host replacement operations.

use std::future::Future;
use std::pin::Pin;

use crate::AppServerChild;
use crate::RouterChild;
use crate::RouterShutdownOutcome;
use crate::ShutdownOutcome;

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
) -> HostReplacementFuture {
    Box::pin(async move {
        let mut app_server_shutdown = None;
        if let Some(child) = app_server.as_mut() {
            match child.shutdown().await {
                Ok(outcome @ (ShutdownOutcome::Graceful | ShutdownOutcome::Forced)) => {
                    app_server_shutdown = Some(outcome);
                    app_server = None;
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
            match child.shutdown().await {
                Ok(RouterShutdownOutcome::Graceful) => {
                    router = None;
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
        HostReplacementCompletion {
            app_server,
            router,
            app_server_shutdown,
            failure: None,
        }
    })
}

//! Conditional managed-Codex updater execution before any lifecycle activation.

use std::future::Future;
use std::path::Path;
use std::path::PathBuf;
use std::pin::Pin;
use std::time::Duration;

use codex_router_codex::ExecutableIdentityTask;
use codex_router_codex::UpdaterCommandSpec;
use codex_router_codex::start_executable_identity;
use thiserror::Error;

use crate::AppServerChild;
use crate::HostSnapshot;
use crate::ProcessGroupChild;
use crate::RouterChild;
use crate::RouterShutdownOutcome;
use crate::ShutdownOutcome;

/// Owner-visible four-result update contract.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UpdateResult {
    /// Official updater succeeded without changing executable content.
    NoChange,
    /// Identity or updater work failed before any child was signalled.
    FailedWithoutRestart {
        /// Redacted bounded explanation.
        message: String,
    },
    /// Changed executable was activated by a locally ready replacement host.
    UpdatedAndHostRestarted {
        /// Replacement lifetime's terminal startup snapshot.
        snapshot: HostSnapshot,
    },
    /// Executable changed, but teardown or replacement activation failed.
    UpdatedButReplacementFailed {
        /// Redacted bounded explanation.
        message: String,
        /// Exact manual recovery command or action.
        recovery_action: String,
    },
}

/// Validated updater and identity bounds.
#[derive(Clone, Copy)]
pub struct UpdateDeadlines {
    identity: Duration,
    updater: Duration,
    terminate_grace: Duration,
    force_wait: Duration,
}

impl UpdateDeadlines {
    /// Production updater containment: 15 minutes, then 10-second TERM/KILL stages.
    #[must_use]
    pub const fn production() -> Self {
        Self {
            identity: Duration::from_secs(30),
            updater: Duration::from_secs(15 * 60),
            terminate_grace: Duration::from_secs(10),
            force_wait: Duration::from_secs(10),
        }
    }

    /// Creates shorter positive fixture bounds.
    pub fn new(
        identity: Duration,
        updater: Duration,
        terminate_grace: Duration,
        force_wait: Duration,
    ) -> Result<Self, UpdateDeadlineError> {
        if [identity, updater, terminate_grace, force_wait]
            .into_iter()
            .any(|duration| duration.is_zero())
        {
            return Err(UpdateDeadlineError::Zero);
        }
        Ok(Self {
            identity,
            updater,
            terminate_grace,
            force_wait,
        })
    }

    pub(crate) const fn identity(self) -> Duration {
        self.identity
    }
}

/// Invalid updater deadline configuration.
#[derive(Debug, Error)]
pub enum UpdateDeadlineError {
    /// Every stage must have a positive finite bound.
    #[error("update deadlines must be greater than zero")]
    Zero,
}

pub(crate) enum UpdatePreparation {
    NoChange,
    Changed,
    Failed(UpdateFailure),
}

pub(crate) struct UpdateFailure {
    pub(crate) message: &'static str,
    pub(crate) pending_identity: Option<ExecutableIdentityTask>,
    pub(crate) retained_updater: Option<ProcessGroupChild>,
}

pub(crate) type UpdateFuture = Pin<Box<dyn Future<Output = UpdatePreparation> + Send + 'static>>;

pub(crate) fn start_update(
    managed_executable: PathBuf,
    deadlines: UpdateDeadlines,
) -> UpdateFuture {
    Box::pin(async move { prepare_update(&managed_executable, deadlines).await })
}

pub(crate) async fn prepare_update(
    managed_executable: &Path,
    deadlines: UpdateDeadlines,
) -> UpdatePreparation {
    let mut initial_identity_task = start_executable_identity(managed_executable);
    let initial_identity =
        match tokio::time::timeout(deadlines.identity, initial_identity_task.wait()).await {
            Ok(Ok(identity)) => identity,
            Ok(Err(_error)) => return failed("managed executable identity failed", None, None),
            Err(_elapsed) => {
                return failed(
                    "managed executable identity timed out",
                    Some(initial_identity_task),
                    None,
                );
            }
        };

    let updater_spec = UpdaterCommandSpec::new(&initial_identity);
    let mut updater_command = tokio::process::Command::new(updater_spec.executable());
    updater_command
        .args(updater_spec.arguments())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let mut updater = match ProcessGroupChild::spawn(&mut updater_command) {
        Ok(child) => child,
        Err(_error) => return failed("official Codex updater failed to start", None, None),
    };
    let updater_status = match tokio::time::timeout(deadlines.updater, updater.wait()).await {
        Ok(Ok(status)) => status,
        Ok(Err(_error)) => {
            return failed("official Codex updater wait failed", None, Some(updater));
        }
        Err(_elapsed) => {
            if updater.send_group_terminate().is_err() {
                return failed(
                    "official Codex updater containment failed",
                    None,
                    Some(updater),
                );
            }
            match tokio::time::timeout(deadlines.terminate_grace, updater.wait()).await {
                Ok(Ok(_status)) => return failed("official Codex updater timed out", None, None),
                Ok(Err(_error)) => {
                    return failed("official Codex updater wait failed", None, Some(updater));
                }
                Err(_elapsed) => {}
            }
            if updater.send_group_kill().is_err() {
                return failed(
                    "official Codex updater containment failed",
                    None,
                    Some(updater),
                );
            }
            return match tokio::time::timeout(deadlines.force_wait, updater.wait()).await {
                Ok(Ok(_status)) => failed("official Codex updater timed out", None, None),
                Ok(Err(_)) | Err(_) => failed(
                    "official Codex updater remained retained",
                    None,
                    Some(updater),
                ),
            };
        }
    };
    if !updater_status.success() {
        return failed("official Codex updater exited unsuccessfully", None, None);
    }

    let mut installed_identity_task = start_executable_identity(managed_executable);
    let installed_identity =
        match tokio::time::timeout(deadlines.identity, installed_identity_task.wait()).await {
            Ok(Ok(identity)) => identity,
            Ok(Err(_error)) => return failed("updated executable identity failed", None, None),
            Err(_elapsed) => {
                return failed(
                    "updated executable identity timed out",
                    Some(installed_identity_task),
                    None,
                );
            }
        };

    if initial_identity == installed_identity {
        UpdatePreparation::NoChange
    } else {
        UpdatePreparation::Changed
    }
}

fn failed(
    message: &'static str,
    pending_identity: Option<ExecutableIdentityTask>,
    retained_updater: Option<ProcessGroupChild>,
) -> UpdatePreparation {
    UpdatePreparation::Failed(UpdateFailure {
        message,
        pending_identity,
        retained_updater,
    })
}

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

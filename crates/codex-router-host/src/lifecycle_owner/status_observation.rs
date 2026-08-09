//! Bounded live observations used only by explicit status requests.

use std::future::Future;
use std::pin::Pin;

use codex_router_codex::ExecutableIdentity;
use codex_router_codex::ExecutableIdentityTask;
use codex_router_codex::RemoteControlObservation;

use crate::RemoteControlIdentity;

use super::*;

pub(super) type StatusObservationFuture =
    Pin<Box<dyn Future<Output = StatusObservation> + Send + 'static>>;

pub(super) struct StatusObservation {
    router: RouterCondition,
    app_server: AppServerCondition,
    remote_control: RemoteControlCondition,
    remote_control_identity: Option<RemoteControlIdentity>,
    executable_relation: ExecutableRelation,
    pending_identity: Option<ExecutableIdentityTask>,
}

impl StatusObservation {
    pub(super) const fn executable_relation(&self) -> ExecutableRelation {
        self.executable_relation
    }

    pub(super) fn snapshot(
        self,
        state: &RuntimeState,
    ) -> (HostSnapshot, Option<ExecutableIdentityTask>) {
        let snapshot = HostSnapshot::new(HostSnapshotDimensions {
            phase: state.phase.clone(),
            router: self.router,
            app_server: self.app_server,
            remote_control: self.remote_control,
            remote_control_identity: self.remote_control_identity,
            executable_relation: self.executable_relation,
            recovery_budget: state.recovery_budget,
            last_lifecycle_outcome: state.last_lifecycle_outcome.clone(),
        });
        (snapshot, self.pending_identity)
    }
}

pub(super) fn observe_status(
    config: HostConfig,
    router_ownership: crate::RouterOwnership,
    owned_router_child_present: bool,
    running_identity: Option<ExecutableIdentity>,
    identity_deadline: std::time::Duration,
    observe_installed_identity: bool,
) -> StatusObservationFuture {
    Box::pin(async move {
        let router = probe_router(config.router_endpoint(), config.deadlines().router_start());
        let app_server_present = running_identity.is_some();
        let app_server = async {
            if !app_server_present {
                return None;
            }
            Some(
                codex_router_codex::observe_app_server(
                    config.app_server_socket(),
                    config.deadlines().app_server_start(),
                    config.deadlines().remote_control(),
                )
                .await,
            )
        };
        let installed_identity = observe_identity(
            config.managed_executable(),
            identity_deadline,
            observe_installed_identity,
        );
        let (router_result, app_server_result, installed_identity) =
            tokio::join!(router, app_server, installed_identity);

        let router = match router_result {
            Ok(RouterProbeResult::Compatible) => match router_ownership {
                crate::RouterOwnership::External => RouterCondition::ExternalReachable,
                crate::RouterOwnership::Owned if owned_router_child_present => {
                    RouterCondition::OwnedReachable
                }
                crate::RouterOwnership::Owned => RouterCondition::Unavailable,
            },
            Ok(
                RouterProbeResult::Unavailable
                | RouterProbeResult::AuthenticationRequired
                | RouterProbeResult::Incompatible,
            )
            | Err(_) => RouterCondition::Unavailable,
        };
        let (app_server, remote_control, remote_control_identity) = match app_server_result {
            Some(Ok(observation)) => {
                let (remote_control, remote_control_identity) =
                    remote_control_status(observation.remote_control());
                (
                    AppServerCondition::NativeReady {
                        running_version: observation.running_version().to_owned(),
                    },
                    remote_control,
                    Some(remote_control_identity),
                )
            }
            Some(Err(_)) => (
                AppServerCondition::Failed,
                RemoteControlCondition::Unavailable,
                None,
            ),
            None => (
                AppServerCondition::Absent,
                RemoteControlCondition::Unavailable,
                None,
            ),
        };
        if let Some(identity) = &remote_control_identity {
            crate::lifecycle_telemetry::record_remote_control_observation(remote_control, identity);
        }
        let (installed_identity, pending_identity) = match installed_identity {
            IdentityObservation::Resolved(identity) => (identity.ok(), None),
            IdentityObservation::TimedOut(task) => (None, Some(task)),
            IdentityObservation::Skipped => (None, None),
        };
        let executable_relation = match (running_identity, installed_identity) {
            (Some(running), Some(installed)) if running == installed => ExecutableRelation::Match,
            (Some(_), Some(_)) => ExecutableRelation::Drift,
            _ => ExecutableRelation::Unknown,
        };

        StatusObservation {
            router,
            app_server,
            remote_control,
            remote_control_identity,
            executable_relation,
            pending_identity,
        }
    })
}

enum IdentityObservation {
    Resolved(Result<ExecutableIdentity, codex_router_codex::ExecutableIdentityError>),
    TimedOut(ExecutableIdentityTask),
    Skipped,
}

async fn observe_identity(
    executable: &std::path::Path,
    deadline: std::time::Duration,
    enabled: bool,
) -> IdentityObservation {
    if !enabled {
        return IdentityObservation::Skipped;
    }
    let mut task = codex_router_codex::start_executable_identity(executable);
    match tokio::time::timeout(deadline, task.wait()).await {
        Ok(result) => IdentityObservation::Resolved(result),
        Err(_) => IdentityObservation::TimedOut(task),
    }
}

fn remote_control_status(
    observation: &RemoteControlObservation,
) -> (RemoteControlCondition, RemoteControlIdentity) {
    let (condition, server_name, environment_id) = match observation {
        RemoteControlObservation::Connected {
            server_name,
            environment_id,
        } => (
            RemoteControlCondition::Connected,
            server_name,
            environment_id,
        ),
        RemoteControlObservation::Connecting {
            server_name,
            environment_id,
        } => (
            RemoteControlCondition::Connecting,
            server_name,
            environment_id,
        ),
        RemoteControlObservation::Errored {
            server_name,
            environment_id,
        } => (RemoteControlCondition::Errored, server_name, environment_id),
        RemoteControlObservation::Disabled {
            server_name,
            environment_id,
        } => (
            RemoteControlCondition::Disabled,
            server_name,
            environment_id,
        ),
    };
    (
        condition,
        RemoteControlIdentity::new(server_name.clone(), environment_id.clone()),
    )
}

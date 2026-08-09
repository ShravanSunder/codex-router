//! Bounded connection tasks and synchronous lifecycle-request admission.

use tokio::net::UnixStream;
use tokio::sync::mpsc;

use super::*;
use crate::HostTerminalResponse;
use crate::OperatorFrame;

/// Admission decision made synchronously from current mutation ownership.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MutationAdmission {
    /// Read-only work remains available during lifecycle mutation.
    ReadOnly,
    /// The request acquires lifecycle mutation ownership.
    StartMutation(HostOperation),
    /// Another lifecycle mutation already owns serialization.
    Busy,
}

/// Classifies one request without awaiting or mutating runtime state.
#[must_use]
const fn classify_operator_request(
    active_mutation: Option<HostOperation>,
    request: OperatorRequest,
) -> MutationAdmission {
    if !request.is_mutating() {
        return MutationAdmission::ReadOnly;
    }
    if active_mutation.is_some() {
        return MutationAdmission::Busy;
    }
    let operation = match request {
        OperatorRequest::RestartAppServer => HostOperation::RestartAppServer,
        OperatorRequest::UpdateCodex => HostOperation::UpdateCodex,
        OperatorRequest::RestartRouter => HostOperation::RestartRouter,
        OperatorRequest::Status | OperatorRequest::AwaitHostStart => HostOperation::Status,
    };
    MutationAdmission::StartMutation(operation)
}

pub(super) struct OperatorWork {
    pub(super) request: OperatorRequest,
    pub(super) response: mpsc::Sender<OperatorFrame>,
}

pub(super) struct ActiveAppServerRestart {
    pub(super) future: crate::explicit_app_server_restart::AppServerRestartFuture,
    pub(super) stop_intent: crate::explicit_app_server_restart::StopIntent,
    pub(super) response: mpsc::Sender<OperatorFrame>,
    pub(super) started_at: tokio::time::Instant,
}

pub(super) struct ActiveRouterRestart {
    pub(super) future: crate::explicit_router_restart::RouterRestartFuture,
    pub(super) stop_intent: crate::explicit_app_server_restart::StopIntent,
    pub(super) response: mpsc::Sender<OperatorFrame>,
    pub(super) started_at: tokio::time::Instant,
}

pub(super) struct ActiveUpdate {
    pub(super) future: crate::codex_update_preparation::UpdateFuture,
    pub(super) response: mpsc::Sender<OperatorFrame>,
    pub(super) started_at: tokio::time::Instant,
}

pub(super) struct ActiveUpdateActivation {
    pub(super) future: crate::changed_update_activation::UpdateActivationFuture,
    pub(super) response: mpsc::Sender<OperatorFrame>,
    pub(super) replacement_command: ChildCommandSpec,
    pub(super) started_at: tokio::time::Instant,
}

pub(super) struct ActiveStatusObservation {
    pub(super) future: crate::lifecycle_owner::status_observation::StatusObservationFuture,
    pub(super) responses: Vec<(OperatorRequest, mpsc::Sender<OperatorFrame>)>,
}

pub(super) struct OperatorRuntimeContext<'a> {
    pub(super) state: &'a mut RuntimeState,
    pub(super) app_server: &'a mut Option<AppServerChild>,
    pub(super) router_child: &'a mut Option<RouterChild>,
    pub(super) config: &'a HostConfig,
    pub(super) child_launch_plans: &'a ManagedChildLaunchPlans,
    pub(super) update_inputs: &'a ManagedUpdateInputs,
    pub(super) active_app_server_restart: &'a mut Option<ActiveAppServerRestart>,
    pub(super) active_router_restart: &'a mut Option<ActiveRouterRestart>,
    pub(super) active_update: &'a mut Option<ActiveUpdate>,
    pub(super) active_status: &'a mut Option<ActiveStatusObservation>,
    pub(super) update_drain_active: bool,
}

pub(super) fn spawn_operator_connection(
    connection_tasks: &mut tokio::task::JoinSet<()>,
    mut stream: UnixStream,
    operator_sender: mpsc::Sender<OperatorWork>,
    request_deadline: std::time::Duration,
    permit: tokio::sync::OwnedSemaphorePermit,
) {
    connection_tasks.spawn(async move {
        let _permit = permit;
        let deadline_at = tokio::time::Instant::now() + request_deadline;
        let Ok(request) =
            crate::operator_connection::read_request_from_stream(&mut stream, deadline_at).await
        else {
            return;
        };
        let (response_sender, mut response_receiver) = mpsc::channel(2);
        if operator_sender
            .send(OperatorWork {
                request,
                response: response_sender,
            })
            .await
            .is_err()
        {
            return;
        }
        while let Some(frame) = response_receiver.recv().await {
            let terminal = matches!(frame, OperatorFrame::Terminal(_));
            let write_deadline_at = tokio::time::Instant::now() + request_deadline;
            if crate::operator_connection::write_frame_to_stream(
                &mut stream,
                &frame,
                write_deadline_at,
            )
            .await
            .is_err()
            {
                return;
            }
            if terminal {
                let _shutdown_result = crate::operator_connection::shutdown_operator_stream(
                    &mut stream,
                    tokio::time::Instant::now() + request_deadline,
                )
                .await;
                return;
            }
        }
    });
}

pub(super) fn handle_operator_work(work: OperatorWork, context: OperatorRuntimeContext<'_>) {
    let snapshot = context.state.snapshot();
    let phase_mutation = match &context.state.phase {
        HostPhase::Mutating { operation, .. } => Some(*operation),
        HostPhase::Starting | HostPhase::Steady | HostPhase::Stopping => None,
    };
    let active_mutation = context
        .active_app_server_restart
        .as_ref()
        .map(|_active| HostOperation::RestartAppServer)
        .or_else(|| {
            context
                .active_router_restart
                .as_ref()
                .map(|_active| HostOperation::RestartRouter)
        })
        .or_else(|| {
            context
                .active_update
                .as_ref()
                .map(|_active| HostOperation::UpdateCodex)
        })
        .or(
            (context.update_drain_active && work.request == OperatorRequest::UpdateCodex)
                .then_some(HostOperation::UpdateCodex),
        )
        .or(phase_mutation);
    if matches!(
        classify_operator_request(active_mutation, work.request),
        MutationAdmission::Busy
    ) {
        let _send_result = work.response.try_send(OperatorFrame::busy(
            work.request,
            snapshot,
            "another lifecycle mutation is active".to_owned(),
        ));
        return;
    }
    match work.request {
        OperatorRequest::Status | OperatorRequest::AwaitHostStart => {
            if let Some(active) = context.active_status.as_mut() {
                active.responses.push((work.request, work.response));
                return;
            }
            let running_identity = context
                .app_server
                .as_ref()
                .map(|child| child.identity().clone());
            *context.active_status = Some(ActiveStatusObservation {
                future: crate::lifecycle_owner::status_observation::observe_status(
                    context.config.clone(),
                    context.state.router_ownership,
                    context.router_child.is_some(),
                    running_identity,
                    context.update_inputs.update_deadlines.identity(),
                    !context.update_drain_active,
                ),
                responses: vec![(work.request, work.response)],
            });
        }
        OperatorRequest::RestartAppServer => {
            let current_child = context.app_server.take();
            context.state.phase = HostPhase::Mutating {
                operation: HostOperation::RestartAppServer,
                phase: "stopping-old-app-server".to_owned(),
            };
            context.state.app_server = if current_child.is_some() {
                AppServerCondition::Stopping
            } else {
                AppServerCondition::Starting
            };
            context.state.remote_control = RemoteControlCondition::Unavailable;
            let stop_intent = crate::explicit_app_server_restart::StopIntent::default();
            *context.active_app_server_restart = Some(ActiveAppServerRestart {
                future: crate::explicit_app_server_restart::restart_app_server(
                    context.config.clone(),
                    context.child_launch_plans.app_server.clone(),
                    current_child,
                    stop_intent.clone(),
                ),
                stop_intent,
                response: work.response,
                started_at: tokio::time::Instant::now(),
            });
        }
        OperatorRequest::RestartRouter
            if context.state.router_ownership == crate::RouterOwnership::External =>
        {
            send_terminal_response(
                work.response,
                work.request,
                TerminalClassification::Failed,
                snapshot,
                "compatible router is external and is not owned by this host",
            );
        }
        OperatorRequest::RestartRouter => {
            let current_child = context.router_child.take();
            let Some(router_command) = context.child_launch_plans.router_command.clone() else {
                *context.router_child = current_child;
                send_terminal_response(
                    work.response,
                    work.request,
                    TerminalClassification::Failed,
                    snapshot,
                    "owned router launch projection is unavailable",
                );
                return;
            };
            context.state.phase = HostPhase::Mutating {
                operation: HostOperation::RestartRouter,
                phase: "stopping-owned-router".to_owned(),
            };
            context.state.router = RouterCondition::OwnedTransitioning;
            let stop_intent = crate::explicit_app_server_restart::StopIntent::default();
            *context.active_router_restart = Some(ActiveRouterRestart {
                future: crate::explicit_router_restart::restart_router(
                    context.config.clone(),
                    router_command,
                    current_child,
                    stop_intent.clone(),
                ),
                stop_intent,
                response: work.response,
                started_at: tokio::time::Instant::now(),
            });
        }
        OperatorRequest::UpdateCodex => {
            context.state.phase = HostPhase::Mutating {
                operation: HostOperation::UpdateCodex,
                phase: "running-official-updater".to_owned(),
            };
            *context.active_update = Some(ActiveUpdate {
                future: crate::codex_update_preparation::start_update(
                    context.config.managed_executable().to_owned(),
                    context.update_inputs.update_deadlines,
                ),
                response: work.response,
                started_at: tokio::time::Instant::now(),
            });
        }
    }
}

pub(super) fn send_terminal_response(
    response_sender: mpsc::Sender<OperatorFrame>,
    request: OperatorRequest,
    classification: TerminalClassification,
    snapshot: HostSnapshot,
    message: &'static str,
) {
    let response = HostTerminalResponse::new(request, classification, snapshot, message.to_owned());
    let _send_result = response_sender.try_send(OperatorFrame::terminal(response));
}

#[cfg(test)]
mod tests {
    use tokio::io::AsyncReadExt;
    use tokio::io::AsyncWriteExt;

    use super::*;

    #[test]
    fn operator_request_admission_classifies_read_only_busy_and_new_mutation() {
        assert_eq!(
            classify_operator_request(
                Some(HostOperation::RestartAppServer),
                OperatorRequest::Status,
            ),
            MutationAdmission::ReadOnly
        );
        assert_eq!(
            classify_operator_request(
                Some(HostOperation::RestartAppServer),
                OperatorRequest::UpdateCodex,
            ),
            MutationAdmission::Busy
        );
        assert_eq!(
            classify_operator_request(None, OperatorRequest::RestartRouter),
            MutationAdmission::StartMutation(HostOperation::RestartRouter)
        );
    }

    #[tokio::test]
    async fn response_write_deadline_starts_after_long_lifecycle_work()
    -> Result<(), Box<dyn std::error::Error>> {
        let (mut client, server) = UnixStream::pair()?;
        let (operator_sender, mut operator_receiver) = mpsc::channel(1);
        let mut connection_tasks = tokio::task::JoinSet::new();
        let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
        let permit = semaphore.clone().try_acquire_owned()?;
        spawn_operator_connection(
            &mut connection_tasks,
            server,
            operator_sender,
            std::time::Duration::from_millis(20),
            permit,
        );
        client
            .write_all(&crate::encode_operator_request(
                &OperatorRequest::RestartAppServer,
            )?)
            .await?;
        client.shutdown().await?;
        let work = operator_receiver
            .recv()
            .await
            .ok_or("operator work was not admitted")?;

        tokio::time::sleep(std::time::Duration::from_millis(60)).await;
        work.response
            .send(OperatorFrame::terminal(HostTerminalResponse::new(
                OperatorRequest::RestartAppServer,
                TerminalClassification::Succeeded,
                fixture_snapshot(),
                "restart completed".to_owned(),
            )))
            .await?;
        drop(work.response);

        let mut response = Vec::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            client.read_to_end(&mut response),
        )
        .await??;
        if !matches!(
            crate::decode_operator_frame(&response)?,
            OperatorFrame::Terminal(terminal)
                if terminal.classification() == TerminalClassification::Succeeded
        ) {
            return Err("delayed lifecycle response was not delivered".into());
        }
        let _completed = connection_tasks.join_next().await;
        Ok(())
    }

    fn fixture_snapshot() -> HostSnapshot {
        HostSnapshot::new(HostSnapshotDimensions {
            phase: HostPhase::Steady,
            router: RouterCondition::OwnedReachable,
            app_server: AppServerCondition::NativeReady {
                running_version: "1.2.3".to_owned(),
            },
            remote_control: RemoteControlCondition::Connected,
            executable_relation: ExecutableRelation::Match,
            recovery_budget: RecoveryBudget::Available,
            last_lifecycle_outcome: None,
        })
    }
}

//! Bounded connection tasks and synchronous owner-loop request admission.

use tokio::net::UnixStream;
use tokio::sync::mpsc;

use super::*;
use crate::HostTerminalResponse;
use crate::OperatorFrame;

pub(super) struct OperatorWork {
    pub(super) request: OperatorRequest,
    pub(super) response: mpsc::Sender<OperatorFrame>,
}

pub(super) struct ActiveAppServerRestart {
    pub(super) future: crate::restart::AppServerRestartFuture,
    pub(super) response: mpsc::Sender<OperatorFrame>,
    pub(super) started_at: tokio::time::Instant,
}

pub(super) struct ActiveRouterRestart {
    pub(super) future: crate::restart::RouterRestartFuture,
    pub(super) response: mpsc::Sender<OperatorFrame>,
    pub(super) started_at: tokio::time::Instant,
}

pub(super) struct ActiveUpdate {
    pub(super) future: crate::update::UpdateFuture,
    pub(super) response: mpsc::Sender<OperatorFrame>,
    pub(super) started_at: tokio::time::Instant,
}

pub(super) struct ActiveUpdateActivation {
    pub(super) future: crate::update::UpdateActivationFuture,
    pub(super) response: mpsc::Sender<OperatorFrame>,
    pub(super) replacement_command: ChildCommandSpec,
    pub(super) started_at: tokio::time::Instant,
}

pub(super) struct OperatorRuntimeContext<'a> {
    pub(super) state: &'a mut RuntimeState,
    pub(super) app_server: &'a mut Option<AppServerChild>,
    pub(super) router_child: &'a mut Option<RouterChild>,
    pub(super) config: &'a HostConfig,
    pub(super) dependencies: &'a HostDependencies,
    pub(super) active_app_server_restart: &'a mut Option<ActiveAppServerRestart>,
    pub(super) active_router_restart: &'a mut Option<ActiveRouterRestart>,
    pub(super) active_update: &'a mut Option<ActiveUpdate>,
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
            crate::operator_protocol::read_request_from_stream(&mut stream, deadline_at).await
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
            if crate::operator_protocol::write_frame_to_stream(&mut stream, &frame, deadline_at)
                .await
                .is_err()
            {
                return;
            }
            if terminal {
                let _shutdown_result =
                    crate::operator_protocol::shutdown_operator_stream(&mut stream, deadline_at)
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
        .or(context
            .update_drain_active
            .then_some(HostOperation::UpdateCodex))
        .or(phase_mutation);
    if matches!(
        crate::classify_operator_request(active_mutation, work.request),
        crate::MutationAdmission::Busy
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
            let classification = match snapshot.hosted_readiness() {
                crate::HostedReadiness::Ready => TerminalClassification::Ready,
                crate::HostedReadiness::LocalReadyRemoteDegraded => {
                    TerminalClassification::LocalReadyRemoteDegraded
                }
                crate::HostedReadiness::Unavailable => TerminalClassification::Unavailable,
            };
            send_terminal_response(
                work.response,
                work.request,
                classification,
                snapshot,
                "shared Codex host status",
            );
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
            *context.active_app_server_restart = Some(ActiveAppServerRestart {
                future: crate::restart::restart_app_server(
                    context.config.clone(),
                    context.dependencies.app_server.clone(),
                    current_child,
                ),
                response: work.response,
                started_at: tokio::time::Instant::now(),
            });
        }
        OperatorRequest::RestartRouter if context.router_child.is_none() => {
            send_terminal_response(
                work.response,
                work.request,
                TerminalClassification::Failed,
                snapshot,
                "compatible router is external and is not owned by this host",
            );
        }
        OperatorRequest::RestartRouter => {
            let Some(current_child) = context.router_child.take() else {
                send_terminal_response(
                    work.response,
                    work.request,
                    TerminalClassification::Failed,
                    snapshot,
                    "owned router child is unavailable",
                );
                return;
            };
            let Some(router_command) = context.dependencies.router_command.clone() else {
                *context.router_child = Some(current_child);
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
            *context.active_router_restart = Some(ActiveRouterRestart {
                future: crate::restart::restart_router(
                    context.config.clone(),
                    router_command,
                    current_child,
                ),
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
                future: crate::update::start_update(
                    context.config.managed_executable().to_owned(),
                    context.dependencies.update_deadlines,
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

//! Changed-update activation transitions applied by the single lifecycle owner.

use super::*;
use crate::OperatorFrame;

pub(super) struct PreparationContext<'a> {
    pub(super) preparation: crate::update::UpdatePreparation,
    pub(super) active: request_admission::ActiveUpdate,
    pub(super) state: &'a mut RuntimeState,
    pub(super) dependencies: &'a HostDependencies,
    pub(super) app_server: &'a mut Option<AppServerChild>,
    pub(super) router: &'a mut Option<RouterChild>,
    pub(super) activation: &'a mut Option<request_admission::ActiveUpdateActivation>,
    pub(super) pending_identity: &'a mut Option<codex_router_codex::ExecutableIdentityTask>,
    pub(super) retained_updater: &'a mut Option<ProcessGroupChild>,
}

pub(super) fn apply_preparation(context: PreparationContext<'_>) {
    let (classification, result, message) = match context.preparation {
        crate::update::UpdatePreparation::NoChange => {
            context.state.executable_relation = ExecutableRelation::Match;
            (
                TerminalClassification::Succeeded,
                LifecycleOutcomeClassification::Succeeded,
                "managed Codex is already current",
            )
        }
        crate::update::UpdatePreparation::Changed => {
            context.state.executable_relation = ExecutableRelation::Drift;
            let _progress_result = context.active.response.try_send(OperatorFrame::Progress(
                crate::operator_protocol::HostProgress::ReplacementStarting,
            ));
            let Some(replacement_command) = context.dependencies.replacement_command.clone() else {
                context.state.phase = HostPhase::Steady;
                context.state.last_lifecycle_outcome = Some(LifecycleOutcome {
                    operation: HostOperation::UpdateCodex,
                    classification: LifecycleOutcomeClassification::Failed,
                });
                request_admission::send_terminal_response(
                    context.active.response,
                    OperatorRequest::UpdateCodex,
                    TerminalClassification::Failed,
                    context.state.snapshot(),
                    "managed Codex changed but replacement command is unavailable",
                );
                return;
            };
            context.state.phase = HostPhase::Mutating {
                operation: HostOperation::UpdateCodex,
                phase: "changed-update-teardown".to_owned(),
            };
            context.state.app_server = AppServerCondition::Stopping;
            if context.router.is_some() {
                context.state.router = RouterCondition::OwnedTransitioning;
            }
            *context.activation = Some(request_admission::ActiveUpdateActivation {
                future: crate::update::activate_changed_update(
                    context.app_server.take(),
                    context.router.take(),
                ),
                response: context.active.response,
                replacement_command,
                started_at: context.active.started_at,
            });
            return;
        }
        crate::update::UpdatePreparation::Failed(failure) => {
            context.state.executable_relation = ExecutableRelation::Unknown;
            *context.pending_identity = failure.pending_identity;
            *context.retained_updater = failure.retained_updater;
            (
                TerminalClassification::Failed,
                LifecycleOutcomeClassification::Failed,
                failure.message,
            )
        }
    };
    context.state.phase = HostPhase::Steady;
    context.state.last_lifecycle_outcome = Some(LifecycleOutcome {
        operation: HostOperation::UpdateCodex,
        classification: result,
    });
    context.state.record_lifecycle(
        HostOperation::UpdateCodex,
        if classification == TerminalClassification::Succeeded {
            "succeeded"
        } else {
            "failed"
        },
        context.active.started_at.elapsed(),
    );
    request_admission::send_terminal_response(
        context.active.response,
        OperatorRequest::UpdateCodex,
        classification,
        context.state.snapshot(),
        message,
    );
}

pub(super) struct ActivationContext<'a> {
    pub(super) completion: crate::update::UpdateActivationCompletion,
    pub(super) active: request_admission::ActiveUpdateActivation,
    pub(super) state: &'a mut RuntimeState,
    pub(super) app_server: &'a mut Option<AppServerChild>,
    pub(super) router: &'a mut Option<RouterChild>,
    pub(super) dependencies: &'a HostDependencies,
    pub(super) instance: &'a HostInstance,
}

pub(super) async fn apply_activation(context: ActivationContext<'_>) -> Result<(), HostError> {
    *context.app_server = context.completion.app_server;
    *context.router = context.completion.router;
    if !context.completion.succeeded {
        context.state.phase = HostPhase::Steady;
        context.state.app_server = if context.app_server.is_some() {
            AppServerCondition::ShutdownTimedOut
        } else {
            AppServerCondition::Absent
        };
        context.state.router = if context.router.is_some() {
            RouterCondition::OwnedTransitioning
        } else {
            RouterCondition::Unavailable
        };
        context.state.last_lifecycle_outcome = Some(LifecycleOutcome {
            operation: HostOperation::UpdateCodex,
            classification: retained_lifecycle::restart_lifecycle_classification(
                false,
                context.completion.app_server_shutdown,
            ),
        });
        request_admission::send_terminal_response(
            context.active.response,
            OperatorRequest::UpdateCodex,
            TerminalClassification::Failed,
            context.state.snapshot(),
            context.completion.message,
        );
        return Ok(());
    }

    context.state.record_lifecycle(
        HostOperation::UpdateCodex,
        if matches!(
            context.completion.app_server_shutdown,
            Some(crate::ShutdownOutcome::Forced)
        ) {
            "forced-replacement-starting"
        } else {
            "replacement-starting"
        },
        context.active.started_at.elapsed(),
    );
    lifecycle_convergence::flush_pre_exec_telemetry(
        context.dependencies.pre_exec_telemetry.clone(),
    )
    .await;
    context.instance.remove_operator_socket_for_exec()?;
    context.instance.prepare_lock_for_exec()?;
    let replacement_command = context.active.replacement_command.with_environment(
        crate::inherited_lock_environment(),
        crate::inherited_lock_marker(),
    );
    use std::os::unix::process::CommandExt;
    let error = replacement_command.std_command().exec();
    context
        .instance
        .release_prepared_lock_after_exec_failure()?;
    Err(HostError::Exec(error))
}

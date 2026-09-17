//! Managed-update preparation and shared Host replacement finalization.

use super::*;
use crate::OperatorFrame;

pub(super) struct PreparationContext<'a> {
    pub(super) preparation: crate::codex_update_preparation::UpdatePreparation,
    pub(super) active: request_admission::ActiveUpdate,
    pub(super) state: &'a mut RuntimeState,
    pub(super) update_inputs: &'a ManagedUpdateInputs,
    pub(super) app_server: &'a mut Option<AppServerChild>,
    pub(super) router: &'a mut Option<RouterChild>,
    pub(super) activation: &'a mut Option<request_admission::ActiveHostReplacement>,
    pub(super) pending_identity: &'a mut Option<codex_native_integration::ExecutableIdentityTask>,
    pub(super) retained_updater: &'a mut Option<ProcessGroupChild>,
}

pub(super) fn apply_preparation(context: PreparationContext<'_>) {
    let (classification, result, message) = match context.preparation {
        crate::codex_update_preparation::UpdatePreparation::NoChange => {
            context.state.executable_relation = ExecutableRelation::Match;
            (
                TerminalClassification::Succeeded,
                LifecycleOutcomeClassification::Succeeded,
                "managed Codex is already current",
            )
        }
        crate::codex_update_preparation::UpdatePreparation::Changed => {
            context.state.executable_relation = ExecutableRelation::Drift;
            let _progress_result = context.active.response.try_send(OperatorFrame::Progress(
                crate::operator_messages::HostProgress::ReplacementStarting,
            ));
            let Some(replacement_command) = context.update_inputs.replacement_command.clone()
            else {
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
            *context.activation = Some(request_admission::ActiveHostReplacement {
                future: crate::host_replacement_activation::activate_host_replacement(
                    context.app_server.take(),
                    context.router.take(),
                    context.active.response.clone(),
                ),
                response: context.active.response,
                replacement_command,
                request: OperatorRequest::UpdateCodex,
                operation: HostOperation::UpdateCodex,
                started_at: context.active.started_at,
                reexecuting_ack: context.active.reexecuting_ack,
            });
            return;
        }
        crate::codex_update_preparation::UpdatePreparation::Failed(failure) => {
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
    pub(super) completion: crate::host_replacement_activation::HostReplacementCompletion,
    pub(super) active: request_admission::ActiveHostReplacement,
    pub(super) state: &'a mut RuntimeState,
    pub(super) app_server: &'a mut Option<AppServerChild>,
    pub(super) router: &'a mut Option<RouterChild>,
    pub(super) update_inputs: &'a ManagedUpdateInputs,
    pub(super) instance: &'a HostInstance,
}

pub(super) async fn apply_activation(context: ActivationContext<'_>) -> Result<(), HostError> {
    *context.app_server = context.completion.app_server;
    *context.router = context.completion.router;
    if let Some(failure) = context.completion.failure {
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
            operation: context.active.operation,
            classification: retained_lifecycle::restart_lifecycle_classification(
                false,
                context.completion.app_server_shutdown,
            ),
        });
        request_admission::send_terminal_response(
            context.active.response,
            context.active.request,
            TerminalClassification::Failed,
            context.state.snapshot(),
            replacement_failure_message(context.active.operation, failure),
        );
        return Ok(());
    }

    context.state.record_lifecycle(
        context.active.operation,
        if matches!(
            context.completion.app_server_shutdown,
            Some(crate::ShutdownOutcome::ForcedDrain | crate::ShutdownOutcome::Killed)
        ) {
            "forced-replacement-starting"
        } else {
            "replacement-starting"
        },
        context.active.started_at.elapsed(),
    );
    if matches!(
        context.completion.app_server_shutdown,
        Some(crate::ShutdownOutcome::ForcedDrain | crate::ShutdownOutcome::Killed)
    ) {
        request_admission::send_progress(
            &context.active.response,
            crate::HostProgress::AppServerKilled,
        );
    }
    // Do not remove the operator socket until the writer has flushed the
    // ReExecuting frame. Queue admission alone is insufficient because exec
    // tears down the writer task with any queued bytes.
    let _ = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        context.active.reexecuting_ack,
    )
    .await;
    lifecycle_convergence::flush_pre_exec_telemetry(
        context.update_inputs.pre_exec_telemetry.clone(),
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

const fn replacement_failure_message(
    operation: HostOperation,
    failure: crate::host_replacement_activation::HostReplacementFailure,
) -> &'static str {
    match (operation, failure) {
        (
            HostOperation::RestartHost,
            crate::host_replacement_activation::HostReplacementFailure::AppServerTeardown,
        ) => "host restart app-server teardown failed",
        (
            HostOperation::RestartHost,
            crate::host_replacement_activation::HostReplacementFailure::RouterTeardown,
        ) => "host restart router teardown failed",
        (
            HostOperation::UpdateCodex,
            crate::host_replacement_activation::HostReplacementFailure::AppServerTeardown,
        ) => "updated Codex but app-server teardown failed",
        (
            HostOperation::UpdateCodex,
            crate::host_replacement_activation::HostReplacementFailure::RouterTeardown,
        ) => "updated Codex but router teardown failed",
        _ => "host replacement teardown failed",
    }
}

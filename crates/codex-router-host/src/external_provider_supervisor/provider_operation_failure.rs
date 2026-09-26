//! Map provider admission and prompt loss into the operation failure contract.

use super::{failure, runtime_failure};
use crate::ExternalProviderRuntimeError;
use collaboration_protocol::{
    ConversationOperationFailure, ConversationOperationFailureKind,
    ConversationOperationFailureStage, OperationId, ProviderOperationEffect, SessionRef,
};

pub(super) fn prompt_runtime_failure(
    operation_id: OperationId,
    target: Option<SessionRef>,
    error: ExternalProviderRuntimeError,
) -> ConversationOperationFailure {
    if matches!(error, ExternalProviderRuntimeError::TransportFailure) {
        return failure(
            ConversationOperationFailureKind::OutcomeUnknown,
            ConversationOperationFailureStage::Settlement,
            ProviderOperationEffect::Unknown,
            "provider connection lost before the agent ended the turn (providerRetired)",
            operation_id,
            target,
        );
    }
    if let ExternalProviderRuntimeError::UnknownStopReason { suffix } = error {
        let mut operation_failure = failure(
            ConversationOperationFailureKind::OutcomeUnknown,
            ConversationOperationFailureStage::Settlement,
            ProviderOperationEffect::Applied,
            "agent ended the turn with an unrecognized stop reason",
            operation_id,
            target,
        );
        if let Ok(message) = collaboration_protocol::NonEmptyText::try_from(format!(
            "agent ended the turn with an unrecognized stop reason{suffix}"
        )) {
            operation_failure.message = message;
        }
        return operation_failure;
    }
    runtime_failure(operation_id, target, error)
}

pub(super) fn admission_failure(
    operation_id: OperationId,
    message: &'static str,
    target: Option<SessionRef>,
) -> ConversationOperationFailure {
    failure(
        ConversationOperationFailureKind::Unavailable,
        ConversationOperationFailureStage::Admission,
        ProviderOperationEffect::None,
        message,
        operation_id,
        target,
    )
}

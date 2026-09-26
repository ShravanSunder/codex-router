//! Map provider admission and prompt loss into the operation failure contract.

use super::{failure, failure_with_provider_code};
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

pub(super) fn runtime_failure(
    operation_id: OperationId,
    target: Option<SessionRef>,
    error: ExternalProviderRuntimeError,
) -> ConversationOperationFailure {
    match error {
        ExternalProviderRuntimeError::LocalBusy => failure(
            ConversationOperationFailureKind::Busy,
            ConversationOperationFailureStage::Validation,
            ProviderOperationEffect::None,
            "provider conversation already has active work",
            operation_id,
            target,
        ),
        ExternalProviderRuntimeError::LocalNotFound
        | ExternalProviderRuntimeError::LocalCancelTargetMismatch => failure(
            ConversationOperationFailureKind::NotFound,
            ConversationOperationFailureStage::Validation,
            ProviderOperationEffect::None,
            "provider conversation operation is not active",
            operation_id,
            target,
        ),
        ExternalProviderRuntimeError::AuthenticationRequired { code } => {
            failure_with_provider_code(
                ConversationOperationFailureKind::AuthenticationRequired,
                ConversationOperationFailureStage::Binding,
                ProviderOperationEffect::None,
                "provider authentication is required",
                operation_id,
                target,
                Some(code),
            )
        }
        ExternalProviderRuntimeError::ProviderSessionNotFound { code } => {
            failure_with_provider_code(
                ConversationOperationFailureKind::ProviderSessionNotFound,
                ConversationOperationFailureStage::Binding,
                ProviderOperationEffect::None,
                "this session never started a turn and did not survive the provider restart; create a new conversation",
                operation_id,
                target,
                Some(code),
            )
        }
        ExternalProviderRuntimeError::ResourceNotFound { code } => failure_with_provider_code(
            ConversationOperationFailureKind::NotFound,
            ConversationOperationFailureStage::Binding,
            ProviderOperationEffect::None,
            "provider resource was not found",
            operation_id,
            target,
            Some(code),
        ),
        ExternalProviderRuntimeError::UnsupportedMethod { code } => failure_with_provider_code(
            ConversationOperationFailureKind::UnsupportedCapability,
            ConversationOperationFailureStage::Validation,
            ProviderOperationEffect::None,
            "provider ACP method is unsupported",
            operation_id,
            target,
            Some(code),
        ),
        ExternalProviderRuntimeError::InvalidParams { code } => failure_with_provider_code(
            ConversationOperationFailureKind::InvalidRequest,
            ConversationOperationFailureStage::Validation,
            ProviderOperationEffect::None,
            "provider ACP parameters are invalid",
            operation_id,
            target,
            Some(code),
        ),
        ExternalProviderRuntimeError::RequestCancelled { code } => failure_with_provider_code(
            ConversationOperationFailureKind::ProviderRejected,
            ConversationOperationFailureStage::Settlement,
            ProviderOperationEffect::Unknown,
            "provider ACP request was cancelled",
            operation_id,
            target,
            Some(code),
        ),
        ExternalProviderRuntimeError::ProviderRejected { code } => failure_with_provider_code(
            ConversationOperationFailureKind::ProviderRejected,
            ConversationOperationFailureStage::Settlement,
            ProviderOperationEffect::Unknown,
            "provider rejected the operation after dispatch",
            operation_id,
            target,
            Some(code),
        ),
        ExternalProviderRuntimeError::PromptOutputLimitExceeded => failure(
            ConversationOperationFailureKind::OutcomeUnknown,
            ConversationOperationFailureStage::Settlement,
            ProviderOperationEffect::Unknown,
            "provider prompt output exceeded the retained output limit; cancellation was requested and settled",
            operation_id,
            target,
        ),
        ExternalProviderRuntimeError::FrameLimitExceeded => failure(
            ConversationOperationFailureKind::OutcomeUnknown,
            ConversationOperationFailureStage::Settlement,
            ProviderOperationEffect::Unknown,
            "provider frame exceeded the configured transport limit",
            operation_id,
            target,
        ),
        ExternalProviderRuntimeError::FrameDecodeFailure => failure(
            ConversationOperationFailureKind::OutcomeUnknown,
            ConversationOperationFailureStage::Settlement,
            ProviderOperationEffect::Unknown,
            "provider frame could not be decoded or classified",
            operation_id,
            target,
        ),
        _ => failure(
            ConversationOperationFailureKind::OutcomeUnknown,
            ConversationOperationFailureStage::Settlement,
            ProviderOperationEffect::Unknown,
            "provider operation response was not available after dispatch",
            operation_id,
            target,
        ),
    }
}

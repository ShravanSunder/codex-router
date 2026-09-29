//! Map provider admission and prompt loss into the operation failure contract.

use super::{failure, failure_with_provider_code};
use crate::ExternalProviderRuntimeError;
use collaboration_protocol::{
    ConversationOperationFailure, ConversationOperationFailureKind,
    ConversationOperationFailureStage, InvalidProviderSetting, InvalidSettingSessionDisposition,
    NonEmptyText, OperationId, ProviderOperationEffect, SessionRef,
};

pub(super) fn invalid_setting_failure(
    operation_id: OperationId,
    target: Option<SessionRef>,
    setting: acp_client_runtime::ProviderSettingKind,
    value: String,
    advertised: Vec<String>,
    disposition: acp_client_runtime::InvalidSettingSessionDisposition,
) -> ConversationOperationFailure {
    let (session_status, disposition) = match disposition {
        acp_client_runtime::InvalidSettingSessionDisposition::Closed => (
            "new Session was closed",
            InvalidSettingSessionDisposition::Closed,
        ),
        acp_client_runtime::InvalidSettingSessionDisposition::RemainsCreated => (
            "new Session remains created and idle",
            InvalidSettingSessionDisposition::RemainsCreated,
        ),
    };
    let advertised_values = if advertised.is_empty() {
        "none".to_owned()
    } else {
        advertised.join(", ")
    };
    let message = format!(
        "invalid provider setting {}={value:?}; advertised: {advertised_values}; {session_status}",
        setting.as_str()
    );
    let mut result = failure(
        ConversationOperationFailureKind::InvalidSetting,
        ConversationOperationFailureStage::Settlement,
        ProviderOperationEffect::Applied,
        "invalid provider setting; see advertised values",
        operation_id,
        target,
    );
    if let Ok(message) = NonEmptyText::try_from(message) {
        result.message = message;
    }
    result.invalid_setting = Some(InvalidProviderSetting {
        setting: crate::provider_operation_settlement::provider_setting_name(setting),
        value,
        advertised,
        session_disposition: disposition,
    });
    result
}

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
        ExternalProviderRuntimeError::SettingsUnresolved => failure(
            ConversationOperationFailureKind::SettingsUnresolved,
            ConversationOperationFailureStage::Validation,
            ProviderOperationEffect::None,
            "provider Session settings are unresolved; set a value or accept current settings before work",
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
        ExternalProviderRuntimeError::UnsupportedContent { content_type } => failure(
            ConversationOperationFailureKind::UnsupportedCapability,
            ConversationOperationFailureStage::Validation,
            ProviderOperationEffect::None,
            match content_type {
                "image" => "unsupportedContent{image}",
                "audio" => "unsupportedContent{audio}",
                "embeddedResource" => "unsupportedContent{embeddedResource}",
                _ => "unsupportedContent{unknown}",
            },
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

#[cfg(test)]
mod tests {
    use super::*;
    use acp_client_runtime::ProviderSettingKind;
    use collaboration_protocol::ProviderSettingName;

    #[test]
    fn invalid_setting_failure_names_each_setting_and_advertised_values() {
        for (setting, name, expected_setting) in [
            (ProviderSettingKind::Mode, "mode", ProviderSettingName::Mode),
            (
                ProviderSettingKind::Model,
                "model",
                ProviderSettingName::Model,
            ),
            (
                ProviderSettingKind::Effort,
                "effort",
                ProviderSettingName::Effort,
            ),
        ] {
            let failure = invalid_setting_failure(
                OperationId::generate(),
                None,
                setting,
                "requested".to_owned(),
                vec!["first".to_owned(), "second".to_owned()],
                acp_client_runtime::InvalidSettingSessionDisposition::Closed,
            );

            assert_eq!(
                String::from(failure.message.clone()),
                format!(
                    "invalid provider setting {name}=\"requested\"; advertised: first, second; new Session was closed"
                )
            );
            let invalid_setting = failure.invalid_setting.expect("invalid-setting detail");
            assert_eq!(invalid_setting.setting, expected_setting);
            assert_eq!(invalid_setting.value, "requested");
            assert_eq!(
                invalid_setting.advertised,
                vec!["first".to_owned(), "second".to_owned()]
            );
        }
    }
}

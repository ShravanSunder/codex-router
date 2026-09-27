//! Persist applied provider effects and session metadata at one settlement boundary.
use collaboration_protocol::{
    AppliedProviderSetting, ConversationOperationFailure, ConversationOperationSettlement,
    EffectiveProviderSettings, FailedProviderSetting, MessageText, NotAppliedProviderSetting,
    OperationId, ProviderAuthenticationState, ProviderIdentity, ProviderOperationEffect,
    ProviderPromptStopReason, ProviderReconciliationState, ProviderRequestedPolicy,
    ProviderSettingName, ProviderSettingsMappingStatus, ProviderWorkingDirectory, SessionRef,
};
use collaboration_service::{
    ProviderOperationStore, ProviderOperationStoreError, ProviderSessionRecord,
};

pub(crate) enum ProviderOperationCompletion {
    Success {
        settlement: ConversationOperationSettlement,
        target: Option<SessionRef>,
        session_record: Option<Box<ProviderSessionRecord>>,
    },
    Failure(ConversationOperationFailure),
    FailureWithSession {
        failure: ConversationOperationFailure,
        session_record: Box<ProviderSessionRecord>,
    },
}

pub(crate) async fn persist_provider_success(
    store: &mut ProviderOperationStore,
    operation_id: &OperationId,
    target: Option<&SessionRef>,
    session_record: Option<&ProviderSessionRecord>,
    terminal_stop_reason: Option<ProviderPromptStopReason>,
) -> Result<(), ProviderOperationStoreError> {
    if let Some(session_record) = session_record {
        return store
            .settle_session_operation(operation_id, session_record)
            .await;
    }
    if let Some(target) = target {
        store
            .record_target(operation_id, target, chrono::Utc::now().timestamp_millis())
            .await?;
    }
    store
        .record_terminal(
            operation_id,
            ProviderOperationEffect::Applied,
            ProviderReconciliationState::Confirmed,
            terminal_stop_reason,
            chrono::Utc::now().timestamp_millis(),
        )
        .await?;
    Ok(())
}

pub(crate) fn effective_settings(
    requested_policy: ProviderRequestedPolicy,
) -> EffectiveProviderSettings {
    EffectiveProviderSettings {
        requested_policy,
        mapping_status: ProviderSettingsMappingStatus::Unverified,
        authentication: ProviderAuthenticationState::Unverified,
        provider_permission_mode: None,
        permission_outcome: None,
        mode: None,
        model: None,
        effort: None,
    }
}

pub(crate) fn provider_setting_name(
    kind: acp_client_runtime::ProviderSettingKind,
) -> ProviderSettingName {
    match kind {
        acp_client_runtime::ProviderSettingKind::Mode => ProviderSettingName::Mode,
        acp_client_runtime::ProviderSettingKind::Model => ProviderSettingName::Model,
        acp_client_runtime::ProviderSettingKind::Effort => ProviderSettingName::Effort,
    }
}

pub(crate) fn provider_applied_setting(
    setting: acp_client_runtime::AppliedProviderSetting,
) -> AppliedProviderSetting {
    AppliedProviderSetting {
        setting: provider_setting_name(setting.kind),
        value: setting.value,
    }
}

pub(crate) fn provider_failed_setting(
    setting: acp_client_runtime::FailedProviderSetting,
) -> FailedProviderSetting {
    FailedProviderSetting {
        setting: provider_setting_name(setting.kind),
        value: setting.value,
        reason: setting.reason,
    }
}

pub(crate) fn provider_not_applied_setting(
    setting: acp_client_runtime::NotAppliedProviderSetting,
) -> NotAppliedProviderSetting {
    NotAppliedProviderSetting {
        setting: provider_setting_name(setting.kind),
        value: setting.value,
    }
}

pub(crate) fn optional_message_text(output: String) -> Result<Option<MessageText>, ()> {
    if output.is_empty() {
        Ok(None)
    } else {
        MessageText::try_from(output).map(Some).map_err(|_| ())
    }
}

pub(crate) fn provider_session_record(
    target: SessionRef,
    working_directory: ProviderWorkingDirectory,
    requested_policy: ProviderRequestedPolicy,
    created_by: ProviderIdentity,
    approver: ProviderIdentity,
) -> ProviderSessionRecord {
    ProviderSessionRecord {
        target,
        working_directory,
        requested_policy,
        created_by,
        approver,
        updated_at_ms: chrono::Utc::now().timestamp_millis(),
    }
}

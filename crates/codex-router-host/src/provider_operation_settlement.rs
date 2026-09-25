//! Persist applied provider effects and session metadata at one settlement boundary.
use collaboration_protocol::{
    ConversationOperationFailure, ConversationOperationSettlement, EffectiveProviderSettings,
    MessageText, OperationId, ProviderAuthenticationState, ProviderOperationEffect,
    ProviderReconciliationState, ProviderRequestedPolicy, ProviderSettingsMappingStatus,
    ProviderWorkingDirectory, SessionRef,
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
}

pub(crate) async fn persist_provider_success(
    store: &mut ProviderOperationStore,
    operation_id: &OperationId,
    target: Option<&SessionRef>,
    session_record: Option<&ProviderSessionRecord>,
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
    created_by: SessionRef,
    approver: SessionRef,
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

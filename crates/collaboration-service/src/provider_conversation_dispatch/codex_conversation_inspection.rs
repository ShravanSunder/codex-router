//! Codex operation inspection and recovery through the shared Control surface.
use super::{LocalFailure, json_rpc_error, local_failure_response, success};
use crate::ServiceIdentity;
use collaboration_protocol::{
    ConversationBindingIdentity, ConversationOperationWaitOutput, ConversationOperationWaitRequest,
    ConversationOperationWaitResult, ConversationOutputUnavailableReason, OperationId,
};
use serde_json::Value;

pub(super) async fn codex_record(
    operation_id: &OperationId,
    identity: &ServiceIdentity,
) -> Result<Option<crate::ProviderOperationRecord>, ()> {
    let Some(store) = identity.provider_operations.as_ref() else {
        return Ok(None);
    };
    let record = store
        .lock()
        .await
        .inspect(operation_id)
        .await
        .map_err(|_| ())?;
    Ok(record.filter(|record| {
        matches!(
            &record.binding,
            ConversationBindingIdentity::CodexAcp { .. }
        )
    }))
}

pub(super) fn codex_snapshot_response(id: Value, record: crate::ProviderOperationRecord) -> Value {
    match crate::conversation_operation_snapshot(record) {
        Ok(snapshot) => success(id, snapshot),
        Err(_) => json_rpc_error(id, -32603, "Invalid stored conversation operation"),
    }
}

pub(super) async fn codex_wait(
    id: Value,
    request: &ConversationOperationWaitRequest,
    identity: &ServiceIdentity,
) -> Value {
    let Some(recorder) = identity.codex_conversation_recorder.as_ref() else {
        return local_failure_response(
            id,
            LocalFailure::Unavailable,
            request.operation_id.clone(),
            None,
        );
    };
    let deadline = tokio::time::Instant::now()
        + std::time::Duration::from_secs(u64::from(u32::from(request.timeout_seconds)));
    loop {
        let mut changed = Box::pin(recorder.changed().notified_owned());
        changed.as_mut().enable();
        let record = match codex_record(&request.operation_id, identity).await {
            Ok(Some(record)) => record,
            Ok(None) => {
                return local_failure_response(
                    id,
                    LocalFailure::InvalidBinding,
                    request.operation_id.clone(),
                    None,
                );
            }
            Err(()) => {
                return local_failure_response(
                    id,
                    LocalFailure::Unavailable,
                    request.operation_id.clone(),
                    None,
                );
            }
        };
        if record.stage == collaboration_protocol::ProviderOperationStage::Terminal
            || tokio::time::Instant::now() >= deadline
        {
            let output = if record.stage == collaboration_protocol::ProviderOperationStage::Terminal
            {
                ConversationOperationWaitOutput::OutputUnavailable {
                    reason: ConversationOutputUnavailableReason::NotRetained,
                }
            } else {
                ConversationOperationWaitOutput::Pending
            };
            return match crate::conversation_operation_snapshot(record) {
                Ok(operation) => success(id, ConversationOperationWaitResult { operation, output }),
                Err(_) => json_rpc_error(id, -32603, "Invalid stored conversation operation"),
            };
        }
        tokio::select! {
            () = &mut changed => {}
            () = tokio::time::sleep_until(deadline) => {}
        }
    }
}

pub(super) async fn codex_reconcile(
    id: Value,
    operation_id: &OperationId,
    identity: &ServiceIdentity,
) -> Value {
    let Some(store) = identity.provider_operations.as_ref() else {
        return local_failure_response(id, LocalFailure::Unavailable, operation_id.clone(), None);
    };
    let active = identity
        .codex_conversation_recorder
        .as_ref()
        .is_some_and(|recorder| recorder.is_active(operation_id));
    let mut store = store.lock().await;
    let record = match store.inspect(operation_id).await {
        Ok(Some(record)) => record,
        Ok(None) => {
            return local_failure_response(
                id,
                LocalFailure::InvalidBinding,
                operation_id.clone(),
                None,
            );
        }
        Err(_) => {
            return local_failure_response(
                id,
                LocalFailure::Unavailable,
                operation_id.clone(),
                None,
            );
        }
    };
    let updated = if record.target.is_some()
        && record.reconciliation_state
            != collaboration_protocol::ProviderReconciliationState::Confirmed
    {
        store
            .confirm_recorded_target(operation_id, chrono::Utc::now().timestamp_millis())
            .await
    } else if !active
        && record.reconciliation_state
            == collaboration_protocol::ProviderReconciliationState::Unresolved
    {
        store
            .record_not_reconcilable(operation_id, chrono::Utc::now().timestamp_millis())
            .await
    } else {
        Ok(record)
    };
    match updated {
        Ok(record) => codex_snapshot_response(id, record),
        Err(_) => local_failure_response(id, LocalFailure::Unavailable, operation_id.clone(), None),
    }
}

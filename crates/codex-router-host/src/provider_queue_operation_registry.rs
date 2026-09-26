//! In-memory operation-show records for prompts accepted by the Router FIFO.

use collaboration_protocol::{
    ConversationBindingIdentity, ConversationOperationQueueState, ConversationOperationSnapshot,
    ObservationTimestamp, OperationId, ProviderBindingIdentity, ProviderOperationEffect,
    ProviderOperationKind, ProviderOperationStage, ProviderReconciliationState, SessionRef,
};
use std::{collections::HashMap, sync::Mutex};

#[derive(Default)]
pub(crate) struct ProviderQueueOperationRegistry {
    operations: Mutex<HashMap<OperationId, QueuedProviderOperation>>,
}

#[derive(Clone)]
struct QueuedProviderOperation {
    target: SessionRef,
    binding: ProviderBindingIdentity,
    queued_at_ms: i64,
    state: ConversationOperationQueueState,
    terminal_at_ms: Option<i64>,
}

impl ProviderQueueOperationRegistry {
    pub(crate) fn record_queued(
        &self,
        operation_id: OperationId,
        target: SessionRef,
        binding: ProviderBindingIdentity,
    ) {
        self.operations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                operation_id,
                QueuedProviderOperation {
                    target,
                    binding,
                    queued_at_ms: now_ms(),
                    state: ConversationOperationQueueState::RouterQueued,
                    terminal_at_ms: None,
                },
            );
    }

    pub(crate) fn mark_not_submitted(&self, operation_id: &OperationId, reason: impl Into<String>) {
        if let Some(operation) = self
            .operations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_mut(operation_id)
        {
            operation.state = ConversationOperationQueueState::NotSubmitted {
                reason: reason.into(),
            };
            operation.terminal_at_ms = Some(now_ms());
        }
    }

    pub(crate) fn clear(&self, operation_id: &OperationId) {
        self.operations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(operation_id);
    }

    pub(crate) fn snapshot(
        &self,
        operation_id: &OperationId,
    ) -> Result<Option<ConversationOperationSnapshot>, &'static str> {
        let operation = self
            .operations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(operation_id)
            .cloned();
        let Some(operation) = operation else {
            return Ok(None);
        };
        let admitted_at = timestamp_from_millis(operation.queued_at_ms)?;
        let terminal_at = operation
            .terminal_at_ms
            .map(timestamp_from_millis)
            .transpose()?;
        Ok(Some(ConversationOperationSnapshot {
            operation_id: operation_id.clone(),
            operation: ProviderOperationKind::ConversationPrompt,
            binding: ConversationBindingIdentity::ExternalProvider {
                binding: operation.binding,
            },
            target: Some(operation.target),
            stage: if terminal_at.is_some() {
                ProviderOperationStage::Terminal
            } else {
                ProviderOperationStage::Admitted
            },
            effect: ProviderOperationEffect::None,
            reconciliation: if terminal_at.is_some() {
                ProviderReconciliationState::Confirmed
            } else {
                ProviderReconciliationState::Unresolved
            },
            terminal_stop_reason: None,
            queue_state: Some(operation.state),
            admitted_at,
            terminal_at,
        }))
    }
}

fn timestamp_from_millis(milliseconds: i64) -> Result<ObservationTimestamp, &'static str> {
    let timestamp = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(milliseconds)
        .ok_or("provider queue timestamp is outside the supported range")?
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    ObservationTimestamp::try_from(timestamp)
        .map_err(|_| "provider queue timestamp is outside the supported range")
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

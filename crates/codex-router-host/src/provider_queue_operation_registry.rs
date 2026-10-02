//! In-memory operation-show records for prompts accepted by the Router FIFO.

use collaboration_protocol::{
    ConversationBindingIdentity, ConversationOperationQueueState, ConversationOperationSnapshot,
    ObservationTimestamp, OperationId, ProviderBindingIdentity, ProviderOperationEffect,
    ProviderOperationKind, ProviderOperationStage, ProviderReconciliationState, SessionRef,
};
use std::{
    collections::HashMap,
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
};
#[cfg(test)]
use tokio::sync::Notify;

#[derive(Default)]
pub(crate) struct ProviderQueueOperationRegistry {
    operations: Mutex<HashMap<OperationId, QueuedProviderOperation>>,
    next_sequence: AtomicU64,
    #[cfg(test)]
    changed: Notify,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderQueuedInput {
    pub input_id: session_event_model::InputId,
    pub position: u64,
    pub preview: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ProviderQueueCancellationError {
    #[error("notQueued")]
    NotQueued,
}

#[derive(Clone)]
struct QueuedProviderOperation {
    target: SessionRef,
    binding: ProviderBindingIdentity,
    queued_at_ms: i64,
    state: ConversationOperationQueueState,
    terminal_at_ms: Option<i64>,
    input_id: session_event_model::InputId,
    preview: String,
    sequence: u64,
    started: bool,
}

impl ProviderQueueOperationRegistry {
    pub(crate) fn record_queued_contents(
        &self,
        operation_id: OperationId,
        target: SessionRef,
        binding: ProviderBindingIdentity,
        input_id: session_event_model::InputId,
        contents: &[session_event_model::PromptContent],
    ) -> ProviderQueuedInput {
        let preview = contents
            .iter()
            .find_map(|content| match content {
                session_event_model::PromptContent::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .map(sanitized_text_preview)
            .unwrap_or_default();
        self.record_queued_with_preview(operation_id, target, binding, input_id, preview)
    }

    fn record_queued_with_preview(
        &self,
        operation_id: OperationId,
        target: SessionRef,
        binding: ProviderBindingIdentity,
        input_id: session_event_model::InputId,
        preview: String,
    ) -> ProviderQueuedInput {
        let mut operations = self
            .operations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let position = operations
            .values()
            .filter(|operation| {
                operation.target == target
                    && !operation.started
                    && operation.state == ConversationOperationQueueState::RouterQueued
            })
            .count()
            .saturating_add(1);
        let queued = ProviderQueuedInput {
            input_id: input_id.clone(),
            position: u64::try_from(position).unwrap_or(u64::MAX),
            preview: preview.clone(),
        };
        operations.insert(
            operation_id,
            QueuedProviderOperation {
                target,
                binding,
                queued_at_ms: now_ms(),
                state: ConversationOperationQueueState::RouterQueued,
                terminal_at_ms: None,
                input_id,
                preview,
                sequence: self.next_sequence.fetch_add(1, Ordering::Relaxed),
                started: false,
            },
        );
        queued
    }

    pub(crate) fn list(&self, target: &SessionRef) -> Vec<ProviderQueuedInput> {
        let operations = self
            .operations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut pending = operations
            .values()
            .filter(|operation| {
                &operation.target == target
                    && !operation.started
                    && operation.state == ConversationOperationQueueState::RouterQueued
            })
            .collect::<Vec<_>>();
        pending.sort_by_key(|operation| operation.sequence);
        pending
            .into_iter()
            .enumerate()
            .map(|(index, operation)| ProviderQueuedInput {
                input_id: operation.input_id.clone(),
                position: u64::try_from(index + 1).unwrap_or(u64::MAX),
                preview: operation.preview.clone(),
            })
            .collect()
    }

    pub(crate) fn cancel(
        &self,
        target: &SessionRef,
        input_id: &session_event_model::InputId,
    ) -> Result<(), ProviderQueueCancellationError> {
        let mut operations = self
            .operations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let operation = operations
            .values_mut()
            .find(|operation| &operation.target == target && &operation.input_id == input_id)
            .ok_or(ProviderQueueCancellationError::NotQueued)?;
        if operation.started || operation.state != ConversationOperationQueueState::RouterQueued {
            return Err(ProviderQueueCancellationError::NotQueued);
        }
        operation.state = ConversationOperationQueueState::NotSubmitted {
            reason: "queueCancelled".into(),
        };
        operation.terminal_at_ms = Some(now_ms());
        Ok(())
    }

    pub(crate) fn try_start(&self, operation_id: &OperationId) -> bool {
        let mut operations = self
            .operations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(operation) = operations.get_mut(operation_id) else {
            return false;
        };
        if operation.started || operation.state != ConversationOperationQueueState::RouterQueued {
            return false;
        }
        operation.started = true;
        true
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
            #[cfg(test)]
            self.changed.notify_one();
        }
    }

    #[cfg(test)]
    pub(crate) async fn wait_for_not_submitted(
        &self,
        operation_id: &OperationId,
    ) -> ConversationOperationQueueState {
        loop {
            let changed = self.changed.notified();
            let state = self
                .snapshot(operation_id)
                .expect("queued operation snapshot")
                .expect("queued operation remains inspectable")
                .queue_state;
            if let Some(state @ ConversationOperationQueueState::NotSubmitted { .. }) = state {
                return state;
            }
            changed.await;
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
            input_id: Some(operation.input_id),
            admitted_at,
            terminal_at,
        }))
    }
}

fn sanitized_text_preview(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|character| !character.is_control())
        .take(80)
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    fn fixture() -> (SessionRef, ProviderBindingIdentity) {
        let target: SessionRef = serde_json::from_value(serde_json::json!({
            "endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"cursor-local"},
            "sessionId":"provider-session"
        })).expect("target");
        let binding = serde_json::from_value(serde_json::json!({
            "endpoint":target.endpoint,"bindingId":"binding",
            "runtime":{"provider":"cursor","runtimeName":"fixture"},
            "transport":"stdioAcp","generation":{"serviceEpoch":"00000000-0000-4000-8000-000000000002","generation":1},
            "capabilities":[{"name":"prompt","status":"supported","evidence":"advertised"}]
        })).expect("binding");
        (target, binding)
    }

    #[test]
    fn list_keeps_admission_order_and_cancel_excludes_only_one_input() {
        let registry = ProviderQueueOperationRegistry::default();
        let (target, binding) = fixture();
        let first = (
            OperationId::generate(),
            session_event_model::InputId::generate(),
        );
        let second = (
            OperationId::generate(),
            session_event_model::InputId::generate(),
        );
        let first_contents =
            [session_event_model::PromptContent::text("first\n  item".into()).expect("text")];
        registry.record_queued_contents(
            first.0.clone(),
            target.clone(),
            binding.clone(),
            first.1.clone(),
            &first_contents,
        );
        let second_contents =
            [session_event_model::PromptContent::text("second item".into()).expect("text")];
        registry.record_queued_contents(
            second.0.clone(),
            target.clone(),
            binding,
            second.1.clone(),
            &second_contents,
        );
        let listed = registry.list(&target);
        assert_eq!(
            listed
                .iter()
                .map(|item| (&item.input_id, item.position))
                .collect::<Vec<_>>(),
            vec![(&first.1, 1), (&second.1, 2)]
        );
        assert_eq!(listed[0].preview, "first item");
        registry.cancel(&target, &first.1).expect("cancel first");
        assert_eq!(
            registry.cancel(&target, &first.1),
            Err(ProviderQueueCancellationError::NotQueued)
        );
        assert!(!registry.try_start(&first.0));
        assert_eq!(registry.list(&target)[0].input_id, second.1);
    }

    #[test]
    fn content_preview_uses_first_text_block_after_non_text_content() {
        let registry = ProviderQueueOperationRegistry::default();
        let (target, binding) = fixture();
        let input_id = session_event_model::InputId::generate();
        let contents = vec![
            session_event_model::PromptContent::resource_link(
                "https://example.test/context".into(),
                "context".into(),
                None,
            )
            .expect("resource link"),
            session_event_model::PromptContent::text("  first\n  text\tblock  ".into())
                .expect("text"),
            session_event_model::PromptContent::text("second text".into()).expect("text"),
        ];
        registry.record_queued_contents(
            OperationId::generate(),
            target.clone(),
            binding,
            input_id,
            &contents,
        );
        assert_eq!(registry.list(&target)[0].preview, "first text block");
    }

    #[test]
    fn cancel_and_start_race_has_one_winner() {
        let registry = Arc::new(ProviderQueueOperationRegistry::default());
        let (target, binding) = fixture();
        let operation_id = OperationId::generate();
        let input_id = session_event_model::InputId::generate();
        let contents =
            [session_event_model::PromptContent::text("racing input".into()).expect("text")];
        registry.record_queued_contents(
            operation_id.clone(),
            target.clone(),
            binding,
            input_id.clone(),
            &contents,
        );
        let barrier = Arc::new(Barrier::new(3));
        let started = {
            let registry = Arc::clone(&registry);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                registry.try_start(&operation_id)
            })
        };
        let cancelled = {
            let registry = Arc::clone(&registry);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                registry.cancel(&target, &input_id).is_ok()
            })
        };
        barrier.wait();
        assert_ne!(
            started.join().expect("start result"),
            cancelled.join().expect("cancel result")
        );
    }
}

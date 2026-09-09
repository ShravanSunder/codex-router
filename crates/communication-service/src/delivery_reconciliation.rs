//! Positive queue evidence can recover acceptance; missing entries never authorize retransmission.
use crate::{NativeAdmission, NativeControlBackend};
use agent_automation::{DeliveryStatus, PreparationEffect, SubmissionEffect};
use automation_storage::{
    AutomationStore, DeliveryCompletion, DeliveryRecord, DeliveryResult, StorageError,
};
use codex_native_integration::{NativeConnectionError, NativeOperation, NativeProtocolConnection};
use communication_protocol::{
    AcceptedResumeEffect, CodexGeneration, MessageContent, NativeSendAcceptance, NativeSendReceipt,
    SessionRef,
};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::Mutex;

pub(crate) async fn reconcile(
    store: &Arc<Mutex<AutomationStore>>,
    backend: Option<&NativeControlBackend>,
    record: DeliveryRecord<SessionRef, CodexGeneration, NativeSendReceipt>,
) -> Result<(), StorageError> {
    if record.status != DeliveryStatus::Uncertain {
        return Ok(());
    }
    let Some(attempt) = record.attempt else {
        return Err(StorageError::InvalidRecord);
    };
    // Auto dispatch does not durably record whether start or steer won its state check.
    // A queue receipt requires both the original operation and its server submission ID.
    if record.mode != "queue"
        || attempt.effects.resume != PreparationEffect::NotRequested
        || attempt.effects.allocation != PreparationEffect::NotRequested
        || attempt.effects.target.as_ref() != Some(&record.target)
    {
        return Ok(());
    }
    let (Some(generation), Some(correlation)) = (
        &attempt.effects.generation,
        &attempt.effects.client_user_message_id,
    ) else {
        return Ok(());
    };
    let Some(backend) = backend.filter(|backend| backend.endpoint == record.target.endpoint) else {
        return Ok(());
    };
    let Ok(admission) = backend.gate.acquire() else {
        return Ok(());
    };
    let content = store
        .lock()
        .await
        .read_delivery_content::<MessageContent>(&record.delivery_id)
        .await?;
    let rendered = crate::agent_declaration::render_message(&record.target, &content)
        .map_err(|_| StorageError::InvalidRecord)?;
    let retirement = admission.retirement();
    let observed = tokio::select! {
        biased;
        _ = retirement.cancelled() => return Ok(()),
        observed = tokio::time::timeout(std::time::Duration::from_secs(20), find_queued_input(&admission, &record.target, correlation, &rendered.text)) => observed,
    };
    let Ok(Ok(Some(submission_id))) = observed else {
        return Ok(());
    };
    if retirement.is_cancelled() {
        return Ok(());
    }
    let receipt = NativeSendReceipt {
        target: record.target,
        generation: generation.clone(),
        input_kind: rendered.kind,
        representation: rendered.representation,
        client_user_message_id: correlation
            .clone()
            .try_into()
            .map_err(|_| StorageError::InvalidRecord)?,
        resume_effect: AcceptedResumeEffect::NotRequested,
        acceptance: NativeSendAcceptance::QueueAccepted {
            submission_id: submission_id
                .clone()
                .try_into()
                .map_err(|_| StorageError::InvalidRecord)?,
        },
    };
    let mut effects = attempt.effects;
    effects.submission = SubmissionEffect::Accepted;
    effects.native_submission_id = Some(submission_id);
    store
        .lock()
        .await
        .complete_delivery(DeliveryCompletion {
            delivery_id: record.delivery_id,
            attempt_id: attempt.attempt_id,
            effects,
            result: DeliveryResult::Accepted { receipt },
            now_ms: chrono::Utc::now().timestamp_millis(),
        })
        .await?;
    Ok(())
}

async fn find_queued_input(
    admission: &NativeAdmission,
    target: &SessionRef,
    correlation: &str,
    text: &str,
) -> Result<Option<String>, NativeConnectionError> {
    let schemas = admission
        .schemas()
        .ok_or(NativeConnectionError::InvalidInput)?;
    if !schemas.supports_operation(NativeOperation::QueueList) {
        return Ok(None);
    }
    let mut connection = NativeProtocolConnection::connect(admission.backend_path()).await?;
    let mut cursor: Option<String> = None;
    let mut cursors = std::collections::HashSet::new();
    let mut matched = None;
    for _ in 0..100 {
        let page = connection.request_validated(&schemas, NativeOperation::QueueList,
            json!({"threadId":String::from(target.session_id.clone()),"cursor":cursor,"limit":100})).await?;
        let records = page
            .get("data")
            .and_then(Value::as_array)
            .ok_or(NativeConnectionError::Protocol)?;
        for record in records {
            if record.get("clientUserMessageId").and_then(Value::as_str) != Some(correlation) {
                continue;
            }
            // Correlation is client supplied. Require the full ordinary-text input and uniqueness.
            let input = record
                .get("input")
                .and_then(Value::as_array)
                .ok_or(NativeConnectionError::Protocol)?;
            let [input] = input.as_slice() else {
                return Ok(None);
            };
            if matched.is_some()
                || input.get("type").and_then(Value::as_str) != Some("text")
                || input.get("text").and_then(Value::as_str) != Some(text)
                || input
                    .get("text_elements")
                    .or_else(|| input.get("textElements"))
                    .is_some_and(|elements| {
                        elements
                            .as_array()
                            .is_none_or(|elements| !elements.is_empty())
                    })
            {
                return Ok(None);
            }
            matched = Some(
                record
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty())
                    .ok_or(NativeConnectionError::Protocol)?
                    .to_owned(),
            );
        }
        match page.get("nextCursor") {
            Some(Value::Null) => return Ok(matched),
            Some(Value::String(next)) if cursors.insert(next.clone()) => {
                cursor = Some(next.clone())
            }
            _ => return Err(NativeConnectionError::Protocol),
        }
    }
    // Bounded partial scans cannot establish uniqueness.
    Ok(None)
}

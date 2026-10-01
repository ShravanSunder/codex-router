//! Only a unique exact queue item can resolve an uncertain Codex submission.
use crate::{
    AttemptReconciliation, AttemptReconciliationContext, NativeAdmission, NativeControlBackend,
};
use agent_automation::{PreparationEffect, RouteEffectEvidence};
use codex_native_integration::{NativeConnectionError, NativeOperation, NativeProtocolConnection};
use collaboration_protocol::{
    AcceptedResumeEffect, DeliveryReceipt, MessageDelivery, MessageInputKind,
    MessageRepresentation, MessageText, NativeSendAcceptance, NativeSendReceipt, PushId,
    RouterLink, SessionRef,
};
use serde_json::{Value, json};

pub(crate) fn prepared_push_id_from_line(line: &MessageText) -> Option<PushId> {
    collaboration_protocol::parse_push_line_header(line.as_str())?;
    let (_, link_text) = line.as_str().rsplit_once(" · ")?;
    let link = RouterLink::parse(link_text).ok()?;
    Some(link.push_id().clone())
}

pub(crate) async fn reconcile(
    backend: &NativeControlBackend,
    context: AttemptReconciliationContext,
) -> AttemptReconciliation {
    let AttemptReconciliationContext {
        target,
        message,
        prepared_push_id,
        mode,
        recorded,
    } = context;
    let RouteEffectEvidence::CodexAppServer(native) = recorded else {
        return AttemptReconciliation::StillUnknown;
    };
    if mode != MessageDelivery::Queue
        || native.resume != PreparationEffect::NotRequested
        || native.allocation != PreparationEffect::NotRequested
        || native.target.as_ref() != Some(&target)
    {
        return AttemptReconciliation::StillUnknown;
    }
    let (Some(generation), Some(correlation)) = (native.generation, native.client_user_message_id)
    else {
        return AttemptReconciliation::StillUnknown;
    };
    if prepared_push_id
        .as_ref()
        .is_some_and(|push_id| push_id.as_str() != correlation)
    {
        return AttemptReconciliation::StillUnknown;
    }
    if backend.endpoint != target.endpoint {
        return AttemptReconciliation::StillUnknown;
    }
    let Ok(admission) = backend.gate.acquire() else {
        return AttemptReconciliation::StillUnknown;
    };
    let (input_kind, representation) = if prepared_push_id.is_some() {
        (
            MessageInputKind::Agent,
            MessageRepresentation::DeclaredAgentText,
        )
    } else {
        let Ok(rendered) = collaboration_protocol::render_message(&target, &message) else {
            return AttemptReconciliation::StillUnknown;
        };
        (rendered.kind, rendered.representation)
    };
    let retirement = admission.retirement();
    let observed = tokio::select! {
        biased;
        () = retirement.cancelled() => return AttemptReconciliation::StillUnknown,
        observed = tokio::time::timeout(std::time::Duration::from_secs(20), find_queued_input(&admission, &target, &correlation, &message, prepared_push_id.is_some())) => observed,
    };
    let Ok(Ok(Some(submission_id))) = observed else {
        return AttemptReconciliation::StillUnknown;
    };
    if retirement.is_cancelled() {
        return AttemptReconciliation::StillUnknown;
    }
    let Ok(client_user_message_id) = correlation.try_into() else {
        return AttemptReconciliation::StillUnknown;
    };
    let Ok(submission_id) = submission_id.try_into() else {
        return AttemptReconciliation::StillUnknown;
    };
    let receipt = NativeSendReceipt {
        target,
        generation,
        input_kind,
        representation,
        client_user_message_id,
        resume_effect: AcceptedResumeEffect::NotRequested,
        acceptance: NativeSendAcceptance::QueueAccepted { submission_id },
    };
    AttemptReconciliation::Accepted(Box::new(DeliveryReceipt::from(receipt)))
}

async fn find_queued_input(
    admission: &NativeAdmission,
    target: &SessionRef,
    correlation: &str,
    message: &collaboration_protocol::MessageContent,
    prepared_push: bool,
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
            let input = record
                .get("input")
                .and_then(Value::as_array)
                .ok_or(NativeConnectionError::Protocol)?;
            let [input] = input.as_slice() else {
                return Ok(None);
            };
            if matched.is_some()
                || input.get("type").and_then(Value::as_str) != Some("text")
                || !input
                    .get("text")
                    .and_then(Value::as_str)
                    .is_some_and(|text| {
                        prepared_push
                            || collaboration_protocol::queued_message_matches_content(
                                target, message, text,
                            )
                    })
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
    Ok(None)
}

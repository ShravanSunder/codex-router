//! Store typed approval and question notices before delivering their push line.

use super::{InteractionHistoryError, InteractionPushContext, SessionMessageDelivery};
use crate::{
    DeliveryPrecondition, LoadPolicy, session_delivery_contract::UnstoredAttemptEvidenceSink,
};
use agent_automation::AttemptId;
use collaboration_protocol::{
    ApprovalOptionScope, ApprovalRequestRecord, DeliveryCorrelationId, DeliveryOutcome,
    DeliveryReceipt, EndpointId, EndpointRef, InteractionId, InteractionPresentationId, MachineId,
    MessageDelivery, MessageText, PushHeaderFacts, PushId, PushKind, PushLineInput, PushOrigin,
    PushRecord, PushRecordDraft, RouterLink, RouterOriginRef, SessionDisplayNameLookup, SessionId,
    SessionRef as ProtocolSessionRef, UuidIdentity, render_push_line,
};
use message_board::{Identity, SessionRef};
use session_event_model::{
    ApprovalEffect, ApprovalRequest, ApprovalScope, ApprovalSubject, QuestionField, QuestionRequest,
};
use std::{sync::Arc, time::Duration};

pub(super) async fn deliver_approval_notice(
    delivery: Option<&Arc<dyn SessionMessageDelivery>>,
    display_names: &crate::SessionDisplayNameCache,
    push_context: Option<&InteractionPushContext>,
    requester: &SessionRef,
    approver: &Identity,
    request: &ApprovalRequest,
) -> Result<(), InteractionHistoryError> {
    let Identity::Session {
        session: approver_session,
    } = approver
    else {
        return Ok(());
    };
    deliver_notice(
        delivery,
        display_names,
        push_context,
        requester,
        approver_session,
        TypedInteractionNotice::Approval(request),
    )
    .await
}

pub(super) async fn deliver_question_notice(
    delivery: Option<&Arc<dyn SessionMessageDelivery>>,
    display_names: &crate::SessionDisplayNameCache,
    push_context: Option<&InteractionPushContext>,
    requester: &SessionRef,
    approver: &Identity,
    request: &QuestionRequest,
) -> Result<(), InteractionHistoryError> {
    let Identity::Session {
        session: approver_session,
    } = approver
    else {
        return Ok(());
    };
    deliver_notice(
        delivery,
        display_names,
        push_context,
        requester,
        approver_session,
        TypedInteractionNotice::Question(request),
    )
    .await
}

async fn deliver_notice(
    delivery: Option<&Arc<dyn SessionMessageDelivery>>,
    display_names: &crate::SessionDisplayNameCache,
    push_context: Option<&InteractionPushContext>,
    requester: &SessionRef,
    approver: &SessionRef,
    notice: TypedInteractionNotice<'_>,
) -> Result<(), InteractionHistoryError> {
    let delivery = delivery.ok_or(InteractionHistoryError::Unavailable)?;
    let push_context = push_context.ok_or(InteractionHistoryError::Unavailable)?;
    let requester = protocol_session_ref(requester)?;
    let target = protocol_session_ref(approver)?;
    let kind = notice.push_kind();
    let body = notice.body(approver)?;
    let origin_router_ref = interaction_origin_ref(notice.request_id())?
        .canonical_string()
        .map_err(|_| InteractionHistoryError::Unavailable)?;
    let requester_display_name = display_names.display_name_for(&requester).ok().flatten();
    let push_id = PushId::try_from(uuid::Uuid::now_v7().hyphenated().to_string())
        .map_err(|_| InteractionHistoryError::Unavailable)?;
    let draft = PushRecordDraft {
        mode: None,
        guard: None,
        push_id,
        kind,
        origin: PushOrigin::Router(kind),
        origin_router_ref: Some(origin_router_ref),
        target: target.clone(),
        reply_to_push_id: None,
        header_facts: match kind {
            PushKind::Approval => PushHeaderFacts::Approval {
                requester,
                requester_display_name,
            },
            PushKind::Question => PushHeaderFacts::Question {
                requester,
                requester_display_name,
            },
            _ => return Err(InteractionHistoryError::Unavailable),
        },
        body: Some(body),
        activity: None,
        created_at: chrono::Utc::now(),
    };

    let outcome = deliver_interaction_push_record(delivery.as_ref(), push_context, draft).await?;
    match outcome {
        DeliveryOutcome::Started
        | DeliveryOutcome::Steered
        | DeliveryOutcome::StartedOrSteered
        | DeliveryOutcome::Queued
        | DeliveryOutcome::PeerMessageWritten
        | DeliveryOutcome::Unknown => Ok(()),
        DeliveryOutcome::NotSubmitted { .. } | DeliveryOutcome::Rejected(_) => {
            Err(InteractionHistoryError::Unavailable)
        }
    }
}

pub(super) async fn deliver_legacy_approval_record_notice(
    delivery: &dyn SessionMessageDelivery,
    display_names: &crate::SessionDisplayNameCache,
    push_context: &InteractionPushContext,
    request: &ApprovalRequestRecord,
) -> Result<DeliveryOutcome, InteractionHistoryError> {
    let body = legacy_approval_notice_body(request)?;
    let requester_display_name = display_names
        .display_name_for(&request.requester)
        .ok()
        .flatten();
    let origin_router_ref = interaction_origin_ref(&request.request_id)?
        .canonical_string()
        .map_err(|_| InteractionHistoryError::Unavailable)?;
    let push_id = PushId::try_from(uuid::Uuid::now_v7().hyphenated().to_string())
        .map_err(|_| InteractionHistoryError::Unavailable)?;
    let draft = PushRecordDraft {
        mode: None,
        guard: None,
        push_id,
        kind: PushKind::Approval,
        origin: PushOrigin::Router(PushKind::Approval),
        origin_router_ref: Some(origin_router_ref),
        target: request.approver.clone(),
        reply_to_push_id: None,
        header_facts: PushHeaderFacts::Approval {
            requester: request.requester.clone(),
            requester_display_name,
        },
        body: Some(body),
        activity: None,
        created_at: chrono::Utc::now(),
    };
    deliver_interaction_push_record(delivery, push_context, draft).await
}

async fn deliver_interaction_push_record(
    delivery: &dyn SessionMessageDelivery,
    push_context: &InteractionPushContext,
    draft: PushRecordDraft,
) -> Result<DeliveryOutcome, InteractionHistoryError> {
    let record = push_context
        .store
        .lock()
        .await
        .insert_push_record(draft)
        .await
        .map_err(|_| InteractionHistoryError::Unavailable)?;
    let prepared = prepared_interaction_push(push_context, &record)?;
    push_context
        .store
        .lock()
        .await
        .mark_push_attempted(&prepared.push_id)
        .await
        .map_err(|_| InteractionHistoryError::Unavailable)?;

    let correlation = DeliveryCorrelationId::try_from(prepared.push_id.as_str().to_owned())
        .map_err(|_| InteractionHistoryError::Unavailable)?;
    let request = crate::layer_zero::DeliveryRequest {
        payload: prepared,
        target: record.target.clone(),
        mode: MessageDelivery::Auto,
        precondition: DeliveryPrecondition::Unpinned,
        correlation,
        attempt: AttemptId::generate(),
    };
    let evidence = UnstoredAttemptEvidenceSink;
    let receipt = match tokio::time::timeout(
        super::APPROVAL_TIMEOUT,
        delivery.deliver(request, &evidence),
    )
    .await
    {
        Ok(Ok(receipt)) => receipt,
        Ok(Err(_)) | Err(_) => unknown_receipt(),
    };
    settle_interaction_push(push_context, &record.push_id, &receipt).await;
    Ok(receipt.outcome)
}

fn prepared_interaction_push(
    push_context: &InteractionPushContext,
    record: &PushRecord,
) -> Result<crate::layer_zero::PreparedPush, InteractionHistoryError> {
    let line = render_push_line(&PushLineInput {
        link: RouterLink::new(
            MachineId::from(push_context.machine_identity.service_id().clone()),
            record.push_id.clone(),
        ),
        machine_label: push_context.machine_identity.machine_label().clone(),
        origin: record.origin.clone(),
        header_facts: record.header_facts.clone(),
        body: record.body.clone(),
    })
    .map_err(|_| InteractionHistoryError::Unavailable)?;
    let line = MessageText::try_from(line).map_err(|_| InteractionHistoryError::Unavailable)?;
    Ok(crate::layer_zero::PreparedPush {
        push_id: record.push_id.clone(),
        line,
        load_policy: LoadPolicy::MayLoad,
    })
}

async fn settle_interaction_push(
    push_context: &InteractionPushContext,
    push_id: &PushId,
    receipt: &DeliveryReceipt,
) {
    let mut delay = Duration::from_secs(1);
    loop {
        let result = push_context
            .store
            .lock()
            .await
            .settle_push_record(push_id, receipt.clone(), chrono::Utc::now())
            .await;
        if result.is_ok() {
            return;
        }
        tracing::warn!(
            push_id = push_id.as_str(),
            "retrying interaction push settlement without redelivery"
        );
        tokio::time::sleep(delay).await;
        delay = delay.saturating_mul(2).min(Duration::from_secs(30));
    }
}

fn unknown_receipt() -> DeliveryReceipt {
    DeliveryReceipt {
        outcome: DeliveryOutcome::Unknown,
        reachability: None,
        client: None,
    }
}

fn interaction_origin_ref(request_id: &str) -> Result<RouterOriginRef, InteractionHistoryError> {
    let interaction_id = InteractionId::try_from(request_id.to_owned())
        .map_err(|_| InteractionHistoryError::Unavailable)?;
    Ok(RouterOriginRef::Interaction {
        interaction_id,
        presentation_id: InteractionPresentationId::generate(),
    })
}

fn protocol_session_ref(
    session: &SessionRef,
) -> Result<ProtocolSessionRef, InteractionHistoryError> {
    Ok(ProtocolSessionRef {
        endpoint: EndpointRef {
            service_id: UuidIdentity::try_from(session.endpoint.service_id.as_str().to_owned())
                .map_err(|_| InteractionHistoryError::Unavailable)?,
            endpoint_id: EndpointId::try_from(session.endpoint.endpoint_id.as_str().to_owned())
                .map_err(|_| InteractionHistoryError::Unavailable)?,
        },
        session_id: SessionId::try_from(session.session_id.as_str().to_owned())
            .map_err(|_| InteractionHistoryError::Unavailable)?,
    })
}

enum TypedInteractionNotice<'a> {
    Approval(&'a ApprovalRequest),
    Question(&'a QuestionRequest),
}

impl TypedInteractionNotice<'_> {
    fn push_kind(&self) -> PushKind {
        match self {
            Self::Approval(_) => PushKind::Approval,
            Self::Question(_) => PushKind::Question,
        }
    }

    fn request_id(&self) -> &str {
        match self {
            Self::Approval(request) => &request.request_id,
            Self::Question(request) => &request.request_id,
        }
    }

    fn body(&self, approver: &SessionRef) -> Result<String, InteractionHistoryError> {
        match self {
            Self::Approval(request) => approval_notice_body(request, approver),
            Self::Question(request) => question_notice_body(request, approver),
        }
    }
}

fn legacy_approval_notice_body(
    request: &ApprovalRequestRecord,
) -> Result<String, InteractionHistoryError> {
    let presentation = request.presentation.as_ref();
    let title = presentation
        .and_then(|presentation| presentation.title.as_deref())
        .filter(|title| !title.trim().is_empty())
        .or_else(|| {
            presentation
                .and_then(|presentation| presentation.tool_name.as_deref())
                .filter(|tool_name| !tool_name.trim().is_empty())
        })
        .unwrap_or("Approval required");
    let actor = serde_json::to_string(&request.approver)
        .map_err(|_| InteractionHistoryError::Unavailable)?;
    let mut body = format!(
        "Approval request: {title}\nRequest ID: {}",
        request.request_id
    );
    if let Some(reason) = request.reason.as_deref() {
        body.push_str("\nReason: ");
        body.push_str(reason);
    }
    if let Some(presentation) = presentation {
        if let Some(tool_name) = presentation
            .tool_name
            .as_deref()
            .filter(|tool_name| !tool_name.trim().is_empty() && *tool_name != title)
        {
            body.push_str("\nTool: ");
            body.push_str(tool_name);
        }
        if let Some(kind) = presentation.kind.as_deref() {
            body.push_str("\nKind: ");
            body.push_str(kind);
        }
        for argument in &presentation.arguments {
            body.push_str(&format!("\n{}: {}", argument.name, argument.value));
        }
        for permission_detail in &presentation.permission_details {
            body.push_str("\nPermission: ");
            body.push_str(permission_detail);
        }
    }
    if request.offered_options.is_empty() {
        body.push_str("\n\nTo decide: agent-collaboration approval decide --request-id ");
        body.push_str(&shell_quote(&request.request_id));
        body.push_str(" --actor ");
        body.push_str(&shell_quote(&actor));
        body.push_str(" --decision <allow|deny>");
    } else {
        body.push_str("\n\nOptions:");
        for option in &request.offered_options {
            let label = option.label.as_deref().unwrap_or(&option.option_id);
            body.push_str(&format!(
                "\n- {label} ({})\n  To choose: agent-collaboration approval decide --request-id {} --actor {} --option-id {}",
                legacy_approval_scope_label(&option.scope),
                shell_quote(&request.request_id),
                shell_quote(&actor),
                shell_quote(&option.option_id),
            ));
        }
    }
    body.push_str("\nExpires at: ");
    body.push_str(&request.expires_at);
    Ok(body)
}

fn legacy_approval_scope_label(scope: &ApprovalOptionScope) -> String {
    match scope {
        ApprovalOptionScope::AllowOnce => "allow once".to_owned(),
        ApprovalOptionScope::AllowForSession => "allow for session".to_owned(),
        ApprovalOptionScope::AllowAlways => "always allow".to_owned(),
        ApprovalOptionScope::RejectOnce => "reject once".to_owned(),
        ApprovalOptionScope::RejectAlways => "always reject".to_owned(),
        ApprovalOptionScope::Unsupported { provider_kind } => {
            format!("unsupported by {provider_kind}")
        }
    }
}

fn approval_notice_body(
    request: &ApprovalRequest,
    approver: &SessionRef,
) -> Result<String, InteractionHistoryError> {
    let actor =
        serde_json::to_string(approver).map_err(|_| InteractionHistoryError::Unavailable)?;
    let mut body = format!(
        "Approval request: {}\nRequest ID: {}",
        request.title, request.request_id
    );
    if let Some(description) = &request.description {
        body.push_str("\nDescription: ");
        body.push_str(description);
    }
    if let Some(subject) = &request.subject {
        append_approval_subject(&mut body, subject);
    }
    for option in request.options.iter() {
        let effect = approval_effect_label(option.choice.effect);
        let scope = approval_scope_label(&option.choice.scope);
        body.push_str(&format!(
            "\n\nOption {} — {} ({effect}, {scope})\nTo choose: {}",
            option.option_id.as_str(),
            option.label,
            approval_decision_command(
                &request.request_id,
                &actor,
                option.option_id.as_str(),
                matches!(&option.choice.scope, ApprovalScope::Persistent { .. }),
            )
        ));
    }
    Ok(body)
}

fn append_approval_subject(body: &mut String, subject: &ApprovalSubject) {
    match subject {
        ApprovalSubject::ToolCall { tool_call } => {
            body.push_str(&format!(
                "\nTool call: {} ({}, id {})",
                tool_call.title, tool_call.kind, tool_call.tool_call_id
            ));
        }
        ApprovalSubject::Command { command, cwd } => {
            body.push_str(&format!("\nCommand: {command}\nWorking directory: {cwd}"));
        }
        ApprovalSubject::Plan {
            tool_call_id,
            plan_item_id,
        } => {
            body.push_str(&format!(
                "\nPlan item: {plan_item_id} (tool call {tool_call_id})"
            ));
        }
    }
}

fn approval_decision_command(
    request_id: &str,
    actor: &str,
    option_id: &str,
    acknowledge_persistent: bool,
) -> String {
    format!(
        "agent-collaboration approval decide --request-id {} --actor {} --option-id {}{}",
        shell_quote(request_id),
        shell_quote(actor),
        shell_quote(option_id),
        if acknowledge_persistent {
            " --acknowledge-persistent"
        } else {
            ""
        },
    )
}

fn approval_effect_label(effect: ApprovalEffect) -> &'static str {
    match effect {
        ApprovalEffect::Allow => "allow",
        ApprovalEffect::Decline => "decline",
        ApprovalEffect::Abort => "abort",
    }
}

fn approval_scope_label(scope: &ApprovalScope) -> String {
    match scope {
        ApprovalScope::Once => "once".to_owned(),
        ApprovalScope::Session => "this session".to_owned(),
        ApprovalScope::Persistent { where_stored } => {
            format!("persistent in {}", where_stored.as_str())
        }
    }
}

fn question_notice_body(
    request: &QuestionRequest,
    approver: &SessionRef,
) -> Result<String, InteractionHistoryError> {
    let actor =
        serde_json::to_string(approver).map_err(|_| InteractionHistoryError::Unavailable)?;
    let mut body = format!(
        "Question: {}\nRequest ID: {}\nFields:",
        request.prompt, request.request_id
    );
    for field in request.fields.iter() {
        append_question_field(&mut body, field);
    }
    body.push_str("\n\nTo answer: ");
    body.push_str(&format!(
        "agent-collaboration question answer --request-id {} --actor {} --content '<answers-json>'",
        shell_quote(&request.request_id),
        shell_quote(&actor),
    ));
    Ok(body)
}

fn append_question_field(body: &mut String, field: &QuestionField) {
    let (kind, field_id, label, description, required) = match field {
        QuestionField::Text {
            field_id,
            label,
            description,
            required,
        } => ("text", field_id, label, description, required),
        QuestionField::Number {
            field_id,
            label,
            description,
            required,
        } => ("number", field_id, label, description, required),
        QuestionField::Boolean {
            field_id,
            label,
            description,
            required,
        } => ("boolean", field_id, label, description, required),
        QuestionField::SingleChoice {
            field_id,
            label,
            description,
            required,
            ..
        } => ("single choice", field_id, label, description, required),
        QuestionField::MultiChoice {
            field_id,
            label,
            description,
            required,
            ..
        } => ("multiple choice", field_id, label, description, required),
    };
    body.push_str(&format!(
        "\n- {label} ({kind}, {}, id {field_id})",
        if *required { "required" } else { "optional" }
    ));
    if let Some(description) = description {
        body.push_str(&format!("\n  {description}"));
    }
    if let QuestionField::MultiChoice { min, max, .. } = field
        && (min.is_some() || max.is_some())
    {
        body.push_str("\n  Selection bounds:");
        if let Some(minimum) = min {
            body.push_str(&format!(" at least {minimum}"));
        }
        if let Some(maximum) = max {
            body.push_str(&format!(" at most {maximum}"));
        }
    }
    match field {
        QuestionField::SingleChoice { options, .. }
        | QuestionField::MultiChoice { options, .. } => {
            for option in options {
                body.push_str(&format!(
                    "\n  - {} (id {})",
                    option.label,
                    option.option_id.as_str()
                ));
            }
        }
        QuestionField::Text { .. }
        | QuestionField::Number { .. }
        | QuestionField::Boolean { .. } => {}
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_interaction_presentation_gets_a_new_v7_identity() {
        let first = interaction_origin_ref("approval-1").expect("first origin");
        let second = interaction_origin_ref("approval-1").expect("second origin");
        let (
            RouterOriginRef::Interaction {
                interaction_id: first_interaction,
                presentation_id: first_presentation,
            },
            RouterOriginRef::Interaction {
                interaction_id: second_interaction,
                presentation_id: second_presentation,
            },
        ) = (first, second)
        else {
            panic!("typed interaction origin expected");
        };

        assert_eq!(first_interaction, second_interaction);
        assert_ne!(first_presentation, second_presentation);
        for presentation_id in [first_presentation, second_presentation] {
            let parsed =
                uuid::Uuid::parse_str(presentation_id.as_str()).expect("valid presentation UUID");
            assert_eq!(parsed.get_version_num(), 7);
        }
    }
}

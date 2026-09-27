//! Projection between typed provider approvals and the legacy approval API.

use super::*;

pub(super) fn typed_cancel_approval_state(
    reason: &session_event_model::InteractionCancelReason,
) -> ApprovalState {
    match reason {
        session_event_model::InteractionCancelReason::TimedOut => ApprovalState::TimedOut,
        session_event_model::InteractionCancelReason::ApproverUnreachable => {
            ApprovalState::ApproverUnreachable
        }
        _ => ApprovalState::Cancelled,
    }
}

pub(super) fn project_typed_legacy_approval(
    record: InteractionHistoryRecord,
) -> Option<ApprovalRequestRecord> {
    use session_event_model::{ApprovalEffect, ApprovalScope};
    let InteractionHistoryRecord::Approval {
        requester,
        approver: message_board::Identity::Session { session: approver },
        request,
        state,
        legacy_metadata: Some(metadata),
    } = record
    else {
        return None;
    };
    let requester: SessionRef =
        serde_json::from_value(serde_json::to_value(requester).ok()?).ok()?;
    let approver: SessionRef = serde_json::from_value(serde_json::to_value(approver).ok()?).ok()?;
    let (state, reason, decision) = match state {
        InteractionHistoryState::Pending => (ApprovalState::PendingClientDecision, None, None),
        InteractionHistoryState::Cancelled { reason } => (
            typed_cancel_approval_state(&reason),
            Some(reason.as_str().to_owned()),
            None,
        ),
        InteractionHistoryState::Decided { option_id } => {
            let decision = request
                .options
                .iter()
                .find(|option| option.option_id == option_id)
                .and_then(
                    |option| match (&option.choice.effect, &option.choice.scope) {
                        (ApprovalEffect::Allow, ApprovalScope::Once) => {
                            Some(ApprovalDecision::Allow)
                        }
                        (ApprovalEffect::Allow, ApprovalScope::Session) => {
                            Some(ApprovalDecision::AllowForSession)
                        }
                        (ApprovalEffect::Decline, ApprovalScope::Once) => {
                            Some(ApprovalDecision::Deny)
                        }
                        _ => None,
                    },
                );
            (ApprovalState::Decided, None, decision)
        }
    };
    let offered_options = request
        .options
        .iter()
        .map(|option| {
            let scope = match (&option.choice.effect, &option.choice.scope) {
                (ApprovalEffect::Allow, ApprovalScope::Once) => ApprovalOptionScope::AllowOnce,
                (ApprovalEffect::Allow, ApprovalScope::Session) => {
                    ApprovalOptionScope::AllowForSession
                }
                (ApprovalEffect::Allow, ApprovalScope::Persistent { .. }) => {
                    ApprovalOptionScope::AllowAlways
                }
                (ApprovalEffect::Decline, ApprovalScope::Once) => ApprovalOptionScope::RejectOnce,
                (ApprovalEffect::Decline, ApprovalScope::Persistent { .. }) => {
                    ApprovalOptionScope::RejectAlways
                }
                _ => ApprovalOptionScope::Unsupported {
                    provider_kind: "unmappedTypedChoice".to_owned(),
                },
            };
            ApprovalOfferedOption {
                option_id: option.option_id.as_str().to_owned(),
                label: Some(option.label.clone()),
                scope,
            }
        })
        .collect();
    let presentation = legacy_presentation_from_typed(&request);
    Some(ApprovalRequestRecord {
        request_id: request.request_id,
        requester,
        approver,
        generation: metadata.generation.clone(),
        state,
        reason,
        offered_options,
        presentation: Some(presentation),
        decision,
        operation: serde_json::json!({
            "kind":"externalProviderPermission",
            "operationId":metadata.operation_id,
            "target":metadata.target,
            "bindingGeneration":metadata.generation,
            "method":"session/request_permission",
        }),
        expires_at: metadata.expires_at,
    })
}

pub(super) fn legacy_presentation_from_typed(
    request: &session_event_model::ApprovalRequest,
) -> ApprovalPresentation {
    use session_event_model::ApprovalSubject;
    let mut presentation = ApprovalPresentation {
        title: Some(request.title.clone()),
        ..ApprovalPresentation::default()
    };
    match &request.subject {
        Some(ApprovalSubject::ToolCall { tool_call }) => {
            presentation.kind = Some(tool_call.kind.clone());
        }
        Some(ApprovalSubject::Command { command, cwd, .. }) => {
            presentation.arguments = vec![
                collaboration_protocol::ApprovalArgument {
                    name: "command".to_owned(),
                    value: command.clone(),
                },
                collaboration_protocol::ApprovalArgument {
                    name: "cwd".to_owned(),
                    value: cwd.clone(),
                },
            ];
        }
        Some(ApprovalSubject::Plan { .. }) | None => {}
    }
    presentation
}

pub(super) fn typed_option_view(option: &session_event_model::OfferedOption) -> ApprovalOptionView {
    use session_event_model::{ApprovalEffect, ApprovalScope};
    let effect = match option.choice.effect {
        ApprovalEffect::Allow => ApprovalOptionEffect::Allow,
        ApprovalEffect::Decline => ApprovalOptionEffect::Decline,
        ApprovalEffect::Abort => ApprovalOptionEffect::Abort,
    };
    let (scope, persistent_target) = match &option.choice.scope {
        ApprovalScope::Once => (ApprovalOptionViewScope::Once, None),
        ApprovalScope::Session => (ApprovalOptionViewScope::Session, None),
        ApprovalScope::Persistent { where_stored } => (
            ApprovalOptionViewScope::Persistent,
            Some(where_stored.as_str().to_owned()),
        ),
    };
    ApprovalOptionView {
        option_id: option.option_id.as_str().to_owned(),
        label: option.label.clone(),
        effect,
        scope,
        persistent_target,
    }
}

pub(super) fn legacy_option_view(option: ApprovalOfferedOption) -> Option<ApprovalOptionView> {
    let (effect, scope) = match option.scope {
        ApprovalOptionScope::AllowOnce => {
            (ApprovalOptionEffect::Allow, ApprovalOptionViewScope::Once)
        }
        ApprovalOptionScope::AllowForSession => (
            ApprovalOptionEffect::Allow,
            ApprovalOptionViewScope::Session,
        ),
        ApprovalOptionScope::RejectOnce => {
            (ApprovalOptionEffect::Decline, ApprovalOptionViewScope::Once)
        }
        ApprovalOptionScope::AllowAlways
        | ApprovalOptionScope::RejectAlways
        | ApprovalOptionScope::Unsupported { .. } => return None,
    };
    Some(ApprovalOptionView {
        label: option.label.unwrap_or_else(|| option.option_id.clone()),
        option_id: option.option_id,
        effect,
        scope,
        persistent_target: None,
    })
}

pub(super) fn legacy_choice_from_typed(
    request: &session_event_model::ApprovalRequest,
    decision: ApprovalDecision,
) -> Result<String, ApprovalDecisionError> {
    use session_event_model::{ApprovalEffect, ApprovalScope};

    request
        .options
        .iter()
        .find(|option| match decision {
            ApprovalDecision::Allow => {
                option.choice.effect == ApprovalEffect::Allow
                    && matches!(&option.choice.scope, ApprovalScope::Once)
            }
            ApprovalDecision::AllowForSession => {
                option.choice.effect == ApprovalEffect::Allow
                    && matches!(&option.choice.scope, ApprovalScope::Session)
            }
            ApprovalDecision::Deny => {
                option.choice.effect == ApprovalEffect::Decline
                    && matches!(&option.choice.scope, ApprovalScope::Once)
            }
        })
        .map(|option| option.option_id.as_str().to_owned())
        .ok_or_else(|| ApprovalDecisionError::OptionNotOffered {
            offered: request
                .options
                .iter()
                .map(|option| option.option_id.as_str().to_owned())
                .collect(),
        })
}

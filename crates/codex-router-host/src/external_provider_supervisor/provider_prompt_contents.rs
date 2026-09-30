//! Provider-neutral prompt admission and Control text projection.

use super::*;

/// Host-internal prompt admission. Public Control requests render their text
/// into one block before reaching this boundary.
#[derive(Clone, Debug)]
pub struct ProviderPromptContentsRequest {
    pub operation_id: OperationId,
    pub input_id: session_event_model::InputId,
    pub target: SessionRef,
    pub requested_by: ProviderIdentity,
    pub approver: ProviderIdentity,
    pub contents: Vec<session_event_model::PromptContent>,
}

impl ProviderPromptContentsRequest {
    pub(super) fn from_control(
        request: ConversationPromptRequest,
        display_names: &collaboration_service::SessionDisplayNameCache,
    ) -> Result<Self, Box<ConversationOperationFailure>> {
        let header_context = collaboration_protocol::MessageHeaderContext::resolve(
            &request.target,
            &request.prompt,
            display_names,
            collaboration_protocol::MessageHeaderOrigin::Agent,
        );
        let rendered = collaboration_protocol::render_message_with_context(
            &request.target,
            &request.prompt,
            &header_context,
        )
        .map_err(|_| {
            Box::new(failure(
                ConversationOperationFailureKind::InvalidRequest,
                ConversationOperationFailureStage::Validation,
                ProviderOperationEffect::None,
                "provider prompt could not be rendered within the Control frame bound",
                request.operation_id.clone(),
                Some(request.target.clone()),
            ))
        })?;
        let content = session_event_model::PromptContent::text(rendered.text).map_err(|_| {
            Box::new(failure(
                ConversationOperationFailureKind::InvalidRequest,
                ConversationOperationFailureStage::Validation,
                ProviderOperationEffect::None,
                "provider prompt text is invalid",
                request.operation_id.clone(),
                Some(request.target.clone()),
            ))
        })?;
        Ok(Self {
            operation_id: request.operation_id,
            input_id: request
                .input_id
                .unwrap_or_else(session_event_model::InputId::generate),
            target: request.target,
            requested_by: request.requested_by,
            approver: request.approver,
            contents: vec![content],
        })
    }
}

impl ExternalProviderSupervisor {
    pub fn prompt_contents(
        &self,
        request: ProviderPromptContentsRequest,
    ) -> ProviderConversationFuture<'static, ConversationOperationSubmission> {
        let operation_id = request.operation_id.clone();
        let failure_target = Some(request.target.clone());
        let backend = self.clone();
        retain_operation(operation_id, failure_target, async move {
            let target = request.target.clone();
            let (binding, runtime) = backend.runtime_binding(
                &target.endpoint,
                &request.operation_id,
                Some(target.clone()),
            )?;
            let approval_context = crate::ExternalProviderApprovalContext {
                requester: request.requested_by.clone(),
                approver: request.approver.clone(),
                target: target.clone(),
                operation_id: request.operation_id.clone(),
                binding_generation: binding.generation.clone(),
                binding_retirement: runtime.retirement(),
            };
            let prepared = backend
                .prepare_operation(
                    request.operation_id.clone(),
                    ProviderOperationKind::ConversationPrompt,
                    binding,
                    Some(&target),
                )
                .await?;
            match prepared {
                PreparedOperation::Existing(operation) => Ok(existing_submission(operation)),
                PreparedOperation::Admitted { snapshot, live } => {
                    let operation_id = request.operation_id.clone();
                    let provider_session_id = String::from(target.session_id.clone());
                    let input_id = request.input_id;
                    let completion_target = target.clone();
                    backend.spawn_operation(operation_id.clone(), live, async move {
                        match runtime
                            .prompt_contents_with_approval_dispatch_for_input(
                                provider_session_id,
                                input_id,
                                request.contents,
                                approval_context,
                                None,
                            )
                            .await
                        {
                            Ok(crate::ExternalProviderPromptOutcome {
                                permission_refusal_reason: Some(reason),
                                ..
                            }) => ProviderOperationCompletion::Failure(failure(
                                ConversationOperationFailureKind::PermissionRejected,
                                ConversationOperationFailureStage::Settlement,
                                ProviderOperationEffect::Applied,
                                reason.code(),
                                operation_id,
                                Some(completion_target),
                            )),
                            Ok(outcome) => match optional_message_text(outcome.output) {
                                Ok(response) => ProviderOperationCompletion::Success {
                                    settlement: ConversationOperationSettlement::PromptCompleted {
                                        target: completion_target.clone(),
                                        stop_reason: outcome.stop_reason,
                                        response,
                                    },
                                    target: Some(completion_target),
                                    session_record: None,
                                },
                                Err(_) => ProviderOperationCompletion::Failure(failure(
                                    ConversationOperationFailureKind::ProtocolViolation,
                                    ConversationOperationFailureStage::Settlement,
                                    ProviderOperationEffect::Applied,
                                    "provider returned invalid prompt output",
                                    operation_id,
                                    Some(completion_target),
                                )),
                            },
                            Err(error) => {
                                ProviderOperationCompletion::Failure(prompt_runtime_failure(
                                    operation_id,
                                    Some(completion_target),
                                    error,
                                ))
                            }
                        }
                    });
                    Ok(admitted_submission(snapshot))
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::ProviderPromptContentsRequest;
    use collaboration_protocol::{
        ConversationPromptRequest, MessageContent, MessageText, OperationId, SessionRef,
    };
    use collaboration_service::SessionDisplayNameCache;
    use session_event_model::PromptContent;

    #[test]
    fn control_prompt_uses_router_known_session_display_names() {
        let target: SessionRef = serde_json::from_value(serde_json::json!({
            "endpoint":{"serviceId":"018f47d2-24d5-7a68-b9ec-6f759c39458f","endpointId":"codex-local"},
            "sessionId":"target-session"
        }))
        .expect("target session");
        let sender: SessionRef = serde_json::from_value(serde_json::json!({
            "endpoint":{"serviceId":"018f47d2-24d5-7a68-b9ec-6f759c39458f","endpointId":"claude-local"},
            "sessionId":"sender-session"
        }))
        .expect("sender session");
        let display_names = SessionDisplayNameCache::default();
        display_names.remember(target.clone(), "🤖 Codex Main");
        display_names.remember(sender.clone(), "🐒 Sidekick · provider prompt");
        let request = ConversationPromptRequest {
            operation_id: OperationId::generate(),
            input_id: None,
            target,
            generation: None,
            requested_by: sender.clone().into(),
            approver: sender.clone().into(),
            prompt: MessageContent::Agent {
                sender,
                text: MessageText::try_from("Check the provider prompt.".to_owned())
                    .expect("prompt text"),
            },
        };

        let rendered = ProviderPromptContentsRequest::from_control(request, &display_names)
            .expect("provider prompt contents");
        let Some(PromptContent::Text { text }) = rendered.contents.first() else {
            panic!("one text prompt should be generated");
        };
        assert!(
            text.as_str().starts_with(
                "🤖 Codex Main ← 🐒 Sidekick · provider prompt\nAgent communication\n"
            )
        );
    }
}

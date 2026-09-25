//! Load, prompt, cancel, and settlement handling for the selected conversation client.
use crate::conversation_client::{
    ConversationCancelInput, ConversationClient, ConversationClientError, ConversationLoadInput,
    ConversationPromptInput, unsupported,
};
use crate::{
    ClientError, ConversationEnd, ConversationOperationResult,
    ConversationPromptRequest as CodexPromptRequest, ConversationSettlement,
    ConversationSettlementDetail, ConversationStopReason, ExistingConversationPromptRequest,
    ProviderLoadOutput, ProviderPromptOutput,
};
use collaboration_protocol::{
    ConversationCancelRequest, ConversationLoadRequest, ConversationOperationFailure,
    ConversationOperationFailureKind, ConversationOperationFailureStage,
    ConversationOperationSettlement, ConversationOperationSubmission,
    ConversationOperationWaitOutput, ConversationOperationWaitRequest,
    ConversationPromptRequest as ProviderPromptRequest, NonEmptyText, OperationId, PositiveSeconds,
    ProviderOperationEffect, ProviderOperationStage, ProviderPromptStopReason,
    ProviderRequestedPolicy, ProviderWorkingDirectory, SessionRef,
};
use std::{path::Path, time::Duration};
use tokio_util::sync::CancellationToken;

impl ConversationClient {
    pub async fn load(
        mut self,
        input: ConversationLoadInput,
        timeout: Duration,
    ) -> Result<ConversationOperationResult, ConversationClientError> {
        validate_target_operation(&input.target, &input.requested_by, input.approver.as_ref())?;
        validate_operation_timeout(timeout)?;
        if !input.working_directory.is_absolute() {
            return Err(ConversationClientError::InvalidInput(
                "load working directory must be absolute",
            ));
        }
        let target = input.target.clone();
        let operation_id = input.operation_id.clone();
        self.validate_operation_id(&target.endpoint, operation_id.as_ref(), "load")?;
        match &mut self {
            Self::CodexAcp(acp) => {
                if input.generation.is_some() {
                    return Err(unsupported(
                        &target.endpoint,
                        "generation",
                        "omit the generation guard for this Codex ACP load",
                    ));
                }
                let request = crate::ConversationCreateRequest {
                    operation_id: OperationId::generate(),
                    endpoint: target.endpoint.clone(),
                    cwd: input.working_directory,
                    session: Some(target.session_id.clone()),
                    fork: None,
                    model: None,
                    effort: None,
                    access: None,
                    created_by: None,
                    approver: None,
                    root_message_id: None,
                };
                let mut emit = |_event| Ok(());
                let loaded = acp.open_session_with_context(&request, &mut emit).await?;
                Ok(ConversationOperationResult::Completed {
                    target: loaded.clone(),
                    operation_id: None,
                    settlement: ConversationSettlement {
                        target: loaded,
                        stop_reason: None,
                        detail: ConversationSettlementDetail::CodexLoad,
                    },
                })
            }
            Self::ExternalProvider(control) => {
                let operation_id =
                    operation_id.ok_or_else(|| ConversationClientError::MissingOperationId {
                        endpoint: target.endpoint.clone(),
                        operation: "load",
                    })?;
                let working_directory = provider_working_directory(&input.working_directory)?;
                let request = ConversationLoadRequest {
                    operation_id: operation_id.clone(),
                    target: target.clone(),
                    generation: input.generation,
                    working_directory,
                    requested_by: input.requested_by.clone(),
                    approver: input.approver.unwrap_or(input.requested_by),
                    requested_policy: ProviderRequestedPolicy {
                        access: input.access,
                    },
                };
                let wait_seconds = provider_wait_seconds(timeout)?;
                let submitted = tokio::time::timeout(timeout, async {
                    control.load_provider_conversation(request).await?;
                    let waited = control
                        .wait_for_provider_conversation_operation(
                            ConversationOperationWaitRequest {
                                operation_id: operation_id.clone(),
                                timeout_seconds: wait_seconds,
                            },
                        )
                        .await?;
                    provider_operation_result(
                        waited,
                        operation_id.clone(),
                        target.clone(),
                        ProviderSettlementKind::Load,
                    )
                })
                .await;
                match submitted {
                    Ok(Err(ConversationClientError::Client(ClientError::Timeout))) | Err(_) => {
                        Ok(ConversationOperationResult::Pending {
                            operation_id,
                            target,
                        })
                    }
                    Ok(result) => result,
                }
            }
        }
    }

    pub async fn prompt(
        mut self,
        input: ConversationPromptInput,
        timeout: Duration,
        cancel: CancellationToken,
    ) -> Result<ConversationOperationResult, ConversationClientError> {
        validate_target_operation(&input.target, &input.requested_by, input.approver.as_ref())?;
        validate_operation_timeout(timeout)?;
        let target = input.target.clone();
        let operation_id = input.operation_id.clone();
        self.validate_operation_id(&target.endpoint, operation_id.as_ref(), "prompt")?;
        match &mut self {
            Self::CodexAcp(acp) => {
                if input.generation.is_some() {
                    return Err(unsupported(
                        &target.endpoint,
                        "generation",
                        "omit the generation guard for this Codex ACP prompt",
                    ));
                }
                let cwd = input
                    .working_directory
                    .ok_or(ConversationClientError::InvalidInput(
                        "Codex prompt requires a working directory",
                    ))?;
                if !cwd.is_absolute() {
                    return Err(ConversationClientError::InvalidInput(
                        "prompt working directory must be absolute",
                    ));
                }
                let prompted = acp
                    .prompt_existing_on_connection(
                        ExistingConversationPromptRequest {
                            target: target.clone(),
                            cwd,
                            prompt: CodexPromptRequest {
                                message: input.message,
                                effort: input.effort,
                                timeout_seconds: timeout.as_secs(),
                            },
                        },
                        cancel,
                    )
                    .await?;
                let stop_reason = match prompted.end {
                    ConversationEnd::Completed => ConversationStopReason::Completed,
                    ConversationEnd::TimedOut => ConversationStopReason::TimedOut,
                    ConversationEnd::Cancelled => ConversationStopReason::Cancelled,
                };
                Ok(ConversationOperationResult::Completed {
                    target: prompted.target.clone(),
                    operation_id: None,
                    settlement: ConversationSettlement {
                        target: prompted.target,
                        stop_reason: Some(stop_reason),
                        detail: ConversationSettlementDetail::CodexPrompt {
                            updates: prompted.updates,
                            permission_required: prompted.permission_required,
                            result: prompted.result,
                        },
                    },
                })
            }
            Self::ExternalProvider(control) => {
                let operation_id =
                    operation_id.ok_or_else(|| ConversationClientError::MissingOperationId {
                        endpoint: target.endpoint.clone(),
                        operation: "prompt",
                    })?;
                for (field, present) in [
                    ("workingDirectory", input.working_directory.is_some()),
                    ("effort", input.effort.is_some()),
                ] {
                    if present {
                        return Err(unsupported(
                            &target.endpoint,
                            field,
                            &format!("omit {field} for this provider endpoint"),
                        ));
                    }
                }
                let request = ProviderPromptRequest {
                    operation_id: operation_id.clone(),
                    target: target.clone(),
                    generation: input.generation,
                    requested_by: input.requested_by.clone(),
                    approver: input.approver.unwrap_or(input.requested_by),
                    prompt: input.message.into(),
                };
                let wait_seconds = provider_wait_seconds(timeout)?;
                let submitted = tokio::time::timeout(timeout, async {
                    control.prompt_provider_conversation(request).await?;
                    let waited = control
                        .wait_for_provider_conversation_operation(
                            ConversationOperationWaitRequest {
                                operation_id: operation_id.clone(),
                                timeout_seconds: wait_seconds,
                            },
                        )
                        .await?;
                    provider_operation_result(
                        waited,
                        operation_id.clone(),
                        target.clone(),
                        ProviderSettlementKind::Prompt,
                    )
                })
                .await;
                match submitted {
                    Ok(Err(ConversationClientError::Client(ClientError::Timeout))) | Err(_) => {
                        Ok(ConversationOperationResult::Pending {
                            operation_id,
                            target,
                        })
                    }
                    Ok(result) => result,
                }
            }
        }
    }

    pub async fn cancel(
        mut self,
        input: ConversationCancelInput,
    ) -> Result<ConversationOperationSubmission, ConversationClientError> {
        validate_target_operation(&input.target, &input.requested_by, input.approver.as_ref())?;
        match &mut self {
            Self::CodexAcp(_) => Err(unsupported(
                &input.target.endpoint,
                "cancel",
                "use `turn interrupt` for a Codex session",
            )),
            Self::ExternalProvider(control) => Ok(control
                .cancel_provider_conversation_operation(ConversationCancelRequest {
                    operation_id: input.operation_id,
                    target_operation_id: input.target_operation_id,
                    target: input.target,
                    generation: input.generation,
                    requested_by: input.requested_by.clone(),
                    approver: input.approver.unwrap_or(input.requested_by),
                })
                .await?),
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum ProviderSettlementKind {
    Load,
    Prompt,
}

pub(crate) fn provider_operation_result(
    waited: collaboration_protocol::ConversationOperationWaitResult,
    operation_id: OperationId,
    target: SessionRef,
    expected: ProviderSettlementKind,
) -> Result<ConversationOperationResult, ConversationClientError> {
    let settlement = match (expected, waited.output) {
        (
            ProviderSettlementKind::Load,
            ConversationOperationWaitOutput::Available {
                settlement:
                    ConversationOperationSettlement::Loaded {
                        target: settled_target,
                        effective_settings,
                    },
            },
        ) if settled_target == target => Some(ConversationSettlement {
            target: settled_target,
            stop_reason: None,
            detail: ConversationSettlementDetail::ProviderLoad {
                output: ProviderLoadOutput::Available { effective_settings },
            },
        }),
        (
            ProviderSettlementKind::Prompt,
            ConversationOperationWaitOutput::Available {
                settlement:
                    ConversationOperationSettlement::PromptCompleted {
                        target: settled_target,
                        stop_reason,
                        response,
                    },
            },
        ) if settled_target == target => Some(ConversationSettlement {
            target: settled_target,
            stop_reason: Some(provider_stop_reason(stop_reason)),
            detail: ConversationSettlementDetail::ProviderPrompt {
                output: ProviderPromptOutput::Available { text: response },
            },
        }),
        (
            ProviderSettlementKind::Load,
            ConversationOperationWaitOutput::OutputUnavailable { reason },
        ) if waited.operation.stage == ProviderOperationStage::Terminal
            && waited.operation.effect == ProviderOperationEffect::Applied
            && waited.operation.target.as_ref() == Some(&target) =>
        {
            Some(ConversationSettlement {
                target: target.clone(),
                stop_reason: None,
                detail: ConversationSettlementDetail::ProviderLoad {
                    output: ProviderLoadOutput::Unavailable { reason },
                },
            })
        }
        (
            ProviderSettlementKind::Prompt,
            ConversationOperationWaitOutput::OutputUnavailable { reason },
        ) if waited.operation.stage == ProviderOperationStage::Terminal
            && waited.operation.effect == ProviderOperationEffect::Applied
            && waited.operation.target.as_ref() == Some(&target) =>
        {
            Some(ConversationSettlement {
                target: target.clone(),
                stop_reason: None,
                detail: ConversationSettlementDetail::ProviderPrompt {
                    output: ProviderPromptOutput::Unavailable { reason },
                },
            })
        }
        (_, ConversationOperationWaitOutput::Pending) => None,
        _ => {
            return Err(operation_settlement_failure(
                operation_id,
                target,
                waited.operation.effect,
            ));
        }
    };
    if let Some(settlement) = settlement {
        return Ok(ConversationOperationResult::Completed {
            target,
            operation_id: Some(operation_id),
            settlement,
        });
    }
    if waited.operation.stage == ProviderOperationStage::Terminal {
        return Err(operation_settlement_failure(
            operation_id,
            target,
            waited.operation.effect,
        ));
    }
    Ok(ConversationOperationResult::Pending {
        operation_id,
        target,
    })
}

fn provider_stop_reason(reason: ProviderPromptStopReason) -> ConversationStopReason {
    match reason {
        ProviderPromptStopReason::EndTurn => ConversationStopReason::EndTurn,
        ProviderPromptStopReason::MaxTokens => ConversationStopReason::MaxTokens,
        ProviderPromptStopReason::MaxTurnRequests => ConversationStopReason::MaxTurnRequests,
        ProviderPromptStopReason::Refusal => ConversationStopReason::Refusal,
        ProviderPromptStopReason::Cancelled => ConversationStopReason::Cancelled,
    }
}

fn operation_settlement_failure(
    operation_id: OperationId,
    target: SessionRef,
    effect: ProviderOperationEffect,
) -> ConversationClientError {
    let message = match NonEmptyText::try_from(
        "conversation operation ended without its expected settlement; inspect the operation ID"
            .to_owned(),
    ) {
        Ok(message) => message,
        Err(_) => {
            return ConversationClientError::InvalidInput(
                "invalid static settlement failure message",
            );
        }
    };
    ConversationClientError::OperationFailure(Box::new(ConversationOperationFailure {
        kind: if effect == ProviderOperationEffect::Unknown {
            ConversationOperationFailureKind::OutcomeUnknown
        } else {
            ConversationOperationFailureKind::ProviderRejected
        },
        stage: ConversationOperationFailureStage::Settlement,
        effect,
        message,
        operation_id,
        target: Some(target),
        endpoint: None,
        availability: None,
    }))
}

fn validate_target_operation(
    target: &SessionRef,
    requested_by: &SessionRef,
    approver: Option<&SessionRef>,
) -> Result<(), ConversationClientError> {
    if requested_by.endpoint.service_id != target.endpoint.service_id
        || approver
            .is_some_and(|approver| approver.endpoint.service_id != target.endpoint.service_id)
    {
        return Err(ConversationClientError::InvalidInput(
            "requester and approver must belong to the target service",
        ));
    }
    Ok(())
}

fn validate_operation_timeout(timeout: Duration) -> Result<(), ConversationClientError> {
    if timeout.is_zero() {
        return Err(ConversationClientError::InvalidInput(
            "conversation timeout must be positive",
        ));
    }
    Ok(())
}

fn provider_working_directory(
    path: &Path,
) -> Result<ProviderWorkingDirectory, ConversationClientError> {
    ProviderWorkingDirectory::try_from(
        path.to_str()
            .ok_or(ConversationClientError::InvalidInput(
                "working directory must be UTF-8",
            ))?
            .to_owned(),
    )
    .map_err(|_| ConversationClientError::InvalidInput("invalid working directory"))
}

fn provider_wait_seconds(timeout: Duration) -> Result<PositiveSeconds, ConversationClientError> {
    u32::try_from(timeout.as_secs())
        .ok()
        .and_then(|seconds| PositiveSeconds::try_from(seconds).ok())
        .ok_or(ConversationClientError::InvalidInput(
            "timeout must be whole seconds within the supported range",
        ))
}

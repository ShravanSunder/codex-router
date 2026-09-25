//! One conversation connection selected from the endpoint's advertised channel.
use crate::{
    AcpConversation, ClientError, ControlClient, ConversationCreatePromptOutcome,
    PublicPromptContent,
};
use collaboration_protocol::{
    ChannelDescription, CodexGeneration, ConversationCreateOutcome, ConversationOperationFailure,
    ConversationOperationFailureKind, ConversationOperationFailureStage,
    ConversationOperationSettlement, ConversationOperationWaitOutput,
    ConversationOperationWaitRequest, EndpointDescription, EndpointRef, NonEmptyText, OperationId,
    PositiveSeconds, ProviderOperationEffect, ProviderOperationStage, ProviderRequestedPolicy,
    ProviderWorkingDirectory, RouterAccess, SessionId, SessionRef, UuidIdentity,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationCreateInput {
    pub operation_id: OperationId,
    pub endpoint: EndpointRef,
    pub working_directory: PathBuf,
    pub access: RouterAccess,
    pub created_by: SessionRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approver: Option<SessionRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<CodexGeneration>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fork: Option<SessionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_message_id: Option<UuidIdentity>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationLoadInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<OperationId>,
    pub target: SessionRef,
    pub working_directory: PathBuf,
    pub requested_by: SessionRef,
    #[serde(default)]
    pub approver: Option<SessionRef>,
    pub access: RouterAccess,
    #[serde(default)]
    pub generation: Option<CodexGeneration>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationPromptInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<OperationId>,
    pub target: SessionRef,
    #[serde(default)]
    pub working_directory: Option<PathBuf>,
    pub requested_by: SessionRef,
    #[serde(default)]
    pub approver: Option<SessionRef>,
    pub message: PublicPromptContent,
    #[serde(default)]
    pub effort: Option<String>,
    #[serde(default)]
    pub generation: Option<CodexGeneration>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationCancelInput {
    pub operation_id: OperationId,
    pub target_operation_id: OperationId,
    pub target: SessionRef,
    pub requested_by: SessionRef,
    #[serde(default)]
    pub approver: Option<SessionRef>,
    #[serde(default)]
    pub generation: Option<CodexGeneration>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationCreatePromptInput {
    pub create: ConversationCreateInput,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_operation_id: Option<OperationId>,
    pub message: PublicPromptContent,
    #[serde(default)]
    pub prompt_effort: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ConversationClientError {
    #[error(transparent)]
    Client(#[from] ClientError),
    #[error(transparent)]
    Codex(Box<crate::OperationError>),
    #[error("{field} is unsupported by {endpoint:?}; {fix}")]
    UnsupportedInput {
        endpoint: EndpointRef,
        field: &'static str,
        fix: String,
    },
    #[error("invalid conversation create input: {0}")]
    InvalidInput(&'static str),
    #[error("{operation} on {endpoint:?} requires a caller UUIDv7 operation ID")]
    MissingOperationId {
        endpoint: EndpointRef,
        operation: &'static str,
    },
    #[error("conversation create operation failed: {0:?}")]
    OperationFailure(Box<ConversationOperationFailure>),
    #[error("caller detached from conversation operation {operation_id:?}")]
    CallerCancelled { operation_id: OperationId },
    #[error("conversation was created, but its prompt did not complete: {source}")]
    AfterCreate {
        create_operation_id: OperationId,
        target: SessionRef,
        #[source]
        source: Box<ConversationClientError>,
    },
}

impl From<crate::OperationError> for ConversationClientError {
    fn from(error: crate::OperationError) -> Self {
        Self::Codex(Box::new(error))
    }
}

pub enum ConversationClient {
    CodexAcp(AcpConversation),
    ExternalProvider(ControlClient),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ConversationTransport {
    CodexAcp,
    ExternalProvider,
}

impl ConversationClient {
    pub fn validate_operation_id(
        &self,
        endpoint: &EndpointRef,
        operation_id: Option<&OperationId>,
        operation: &'static str,
    ) -> Result<(), ConversationClientError> {
        match self {
            Self::CodexAcp(_) if operation_id.is_some() => {
                Err(ConversationClientError::UnsupportedInput {
                    endpoint: endpoint.clone(),
                    field: "operationId",
                    fix: "omit the operation ID for Codex prompts; it is not inspectable"
                        .to_owned(),
                })
            }
            Self::ExternalProvider(_) if operation_id.is_none() => {
                Err(ConversationClientError::MissingOperationId {
                    endpoint: endpoint.clone(),
                    operation,
                })
            }
            _ => Ok(()),
        }
    }

    pub async fn create_and_prompt(
        directory: &Path,
        input: ConversationCreatePromptInput,
        timeout: Duration,
        cancel: CancellationToken,
    ) -> Result<ConversationCreatePromptOutcome, ConversationClientError> {
        if input.create.fork.is_some() {
            return Err(ConversationClientError::InvalidInput(
                "create-and-prompt requires a fresh destination",
            ));
        }
        Self::validate_create_input(&input.create, timeout)?;
        let create_operation_id = input.create.operation_id.clone();
        let endpoint = input.create.endpoint.clone();
        let requested_by = input.create.created_by.clone();
        let approver = input.create.approver.clone();
        let working_directory = input.create.working_directory.clone();
        let create_client = Self::connect(directory, &endpoint).await?;
        create_client.validate_operation_id(
            &endpoint,
            input.prompt_operation_id.as_ref(),
            "prompt",
        )?;
        let created = tokio::select! {
            _ = cancel.cancelled() => return Err(ConversationClientError::CallerCancelled { operation_id: create_operation_id }),
            result = create_client.create(input.create, timeout) => result?,
        };
        let target = match created {
            ConversationCreateOutcome::Created { target, .. } => target,
            ConversationCreateOutcome::Pending { operation_id } => {
                return Ok(ConversationCreatePromptOutcome::CreatePending { operation_id });
            }
        };
        let client = Self::connect(directory, &endpoint)
            .await
            .map_err(|source| ConversationClientError::AfterCreate {
                create_operation_id: create_operation_id.clone(),
                target: target.clone(),
                source: Box::new(source),
            })?;
        let native = matches!(client, Self::CodexAcp(_));
        let prompt_input = ConversationPromptInput {
            operation_id: input.prompt_operation_id,
            target: target.clone(),
            working_directory: native.then_some(working_directory),
            requested_by,
            approver,
            message: input.message,
            effort: input.prompt_effort,
            generation: None,
        };
        let prompt_operation_id = prompt_input.operation_id.clone();
        let prompt = if native {
            client.prompt(prompt_input, timeout, cancel).await
        } else {
            let detach = cancel.clone();
            tokio::select! {
                _ = detach.cancelled() => match prompt_operation_id {
                    Some(operation_id) => Err(ConversationClientError::CallerCancelled { operation_id }),
                    None => Err(ConversationClientError::MissingOperationId { endpoint: endpoint.clone(), operation: "prompt" }),
                },
                result = client.prompt(prompt_input, timeout, cancel) => result,
            }
        }.map_err(|source| ConversationClientError::AfterCreate {
            create_operation_id: create_operation_id.clone(),
            target: target.clone(),
            source: Box::new(source),
        })?;
        Ok(ConversationCreatePromptOutcome::Prompt {
            create_operation_id,
            prompt: Box::new(prompt),
        })
    }

    pub fn validate_create_input(
        input: &ConversationCreateInput,
        timeout: Duration,
    ) -> Result<(), ConversationClientError> {
        if timeout.is_zero() || !input.working_directory.is_absolute() {
            return Err(ConversationClientError::InvalidInput(
                "create requires a positive timeout and absolute working directory",
            ));
        }
        if input.created_by.endpoint.service_id != input.endpoint.service_id
            || input
                .approver
                .as_ref()
                .is_some_and(|approver| approver.endpoint.service_id != input.endpoint.service_id)
        {
            return Err(ConversationClientError::InvalidInput(
                "creator and approver must belong to the selected service",
            ));
        }
        Ok(())
    }

    pub async fn connect(
        directory: &Path,
        target: &EndpointRef,
    ) -> Result<Self, ConversationClientError> {
        let mut control =
            ControlClient::connect(directory, "conversation-client", env!("CARGO_PKG_VERSION"))
                .await?;
        if target.service_id != control.identity().service_id {
            return Err(
                ClientError::Protocol("conversation endpoint belongs to another service").into(),
            );
        }
        let endpoint = control
            .list_endpoints()
            .await?
            .endpoints
            .into_iter()
            .find(|endpoint| endpoint.endpoint == *target)
            .ok_or(ClientError::Protocol("conversation endpoint not found"))?;
        match advertised_conversation_transport(&endpoint)? {
            ConversationTransport::CodexAcp => {
                control.close().await?;
                let acp =
                    AcpConversation::connect_with_context(directory, target.endpoint_id.clone())
                        .await
                        .map_err(ConversationClientError::from)?;
                if acp.endpoint() != target {
                    return Err(ClientError::Protocol(
                        "conversation endpoint changed during connect",
                    )
                    .into());
                }
                Ok(Self::CodexAcp(acp))
            }
            ConversationTransport::ExternalProvider => Ok(Self::ExternalProvider(control)),
        }
    }

    #[must_use]
    pub fn transport(&self) -> &'static str {
        match self {
            Self::CodexAcp(_) => "codexAcp",
            Self::ExternalProvider(_) => "externalProvider",
        }
    }

    pub async fn create(
        mut self,
        input: ConversationCreateInput,
        timeout: Duration,
    ) -> Result<ConversationCreateOutcome, ConversationClientError> {
        Self::validate_create_input(&input, timeout)?;
        let operation_id = input.operation_id.clone();
        match &mut self {
            Self::CodexAcp(acp) => {
                if acp.endpoint() != &input.endpoint {
                    return Err(ConversationClientError::InvalidInput(
                        "Codex ACP endpoint changed",
                    ));
                }
                if input.generation.is_some() {
                    return Err(unsupported(
                        &input.endpoint,
                        "generation",
                        "omit the generation guard for this Codex ACP create",
                    ));
                }
                let request = crate::ConversationCreateRequest {
                    operation_id: operation_id.clone(),
                    endpoint: input.endpoint,
                    cwd: input.working_directory,
                    session: None,
                    fork: input.fork,
                    model: input.model,
                    effort: input.effort,
                    access: Some(match input.access {
                        RouterAccess::WriteRestricted => "write-restricted".to_owned(),
                        RouterAccess::WorkspaceWrite => "workspace-write".to_owned(),
                    }),
                    created_by: Some(input.created_by),
                    approver: input.approver,
                    root_message_id: input.root_message_id,
                };
                let mut emit = |_event| Ok(());
                match tokio::time::timeout(
                    timeout,
                    acp.open_session_with_context(&request, &mut emit),
                )
                .await
                {
                    Ok(Ok(target)) => Ok(ConversationCreateOutcome::Created {
                        operation_id,
                        target,
                    }),
                    Ok(Err(error)) => Err(error.into()),
                    Err(_) => Ok(ConversationCreateOutcome::Pending { operation_id }),
                }
            }
            Self::ExternalProvider(control) => {
                if control.identity().service_id != input.endpoint.service_id {
                    return Err(ConversationClientError::InvalidInput(
                        "provider endpoint belongs to another service",
                    ));
                }
                for (field, present) in [
                    ("model", input.model.is_some()),
                    ("effort", input.effort.is_some()),
                    ("fork", input.fork.is_some()),
                    ("rootMessageId", input.root_message_id.is_some()),
                ] {
                    if present {
                        return Err(unsupported(
                            &input.endpoint,
                            field,
                            &format!("omit {field} for this provider endpoint"),
                        ));
                    }
                }
                let working_directory = ProviderWorkingDirectory::try_from(
                    input
                        .working_directory
                        .to_str()
                        .ok_or(ConversationClientError::InvalidInput(
                            "working directory must be UTF-8",
                        ))?
                        .to_owned(),
                )
                .map_err(|_| ConversationClientError::InvalidInput("invalid working directory"))?;
                let request = collaboration_protocol::ConversationCreateRequest {
                    operation_id: operation_id.clone(),
                    endpoint: input.endpoint,
                    generation: input.generation,
                    working_directory,
                    created_by: input.created_by.clone(),
                    approver: input.approver.unwrap_or(input.created_by),
                    requested_policy: ProviderRequestedPolicy {
                        access: input.access,
                    },
                };
                let wait_seconds = u32::try_from(timeout.as_secs())
                    .ok()
                    .and_then(|seconds| PositiveSeconds::try_from(seconds).ok())
                    .ok_or(ConversationClientError::InvalidInput(
                        "create timeout must be whole seconds within the supported range",
                    ))?;
                let submitted = tokio::time::timeout(timeout, async {
                    let admitted = control.create_provider_conversation(request).await?;
                    if let Some(target) = created_target(&admitted.operation) {
                        return Ok(ConversationCreateOutcome::Created {
                            operation_id: operation_id.clone(),
                            target,
                        });
                    }
                    let settled = control
                        .wait_for_provider_conversation_operation(
                            ConversationOperationWaitRequest {
                                operation_id: operation_id.clone(),
                                timeout_seconds: wait_seconds,
                            },
                        )
                        .await?;
                    if let ConversationOperationWaitOutput::Available {
                        settlement: ConversationOperationSettlement::Created { target, .. },
                    } = settled.output
                    {
                        return Ok(ConversationCreateOutcome::Created {
                            operation_id: operation_id.clone(),
                            target,
                        });
                    }
                    if let Some(target) = created_target(&settled.operation) {
                        return Ok(ConversationCreateOutcome::Created {
                            operation_id: operation_id.clone(),
                            target,
                        });
                    }
                    if settled.operation.stage == ProviderOperationStage::Terminal {
                        let effect = settled.operation.effect;
                        let kind = if effect == ProviderOperationEffect::Unknown {
                            ConversationOperationFailureKind::OutcomeUnknown
                        } else {
                            ConversationOperationFailureKind::ProviderRejected
                        };
                        let message = NonEmptyText::try_from(
                            "conversation create ended without a target; inspect the operation ID"
                                .to_owned(),
                        )
                        .map_err(|_| {
                            ConversationClientError::InvalidInput("invalid create failure message")
                        })?;
                        return Err(ConversationClientError::OperationFailure(Box::new(
                            ConversationOperationFailure {
                                kind,
                                stage: ConversationOperationFailureStage::Settlement,
                                effect,
                                message,
                                operation_id: operation_id.clone(),
                                target: None,
                            },
                        )));
                    }
                    Ok::<_, ConversationClientError>(ConversationCreateOutcome::Pending {
                        operation_id: operation_id.clone(),
                    })
                })
                .await;
                match submitted {
                    Ok(Ok(outcome)) => Ok(outcome),
                    Ok(Err(ConversationClientError::Client(ClientError::Timeout))) | Err(_) => {
                        Ok(ConversationCreateOutcome::Pending { operation_id })
                    }
                    Ok(Err(error)) => Err(error),
                }
            }
        }
    }
}

fn created_target(
    snapshot: &collaboration_protocol::ConversationOperationSnapshot,
) -> Option<SessionRef> {
    if snapshot.stage == ProviderOperationStage::Terminal
        && snapshot.effect == ProviderOperationEffect::Applied
    {
        snapshot.target.clone()
    } else {
        None
    }
}

pub(crate) fn unsupported(
    endpoint: &EndpointRef,
    field: &'static str,
    fix: &str,
) -> ConversationClientError {
    ConversationClientError::UnsupportedInput {
        endpoint: endpoint.clone(),
        field,
        fix: format!("{fix} ({})", String::from(endpoint.endpoint_id.clone())),
    }
}

fn advertised_conversation_transport(
    endpoint: &EndpointDescription,
) -> Result<ConversationTransport, ClientError> {
    let mut selected = None;
    for channel in &endpoint.channels {
        let candidate = match channel {
            ChannelDescription::Acp { .. } => Some(ConversationTransport::CodexAcp),
            ChannelDescription::ExternalProvider { .. } => {
                Some(ConversationTransport::ExternalProvider)
            }
            ChannelDescription::NativeCodex { .. } => None,
        };
        if let Some(candidate) = candidate
            && selected.replace(candidate).is_some()
        {
            return Err(ClientError::Protocol("ambiguous conversation transport"));
        }
    }
    selected.ok_or(ClientError::UnsupportedCapability("conversation transport"))
}

#[cfg(test)]
#[path = "conversation_client_tests.rs"]
mod tests;

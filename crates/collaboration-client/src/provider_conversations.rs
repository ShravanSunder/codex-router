//! Where a provider conversation's operations run.
//!
//! Through the collaboration API, a create, load or prompt is one composite tool call: the
//! Router submits the operation and waits for its settlement. Inside the Host, the same
//! composition runs here over the Router's own operations.
use crate::api_connection::rejection_from_tool_failure;
use crate::{ClientError, CollaborationClient, ConversationClientError, LocalCollaboration};
use collaboration_protocol::{
    ConversationCancelRequest, ConversationCreateRequest, ConversationLoadRequest,
    ConversationOperationSnapshot, ConversationOperationSubmission,
    ConversationOperationWaitRequest, ConversationOperationWaitResult, ConversationPromptRequest,
    OperationId, UuidIdentity,
};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::{sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

/// How long a composite call may outlast its own wait before the client stops waiting.
const COMPOSITE_CALL_ALLOWANCE: Duration = Duration::from_secs(30);

pub enum ProviderConversations {
    /// The collaboration API: one composite tool call per operation.
    Api(CollaborationClient),
    /// The Router's own operations, for callers inside the Host.
    Local(Arc<dyn LocalCollaboration>),
}

impl ProviderConversations {
    pub(crate) fn service_id(&self) -> UuidIdentity {
        match self {
            Self::Api(client) => client.identity().service_id.clone(),
            Self::Local(router) => router.service_id(),
        }
    }
}

/// The outcome of one composite tool call.
pub(crate) enum CompositeAnswer<TResult> {
    Settled(TResult),
    /// The call outlasted its wait; the operation may still settle.
    StillRunning,
}

/// Runs one composite conversation tool, waiting up to `timeout` for the Router's own wait.
pub(crate) async fn composite_call<TResult: DeserializeOwned>(
    client: &CollaborationClient,
    tool: &str,
    arguments: Value,
    timeout: Duration,
    cancel: Option<(&CancellationToken, &OperationId)>,
) -> Result<CompositeAnswer<TResult>, ConversationClientError> {
    let call = client.connection.answer_with_timeout(
        tool,
        arguments,
        timeout.saturating_add(COMPOSITE_CALL_ALLOWANCE),
    );
    let answer = match cancel {
        Some((cancel, operation_id)) => tokio::select! {
            () = cancel.cancelled() => {
                return Err(ConversationClientError::CallerCancelled {
                    operation_id: operation_id.clone(),
                });
            }
            answer = call => answer,
        },
        None => call.await,
    };
    match answer {
        Ok(crate::api_connection::ToolAnswer::Success(value)) => serde_json::from_value(value)
            .map(CompositeAnswer::Settled)
            .map_err(|_| ClientError::Protocol("invalid provider conversation response").into()),
        Ok(crate::api_connection::ToolAnswer::Failure(failure)) => {
            Err(conversation_failure(failure))
        }
        Err(ClientError::Timeout) => Ok(CompositeAnswer::StillRunning),
        Err(error) => Err(error.into()),
    }
}

/// The conversation error a composite tool reported.
fn conversation_failure(failure: Value) -> ConversationClientError {
    if failure.get("kind").and_then(Value::as_str) == Some("callerCancelled")
        && let Some(operation_id) = failure
            .get("operationId")
            .cloned()
            .and_then(|value| serde_json::from_value::<OperationId>(value).ok())
    {
        return ConversationClientError::CallerCancelled { operation_id };
    }
    ConversationClientError::Client(rejection_from_tool_failure(failure))
}

/// The tool arguments for a conversation input with its bounded wait.
pub(crate) fn composite_arguments(
    input: &impl serde::Serialize,
    timeout: Duration,
) -> Result<Value, ConversationClientError> {
    let mut arguments = serde_json::to_value(input)
        .map_err(|_| ConversationClientError::InvalidInput("invalid conversation input"))?;
    let seconds = u32::try_from(timeout.as_secs()).map_err(|_| {
        ConversationClientError::InvalidInput(
            "timeout must be whole seconds within the supported range",
        )
    })?;
    if let Some(fields) = arguments.as_object_mut() {
        fields.insert("timeoutSeconds".to_owned(), Value::from(seconds));
    }
    Ok(arguments)
}

/// The Router's own provider operations, checked to answer for the operation submitted.
pub(crate) struct LocalProviderOperations<'router>(pub(crate) &'router dyn LocalCollaboration);

impl LocalProviderOperations<'_> {
    pub(crate) async fn create_provider_conversation(
        &self,
        request: ConversationCreateRequest,
    ) -> Result<ConversationOperationSubmission, ClientError> {
        let operation_id = request.operation_id.clone();
        let submission = self.0.create_provider_conversation(request).await?;
        same_operation(&operation_id, &submission.operation)?;
        Ok(submission)
    }

    pub(crate) async fn load_provider_conversation(
        &self,
        request: ConversationLoadRequest,
    ) -> Result<ConversationOperationSubmission, ClientError> {
        let operation_id = request.operation_id.clone();
        let submission = self.0.load_provider_conversation(request).await?;
        same_operation(&operation_id, &submission.operation)?;
        Ok(submission)
    }

    pub(crate) async fn prompt_provider_conversation(
        &self,
        request: ConversationPromptRequest,
    ) -> Result<ConversationOperationSubmission, ClientError> {
        let operation_id = request.operation_id.clone();
        let submission = self.0.prompt_provider_conversation(request).await?;
        same_operation(&operation_id, &submission.operation)?;
        Ok(submission)
    }

    pub(crate) async fn cancel_provider_conversation_operation(
        &self,
        request: ConversationCancelRequest,
    ) -> Result<ConversationOperationSubmission, ClientError> {
        let operation_id = request.operation_id.clone();
        let submission = self
            .0
            .cancel_provider_conversation_operation(request)
            .await?;
        same_operation(&operation_id, &submission.operation)?;
        Ok(submission)
    }

    pub(crate) async fn wait_for_provider_conversation_operation(
        &self,
        request: ConversationOperationWaitRequest,
    ) -> Result<ConversationOperationWaitResult, ClientError> {
        let operation_id = request.operation_id.clone();
        let waited = self
            .0
            .wait_for_provider_conversation_operation(request)
            .await?;
        same_operation(&operation_id, &waited.operation)?;
        Ok(waited)
    }
}

fn same_operation(
    requested: &OperationId,
    snapshot: &ConversationOperationSnapshot,
) -> Result<(), ClientError> {
    if &snapshot.operation_id == requested {
        Ok(())
    } else {
        Err(ClientError::Protocol(
            "provider conversation operation identity changed",
        ))
    }
}

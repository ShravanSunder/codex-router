//! Provider conversation submissions through the CLIs' conversation client on the collaboration
//! API.
//!
//! The API has no bare submission: a create, load or prompt is one composite call that also
//! waits for the operation to settle, and a cancel is one exact call. These helpers make those
//! calls from the protocol request a test builds, so the test still reads the settlement with
//! `conversation_operation_wait`. A prompt that stays pending until the test acts runs in the
//! background.
//!
//! The API's prompt input names Session actors only: a prompt whose approver is a Human runs
//! with the requester as its approver.
use collaboration_client::{
    CollaborationAccess, CollaborationClient, ConversationCancelInput, ConversationClient,
    ConversationClientError, ConversationCreateActor, ConversationCreateInput,
    ConversationLoadInput, ConversationOperationResult, ConversationPromptInput,
    PublicPromptContent,
};
use collaboration_protocol::{
    ConversationCancelRequest, ConversationCreateRequest, ConversationLoadRequest,
    ConversationOperationSubmission, ConversationPromptRequest, EndpointRef, ProviderIdentity,
    SessionRef,
};
use std::{path::PathBuf, time::Duration};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// How long one composite call waits for its operation before answering pending.
const COMPOSITE_WAIT: Duration = Duration::from_secs(30);

async fn conversation_client(
    client: &CollaborationClient,
    endpoint: &EndpointRef,
) -> Result<ConversationClient, ConversationClientError> {
    ConversationClient::connect(&CollaborationAccess::api(client.directory()), endpoint).await
}

fn create_actor(
    identity: &ProviderIdentity,
) -> Result<ConversationCreateActor, ConversationClientError> {
    match identity {
        ProviderIdentity::Session(session) => Ok(ConversationCreateActor::Session(session.clone())),
        ProviderIdentity::Human { .. } => identity
            .to_board_identity()
            .map(ConversationCreateActor::Typed)
            .map_err(ConversationClientError::InvalidInput),
    }
}

fn session_actor(
    identity: &ProviderIdentity,
    field: &'static str,
) -> Result<SessionRef, ConversationClientError> {
    identity
        .session()
        .cloned()
        .ok_or(ConversationClientError::InvalidInput(field))
}

/// Creates the provider conversation `request` names and waits for it to settle.
pub(crate) async fn submit_provider_create(
    client: &CollaborationClient,
    request: ConversationCreateRequest,
) -> Result<(), ConversationClientError> {
    let (model, mode, effort) = request.settings.map_or((None, None, None), |settings| {
        (settings.model, settings.mode, settings.effort)
    });
    let input = ConversationCreateInput {
        operation_id: request.operation_id,
        endpoint: request.endpoint.clone(),
        working_directory: PathBuf::from(String::from(request.working_directory)),
        access: request.requested_policy.access,
        created_by: create_actor(&request.created_by)?,
        approver: Some(create_actor(&request.approver)?),
        generation: request.generation,
        model,
        mode,
        effort,
        fork: None,
        root_message_id: None,
    };
    conversation_client(client, &request.endpoint)
        .await?
        .create(input, COMPOSITE_WAIT)
        .await
        .map(|_| ())
}

/// Loads the provider conversation `request` names and waits for it to settle.
pub(crate) async fn submit_provider_load(
    client: &CollaborationClient,
    request: ConversationLoadRequest,
) -> Result<ConversationOperationResult, ConversationClientError> {
    let input = ConversationLoadInput {
        operation_id: Some(request.operation_id),
        target: request.target.clone(),
        working_directory: PathBuf::from(String::from(request.working_directory)),
        requested_by: session_actor(&request.requested_by, "load requester must be a Session")?,
        approver: request.approver.session().cloned(),
        access: request.requested_policy.access,
        generation: request.generation,
    };
    conversation_client(client, &request.target.endpoint)
        .await?
        .load(input, COMPOSITE_WAIT)
        .await
}

fn prompt_input(
    request: ConversationPromptRequest,
) -> Result<ConversationPromptInput, ConversationClientError> {
    let message: PublicPromptContent = serde_json::to_value(request.prompt)
        .and_then(serde_json::from_value)
        .map_err(|_| ConversationClientError::InvalidInput("prompt content is not public"))?;
    Ok(ConversationPromptInput {
        operation_id: Some(request.operation_id),
        target: request.target,
        working_directory: None,
        requested_by: session_actor(&request.requested_by, "prompt requester must be a Session")?,
        approver: request.approver.session().cloned(),
        message,
        effort: None,
        generation: request.generation,
    })
}

/// Prompts the provider conversation `request` names and waits for it to settle.
pub(crate) async fn submit_provider_prompt(
    client: &CollaborationClient,
    request: ConversationPromptRequest,
) -> Result<ConversationOperationResult, ConversationClientError> {
    let endpoint = request.target.endpoint.clone();
    let input = prompt_input(request)?;
    conversation_client(client, &endpoint)
        .await?
        .prompt(input, COMPOSITE_WAIT, CancellationToken::new())
        .await
}

/// Prompts in the background, for a prompt that settles only after the test acts.
pub(crate) fn spawn_provider_prompt(
    client: &CollaborationClient,
    request: ConversationPromptRequest,
) -> JoinHandle<Result<ConversationOperationResult, ConversationClientError>> {
    let client = client.clone();
    tokio::spawn(async move { submit_provider_prompt(&client, request).await })
}

/// Requests cancellation of one provider operation; the cancel itself does not wait.
pub(crate) async fn submit_provider_cancel(
    client: &CollaborationClient,
    request: ConversationCancelRequest,
) -> Result<ConversationOperationSubmission, ConversationClientError> {
    let input = ConversationCancelInput {
        operation_id: request.operation_id,
        target_operation_id: request.target_operation_id,
        target: request.target.clone(),
        requested_by: session_actor(&request.requested_by, "cancel requester must be a Session")?,
        approver: request.approver.session().cloned(),
        generation: request.generation,
    };
    conversation_client(client, &request.target.endpoint)
        .await?
        .cancel(input)
        .await
}

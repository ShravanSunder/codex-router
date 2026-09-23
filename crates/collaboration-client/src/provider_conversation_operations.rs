//! Typed Control calls for Host-owned external ACP conversations.

use crate::{ClientError, ControlClient};
use collaboration_protocol::{
    ConversationCancelRequest, ConversationCreateRequest, ConversationLoadRequest,
    ConversationOperationReconcileRequest, ConversationOperationShowRequest,
    ConversationOperationSnapshot, ConversationOperationSubmission,
    ConversationOperationWaitRequest, ConversationOperationWaitResult, ConversationPromptRequest,
    OperationId,
};
use serde::{Serialize, de::DeserializeOwned};
use std::time::Duration;

const PROVIDER_WAIT_TRANSPORT_ALLOWANCE: Duration = Duration::from_secs(5);

impl ControlClient {
    pub async fn create_provider_conversation(
        &mut self,
        request: ConversationCreateRequest,
    ) -> Result<ConversationOperationSubmission, ClientError> {
        self.submit_provider_operation("conversation/create", request.operation_id.clone(), request)
            .await
    }

    pub async fn load_provider_conversation(
        &mut self,
        request: ConversationLoadRequest,
    ) -> Result<ConversationOperationSubmission, ClientError> {
        self.submit_provider_operation("conversation/load", request.operation_id.clone(), request)
            .await
    }

    pub async fn prompt_provider_conversation(
        &mut self,
        request: ConversationPromptRequest,
    ) -> Result<ConversationOperationSubmission, ClientError> {
        self.submit_provider_operation("conversation/prompt", request.operation_id.clone(), request)
            .await
    }

    pub async fn cancel_provider_conversation_operation(
        &mut self,
        request: ConversationCancelRequest,
    ) -> Result<ConversationOperationSubmission, ClientError> {
        self.submit_provider_operation("conversation/cancel", request.operation_id.clone(), request)
            .await
    }

    pub async fn show_provider_conversation_operation(
        &mut self,
        request: ConversationOperationShowRequest,
    ) -> Result<ConversationOperationSnapshot, ClientError> {
        let operation_id = request.operation_id.clone();
        let snapshot = self
            .call_provider_operation("conversation/operationShow", request)
            .await?;
        self.validate_provider_operation_id(&operation_id, &snapshot)?;
        Ok(snapshot)
    }

    pub async fn wait_for_provider_conversation_operation(
        &mut self,
        request: ConversationOperationWaitRequest,
    ) -> Result<ConversationOperationWaitResult, ClientError> {
        let operation_id = request.operation_id.clone();
        let server_wait = Duration::from_secs(u64::from(u32::from(request.timeout_seconds)));
        let transport_timeout = server_wait
            .checked_add(PROVIDER_WAIT_TRANSPORT_ALLOWANCE)
            .unwrap_or(Duration::MAX);
        let params = serde_json::to_value(request)
            .map_err(|_| ClientError::InvalidRequest("invalid provider conversation request"))?;
        let response = self
            .connection
            .call_with_timeout("conversation/operationWait", params, transport_timeout)
            .await?;
        let result: ConversationOperationWaitResult =
            serde_json::from_value(response).map_err(|_| {
                self.connection.failed = true;
                ClientError::Protocol("invalid provider conversation response")
            })?;
        self.validate_provider_operation_id(&operation_id, &result.operation)?;
        Ok(result)
    }

    pub async fn reconcile_provider_conversation_operation(
        &mut self,
        request: ConversationOperationReconcileRequest,
    ) -> Result<ConversationOperationSnapshot, ClientError> {
        let operation_id = request.operation_id.clone();
        let snapshot = self
            .call_provider_operation("conversation/operationReconcile", request)
            .await?;
        self.validate_provider_operation_id(&operation_id, &snapshot)?;
        Ok(snapshot)
    }

    async fn submit_provider_operation<Request>(
        &mut self,
        method: &'static str,
        operation_id: OperationId,
        request: Request,
    ) -> Result<ConversationOperationSubmission, ClientError>
    where
        Request: Serialize,
    {
        let submission: ConversationOperationSubmission =
            self.call_provider_operation(method, request).await?;
        self.validate_provider_operation_id(&operation_id, &submission.operation)?;
        Ok(submission)
    }

    async fn call_provider_operation<Request, Response>(
        &mut self,
        method: &'static str,
        request: Request,
    ) -> Result<Response, ClientError>
    where
        Request: Serialize,
        Response: DeserializeOwned,
    {
        let params = serde_json::to_value(request)
            .map_err(|_| ClientError::InvalidRequest("invalid provider conversation request"))?;
        let response = self.connection.call(method, params).await?;
        serde_json::from_value(response).map_err(|_| {
            self.connection.failed = true;
            ClientError::Protocol("invalid provider conversation response")
        })
    }

    fn validate_provider_operation_id(
        &mut self,
        requested: &OperationId,
        snapshot: &ConversationOperationSnapshot,
    ) -> Result<(), ClientError> {
        if &snapshot.operation_id == requested {
            return Ok(());
        }
        self.connection.failed = true;
        Err(ClientError::Protocol(
            "provider conversation operation identity changed",
        ))
    }
}

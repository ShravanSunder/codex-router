//! Typed calls for Host-owned external provider conversations: inspection, settings,
//! resume, close and operation inspection. Create, load, prompt and cancel run as composite
//! tools (`ProviderConversations`).

use crate::{ClientError, CollaborationClient};
use collaboration_protocol::{
    ConversationCloseRequest, ConversationOperationReconcileRequest,
    ConversationOperationShowRequest, ConversationOperationSnapshot,
    ConversationOperationSubmission, ConversationOperationWaitRequest,
    ConversationOperationWaitResult, ConversationResumeRequest, OperationId,
    ProviderSessionInspectRequest, ProviderSessionInspectResult, ProviderSettingsAcceptRequest,
    ProviderSettingsResult, ProviderSettingsSetRequest,
};
use serde::{Serialize, de::DeserializeOwned};
use std::time::Duration;

const PROVIDER_WAIT_TRANSPORT_ALLOWANCE: Duration = Duration::from_secs(5);

impl CollaborationClient {
    pub async fn inspect_provider_session(
        &self,
        request: ProviderSessionInspectRequest,
    ) -> Result<ProviderSessionInspectResult, ClientError> {
        self.call_provider_operation("provider_session_inspect", request)
            .await
    }

    pub async fn set_provider_conversation_setting(
        &self,
        request: ProviderSettingsSetRequest,
    ) -> Result<ProviderSettingsResult, ClientError> {
        self.call_provider_operation("conversation_settings_set", request)
            .await
    }

    pub async fn accept_provider_conversation_settings(
        &self,
        request: ProviderSettingsAcceptRequest,
    ) -> Result<ProviderSettingsResult, ClientError> {
        self.call_provider_operation("conversation_settings_accept", request)
            .await
    }

    pub async fn resume_provider_conversation(
        &self,
        request: ConversationResumeRequest,
    ) -> Result<ConversationOperationSubmission, ClientError> {
        self.submit_provider_operation("conversation_resume", request.operation_id.clone(), request)
            .await
    }

    pub async fn close_provider_conversation(
        &self,
        request: ConversationCloseRequest,
    ) -> Result<ConversationOperationSubmission, ClientError> {
        self.submit_provider_operation("conversation_close", request.operation_id.clone(), request)
            .await
    }

    pub async fn show_provider_conversation_operation(
        &self,
        request: ConversationOperationShowRequest,
    ) -> Result<ConversationOperationSnapshot, ClientError> {
        let operation_id = request.operation_id.clone();
        let snapshot = self
            .call_provider_operation("conversation_operation_show", request)
            .await?;
        self.validate_provider_operation_id(&operation_id, &snapshot)?;
        Ok(snapshot)
    }

    pub async fn wait_for_provider_conversation_operation(
        &self,
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
            .call_with_timeout("conversation_operation_wait", params, transport_timeout)
            .await?;
        let result: ConversationOperationWaitResult = serde_json::from_value(response)
            .map_err(|_| ClientError::Protocol("invalid provider conversation response"))?;
        self.validate_provider_operation_id(&operation_id, &result.operation)?;
        Ok(result)
    }

    pub async fn reconcile_provider_conversation_operation(
        &self,
        request: ConversationOperationReconcileRequest,
    ) -> Result<ConversationOperationSnapshot, ClientError> {
        let operation_id = request.operation_id.clone();
        let snapshot = self
            .call_provider_operation("conversation_operation_reconcile", request)
            .await?;
        self.validate_provider_operation_id(&operation_id, &snapshot)?;
        Ok(snapshot)
    }

    async fn submit_provider_operation<Request>(
        &self,
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
        &self,
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
        serde_json::from_value(response)
            .map_err(|_| ClientError::Protocol("invalid provider conversation response"))
    }

    fn validate_provider_operation_id(
        &self,
        requested: &OperationId,
        snapshot: &ConversationOperationSnapshot,
    ) -> Result<(), ClientError> {
        if &snapshot.operation_id == requested {
            return Ok(());
        }
        Err(ClientError::Protocol(
            "provider conversation operation identity changed",
        ))
    }
}

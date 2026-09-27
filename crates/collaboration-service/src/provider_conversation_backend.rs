//! Host-owned execution boundary for external ACP conversation operations.
use collaboration_protocol::{
    ConversationCancelRequest, ConversationCloseRequest, ConversationCreateRequest,
    ConversationLoadRequest, ConversationOperationFailure, ConversationOperationReconcileRequest,
    ConversationOperationShowRequest, ConversationOperationSnapshot,
    ConversationOperationSubmission, ConversationOperationWaitRequest,
    ConversationOperationWaitResult, ConversationPromptRequest, ConversationResumeRequest,
    EndpointRef, ProviderBindingIdentity, ProviderInspectFailure, ProviderInspectFailureKind,
    ProviderSessionInspectRequest, ProviderSessionInspectResult, ProviderSettingsAcceptRequest,
    ProviderSettingsFailure, ProviderSettingsFailureKind, ProviderSettingsResult,
    ProviderSettingsSetRequest,
};
use std::{future::Future, pin::Pin};

pub type ProviderConversationFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, ConversationOperationFailure>> + Send + 'a>>;

pub type ProviderSettingsFuture<'a> = Pin<
    Box<dyn Future<Output = Result<ProviderSettingsResult, ProviderSettingsFailure>> + Send + 'a>,
>;

pub type ProviderSessionInspectFuture<'a> = Pin<
    Box<
        dyn Future<Output = Result<ProviderSessionInspectResult, ProviderInspectFailure>>
            + Send
            + 'a,
    >,
>;

/// Host-owned provider work. Dropping one returned future only detaches its caller;
/// implementations retain ownership of admitted provider operations.
pub trait ProviderConversationBackend: Send + Sync {
    fn binding(&self, endpoint: &EndpointRef) -> Option<ProviderBindingIdentity>;
    fn inspect_session(
        &self,
        request: ProviderSessionInspectRequest,
    ) -> ProviderSessionInspectFuture<'_> {
        Box::pin(async move {
            Err(ProviderInspectFailure {
                kind: ProviderInspectFailureKind::Unavailable,
                stage: None,
                target: Some(request.target),
                message: "provider Session inspection is unavailable".into(),
            })
        })
    }
    fn settings_set(&self, request: ProviderSettingsSetRequest) -> ProviderSettingsFuture<'_> {
        Box::pin(async move { Err(settings_unavailable(request.target)) })
    }
    fn settings_accept(
        &self,
        request: ProviderSettingsAcceptRequest,
    ) -> ProviderSettingsFuture<'_> {
        Box::pin(async move { Err(settings_unavailable(request.target)) })
    }
    fn lookup_existing(
        &self,
        _operation_id: collaboration_protocol::OperationId,
    ) -> ProviderConversationFuture<'_, Option<ConversationOperationSnapshot>> {
        Box::pin(async { Ok(None) })
    }
    fn create(
        &self,
        request: ConversationCreateRequest,
    ) -> ProviderConversationFuture<'_, ConversationOperationSubmission>;
    fn load(
        &self,
        request: ConversationLoadRequest,
    ) -> ProviderConversationFuture<'_, ConversationOperationSubmission>;
    fn resume(
        &self,
        request: ConversationResumeRequest,
    ) -> ProviderConversationFuture<'_, ConversationOperationSubmission>;
    fn close(
        &self,
        request: ConversationCloseRequest,
    ) -> ProviderConversationFuture<'_, ConversationOperationSubmission>;
    fn prompt(
        &self,
        request: ConversationPromptRequest,
    ) -> ProviderConversationFuture<'_, ConversationOperationSubmission>;
    fn cancel(
        &self,
        request: ConversationCancelRequest,
    ) -> ProviderConversationFuture<'_, ConversationOperationSubmission>;
    fn show(
        &self,
        request: ConversationOperationShowRequest,
    ) -> ProviderConversationFuture<'_, ConversationOperationSnapshot>;
    fn wait(
        &self,
        request: ConversationOperationWaitRequest,
    ) -> ProviderConversationFuture<'_, ConversationOperationWaitResult>;
    fn reconcile(
        &self,
        request: ConversationOperationReconcileRequest,
    ) -> ProviderConversationFuture<'_, ConversationOperationSnapshot>;
}

fn settings_unavailable(target: collaboration_protocol::SessionRef) -> ProviderSettingsFailure {
    ProviderSettingsFailure {
        kind: ProviderSettingsFailureKind::Unavailable,
        stage: None,
        target: Some(target),
        message: "provider settings are unavailable".into(),
        setting: None,
        value: None,
        advertised: Vec::new(),
    }
}

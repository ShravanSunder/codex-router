//! Host-owned execution boundary for external ACP conversation operations.
use collaboration_protocol::{
    ConversationCancelRequest, ConversationCreateRequest, ConversationLoadRequest,
    ConversationOperationFailure, ConversationOperationReconcileRequest,
    ConversationOperationShowRequest, ConversationOperationSnapshot,
    ConversationOperationSubmission, ConversationOperationWaitRequest,
    ConversationOperationWaitResult, ConversationPromptRequest, EndpointRef,
    ProviderBindingIdentity,
};
use std::{future::Future, pin::Pin};

pub type ProviderConversationFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, ConversationOperationFailure>> + Send + 'a>>;

/// Host-owned provider work. Dropping one returned future only detaches its caller;
/// implementations retain ownership of admitted provider operations.
pub trait ProviderConversationBackend: Send + Sync {
    fn binding(&self, endpoint: &EndpointRef) -> Option<ProviderBindingIdentity>;
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

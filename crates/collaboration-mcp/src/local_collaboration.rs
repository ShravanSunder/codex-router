//! The Router's own collaboration operations for the carrier clients the tools run in the Host.
//!
//! The conversation and observation tools run the same carrier clients the CLIs do. Inside
//! the Host those clients discover carriers and run provider operations here, in process,
//! instead of calling the API over its socket.
use collaboration_client::{ClientError, LocalCollaboration, LocalFuture};
use collaboration_protocol::{
    BoundedObservationRequest, BoundedObservationResult, ConversationCancelRequest,
    ConversationCreateRequest, ConversationLoadRequest, ConversationOperationSubmission,
    ConversationOperationWaitRequest, ConversationOperationWaitResult, ConversationPromptRequest,
    EndpointInventory, UuidIdentity,
};
use collaboration_service::CollaborationApplication;
use collaboration_service::collaboration_application::CollaborationRejection;

pub(crate) struct ApplicationCollaboration {
    application: CollaborationApplication,
}

impl ApplicationCollaboration {
    pub(crate) fn new(application: CollaborationApplication) -> Self {
        Self { application }
    }
}

/// The rejection as the client error a caller of the API would see.
fn rejected(rejection: &impl CollaborationRejection) -> ClientError {
    let published = rejection.published_rejection();
    ClientError::Rejected {
        code: published.code,
        data: published.data,
    }
}

impl LocalCollaboration for ApplicationCollaboration {
    fn service_id(&self) -> UuidIdentity {
        self.application.service_id().clone()
    }

    fn endpoints(&self) -> Result<EndpointInventory, ClientError> {
        self.application
            .sessions()
            .endpoints_list()
            .map_err(|failure| rejected(&failure))
    }

    fn create_provider_conversation(
        &self,
        request: ConversationCreateRequest,
    ) -> LocalFuture<'_, ConversationOperationSubmission> {
        Box::pin(async move {
            self.application
                .conversations()
                .conversation_create(request)
                .await
                .map_err(|failure| rejected(&failure))
        })
    }

    fn load_provider_conversation(
        &self,
        request: ConversationLoadRequest,
    ) -> LocalFuture<'_, ConversationOperationSubmission> {
        Box::pin(async move {
            self.application
                .conversations()
                .conversation_load(request)
                .await
                .map_err(|failure| rejected(&failure))
        })
    }

    fn prompt_provider_conversation(
        &self,
        request: ConversationPromptRequest,
    ) -> LocalFuture<'_, ConversationOperationSubmission> {
        Box::pin(async move {
            self.application
                .conversations()
                .conversation_prompt(request)
                .await
                .map_err(|failure| rejected(&failure))
        })
    }

    fn cancel_provider_conversation_operation(
        &self,
        request: ConversationCancelRequest,
    ) -> LocalFuture<'_, ConversationOperationSubmission> {
        Box::pin(async move {
            self.application
                .conversations()
                .conversation_cancel(request)
                .await
                .map_err(|failure| rejected(&failure))
        })
    }

    fn wait_for_provider_conversation_operation(
        &self,
        request: ConversationOperationWaitRequest,
    ) -> LocalFuture<'_, ConversationOperationWaitResult> {
        Box::pin(async move {
            self.application
                .conversations()
                .operation_wait(request)
                .await
                .map_err(|failure| rejected(&failure))
        })
    }

    fn observe_provider_session(
        &self,
        request: BoundedObservationRequest,
    ) -> LocalFuture<'_, BoundedObservationResult> {
        Box::pin(async move {
            self.application
                .observation()
                .session_observe(request)
                .await
                .map_err(|failure| rejected(&failure))
        })
    }
}

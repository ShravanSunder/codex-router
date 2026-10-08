//! How a carrier client reaches the Router's collaboration operations.
//!
//! The carrier clients (Codex conversations over ACP, Codex native observation, the stdio
//! bridges) discover their carrier socket from the endpoint directory and run provider
//! operations through the Router. A CLI or bridge reaches the Router through the
//! collaboration API on the service socket; the same clients running inside the Host, as
//! tools, reach it directly.
use crate::{ClientError, CollaborationClient};
use collaboration_protocol::{
    BoundedObservationRequest, BoundedObservationResult, ConversationCancelRequest,
    ConversationCreateRequest, ConversationLoadRequest, ConversationOperationSubmission,
    ConversationOperationWaitRequest, ConversationOperationWaitResult, ConversationPromptRequest,
    EndpointInventory, UuidIdentity,
};
use std::{
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
};

/// An operation the Router performs for a caller inside the Host.
pub type LocalFuture<'a, TResult> =
    Pin<Box<dyn Future<Output = Result<TResult, ClientError>> + Send + 'a>>;

/// The Router's own collaboration operations, for carrier clients running inside the Host.
/// A failure is the rejection the Router published, as `ClientError::Rejected`.
pub trait LocalCollaboration: Send + Sync {
    fn service_id(&self) -> UuidIdentity;
    fn endpoints(&self) -> Result<EndpointInventory, ClientError>;
    fn create_provider_conversation(
        &self,
        request: ConversationCreateRequest,
    ) -> LocalFuture<'_, ConversationOperationSubmission>;
    fn load_provider_conversation(
        &self,
        request: ConversationLoadRequest,
    ) -> LocalFuture<'_, ConversationOperationSubmission>;
    fn prompt_provider_conversation(
        &self,
        request: ConversationPromptRequest,
    ) -> LocalFuture<'_, ConversationOperationSubmission>;
    fn cancel_provider_conversation_operation(
        &self,
        request: ConversationCancelRequest,
    ) -> LocalFuture<'_, ConversationOperationSubmission>;
    fn wait_for_provider_conversation_operation(
        &self,
        request: ConversationOperationWaitRequest,
    ) -> LocalFuture<'_, ConversationOperationWaitResult>;
    fn observe_provider_session(
        &self,
        request: BoundedObservationRequest,
    ) -> LocalFuture<'_, BoundedObservationResult>;
}

/// Where a carrier client's collaboration operations come from.
#[derive(Clone)]
pub enum CollaborationAccess {
    /// The collaboration API on the service directory's socket: the CLIs and bridges.
    Api { directory: PathBuf },
    /// The Router itself, for carrier clients running inside the Host.
    Local {
        directory: PathBuf,
        router: Arc<dyn LocalCollaboration>,
    },
}

impl CollaborationAccess {
    #[must_use]
    pub fn api(directory: &Path) -> Self {
        Self::Api {
            directory: directory.to_owned(),
        }
    }

    #[must_use]
    pub fn local(directory: &Path, router: Arc<dyn LocalCollaboration>) -> Self {
        Self::Local {
            directory: directory.to_owned(),
            router,
        }
    }

    /// The service directory carrier socket paths are relative to.
    #[must_use]
    pub fn directory(&self) -> &Path {
        match self {
            Self::Api { directory } | Self::Local { directory, .. } => directory,
        }
    }

    /// Opens the endpoint directory this access reads.
    pub(crate) async fn endpoint_directory(
        &self,
        client_name: &str,
    ) -> Result<EndpointDirectoryReader, ClientError> {
        match self {
            Self::Api { directory } => {
                CollaborationClient::connect(directory, client_name, env!("CARGO_PKG_VERSION"))
                    .await
                    .map(EndpointDirectoryReader::Api)
            }
            Self::Local { router, .. } => Ok(EndpointDirectoryReader::Local(Arc::clone(router))),
        }
    }
}

/// One caller's view of the endpoint directory, which it may read more than once.
pub(crate) enum EndpointDirectoryReader {
    Api(CollaborationClient),
    Local(Arc<dyn LocalCollaboration>),
}

impl EndpointDirectoryReader {
    pub(crate) fn service_id(&self) -> UuidIdentity {
        match self {
            Self::Api(client) => client.identity().service_id.clone(),
            Self::Local(router) => router.service_id(),
        }
    }

    pub(crate) async fn endpoints(&self) -> Result<EndpointInventory, ClientError> {
        match self {
            Self::Api(client) => client.list_endpoints().await,
            Self::Local(router) => router.endpoints(),
        }
    }
}

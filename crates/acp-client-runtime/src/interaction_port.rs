//! The ACP client asks Host policy to route and settle provider interactions.

use std::{future::Future, hash::Hash, pin::Pin};
use tokio_util::sync::CancellationToken;

use crate::RefusedPermissionOption;
use session_event_model::{ApprovalRequest, ApprovalSubject};

pub type InteractionFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ApprovalPortOutcome {
    Selected { option_id: String },
    Cancelled,
    Unavailable,
}

/// Malformed ACP offers are recorded and answered cancelled. This type never
/// appears in the public session profile or replaces a valid ApprovalRequest.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RefusedApprovalOffer {
    pub request_id: String,
    pub reason: &'static str,
    pub title: String,
    pub description: Option<String>,
    pub subject: Option<ApprovalSubject>,
    pub options: Vec<RefusedPermissionOption>,
}

/// The context is Host-owned. ACP code never names Router operation or generation types.
pub trait InteractionPort: Send + Sync + 'static {
    type Context: Clone + Send + Sync + 'static;
    type OperationId: Clone + Eq + Hash + Send + Sync + 'static;

    fn operation_id(context: &Self::Context) -> Self::OperationId;

    fn binding_retirement(context: &Self::Context) -> CancellationToken;

    fn request_approval(
        &self,
        context: Self::Context,
        request: ApprovalRequest,
        turn_cancellation: CancellationToken,
        agent_cancellation: CancellationToken,
    ) -> InteractionFuture<'_, ApprovalPortOutcome>;

    fn record_refusal(
        &self,
        context: Self::Context,
        refusal: RefusedApprovalOffer,
    ) -> InteractionFuture<'_, ()>;

    fn cancel_all(&self, context: Self::Context, reason: &'static str)
    -> InteractionFuture<'_, ()>;

    fn cancel_retired(&self) -> InteractionFuture<'_, ()>;
}

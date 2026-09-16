//! Provider-neutral routing boundary for client-exposed native approvals.
use collaboration_protocol::{CodexGeneration, SessionRef};
use serde_json::Value;
use std::{future::Future, pin::Pin};

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApprovalRoute {
    pub thread_id: String,
    pub created_by: SessionRef,
    pub approver: SessionRef,
}

#[derive(Clone, Debug)]
pub struct BrokeredApprovalRequest {
    pub thread_id: String,
    pub generation: CodexGeneration,
    pub request: Value,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BrokeredApprovalOutcome {
    Selected { option_id: String },
    Cancelled,
}

#[derive(Debug, thiserror::Error)]
pub enum ApprovalBrokerError {
    #[error("approval route unavailable")]
    RouteUnavailable,
    #[error("approval broker unavailable")]
    Unavailable,
}

pub trait ApprovalBroker: Send + Sync {
    fn register_route(
        &self,
        route: ApprovalRoute,
    ) -> Pin<Box<dyn Future<Output = Result<(), ApprovalBrokerError>> + Send + '_>>;

    fn request(
        &self,
        request: BrokeredApprovalRequest,
    ) -> Pin<
        Box<dyn Future<Output = Result<BrokeredApprovalOutcome, ApprovalBrokerError>> + Send + '_>,
    >;
}

pub struct RejectingApprovalBroker;

impl ApprovalBroker for RejectingApprovalBroker {
    fn register_route(
        &self,
        _route: ApprovalRoute,
    ) -> Pin<Box<dyn Future<Output = Result<(), ApprovalBrokerError>> + Send + '_>> {
        Box::pin(async { Ok(()) })
    }

    fn request(
        &self,
        _request: BrokeredApprovalRequest,
    ) -> Pin<
        Box<dyn Future<Output = Result<BrokeredApprovalOutcome, ApprovalBrokerError>> + Send + '_>,
    > {
        Box::pin(async { Ok(BrokeredApprovalOutcome::Cancelled) })
    }
}

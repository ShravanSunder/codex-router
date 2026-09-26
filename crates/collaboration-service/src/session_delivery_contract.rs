//! The feature-facing delivery seam and the route-facing client contract.
use agent_automation::RouteEffectEvidence;
use collaboration_protocol::{
    AttemptId, CodexGeneration, DeliveryCorrelationId, DeliveryReceipt, MessageContent,
    MessageDelivery, SessionReachability, SessionRef,
};
use serde::{Deserialize, Serialize};
use std::{future::Future, pin::Pin, sync::Arc};

pub type DeliveryFuture<'a, TValue> =
    Pin<Box<dyn Future<Output = Result<TValue, DeliveryContractError>> + Send + 'a>>;

#[derive(Debug, thiserror::Error)]
pub enum DeliveryContractError {
    #[error("delivery evidence could not be recorded")]
    EvidencePersistence,
    #[error("stored delivery evidence is invalid")]
    InvalidEvidence,
    #[error("delivery client operation failed")]
    ClientOperation,
    #[error("prepared target is owned by another schedule")]
    PreparationOwnershipConflict,
    #[error("schedule changed during preparation")]
    PreparationChangeConflict,
}

#[derive(Clone, Debug)]
pub struct DeliveryRequest {
    pub target: SessionRef,
    pub message: MessageContent,
    pub mode: MessageDelivery,
    pub precondition: DeliveryPrecondition,
    pub correlation: DeliveryCorrelationId,
    pub attempt: AttemptId,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum DeliveryPrecondition {
    Unpinned,
    EndpointGeneration { expected: CodexGeneration },
}

pub struct AttemptReconciliationContext {
    pub target: SessionRef,
    pub message: MessageContent,
    pub mode: MessageDelivery,
    pub recorded: RouteEffectEvidence<SessionRef, CodexGeneration>,
}

pub enum AttemptReconciliation {
    Accepted(Box<DeliveryReceipt>),
    KnownNotSubmitted,
    StillUnknown,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RouteUnavailableReason {
    pub reason: String,
    pub fix: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum RouteClaim {
    NotMine,
    Holds,
    CanLoad,
    LiveElsewhere {
        writable: bool,
        detail: Option<String>,
    },
    Unavailable {
        reason: RouteUnavailableReason,
        retryable: bool,
    },
}

pub trait AttemptEvidenceSink: Send + Sync {
    fn record(
        &self,
        evidence: RouteEffectEvidence<SessionRef, CodexGeneration>,
    ) -> DeliveryFuture<'_, ()>;
}

/// Direct and transient messages have no durable attempt record to update.
pub(crate) struct UnstoredAttemptEvidenceSink;

impl AttemptEvidenceSink for UnstoredAttemptEvidenceSink {
    fn record(
        &self,
        _: RouteEffectEvidence<SessionRef, CodexGeneration>,
    ) -> DeliveryFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}

pub trait SessionMessageDelivery: Send + Sync {
    fn deliver<'a>(
        &'a self,
        request: DeliveryRequest,
        evidence: &'a dyn AttemptEvidenceSink,
    ) -> DeliveryFuture<'a, DeliveryReceipt>;

    fn reconcile_attempt(
        &self,
        context: AttemptReconciliationContext,
    ) -> DeliveryFuture<'_, AttemptReconciliation>;
}

pub trait SessionDeliveryRoute: Send + Sync {
    fn reachability(&self) -> SessionReachability;
    fn claim(&self, target: &SessionRef) -> DeliveryFuture<'_, RouteClaim>;
    fn deliver<'a>(
        &'a self,
        request: DeliveryRequest,
        evidence: &'a dyn AttemptEvidenceSink,
    ) -> DeliveryFuture<'a, DeliveryReceipt>;
    fn reconcile_attempt(
        &self,
        context: AttemptReconciliationContext,
    ) -> DeliveryFuture<'_, AttemptReconciliation>;
    fn scheduled_runs(&self) -> Option<Arc<dyn crate::ScheduledRunRoute>> {
        None
    }
}

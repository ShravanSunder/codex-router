//! Scheduled work uses the same selected route throughout one recorded run.
use crate::{DeliveryFuture, DeliveryPrecondition};
use agent_automation::{RouteEffectEvidence, RunId};
use collaboration_protocol::{CodexGeneration, EndpointRef, MessageText, NonEmptyText, SessionRef};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug)]
pub enum ScheduleDestination {
    Existing { target: SessionRef },
    Fresh { endpoint: EndpointRef },
}

#[derive(Clone, Debug)]
pub struct FreshSessionRequest {
    pub endpoint: EndpointRef,
    pub working_directory: String,
}

#[derive(Clone, Debug)]
pub struct PreparedTarget {
    pub target: SessionRef,
    pub evidence: RouteEffectEvidence<SessionRef, CodexGeneration>,
}

#[derive(Clone, Debug)]
pub struct ScheduledRunSubmission {
    pub run_id: RunId,
    pub target: SessionRef,
    pub message: MessageText,
    pub precondition: DeliveryPrecondition,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum RunSubmission {
    Started,
    NotStartedBusy,
    Rejected { reason: String },
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SettlementEvidence {
    TurnCompletion,
    OperationSettlement,
    WriteOnly,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScheduleSupport {
    pub can_create: bool,
    pub can_stop: bool,
    pub settlement: SettlementEvidence,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeTurnRef {
    pub target: SessionRef,
    pub turn_id: NonEmptyText,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum RunSummarySource {
    NativeTurn { turn: NativeTurnRef },
    ProviderResponse { text: String },
    Unavailable { reason: String },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum RunSettlement {
    Pending,
    Completed { summary_source: RunSummarySource },
    Stopped,
    WrittenWithoutCompletion,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum StopRequestOutcome {
    Requested,
    Unsupported,
    Unknown,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum RunReconciliation {
    Settled { settlement: RunSettlement },
    KnownNotSubmitted,
    StillUnknown,
}

pub trait RunEvidenceSink: Send + Sync {
    fn record(
        &self,
        evidence: RouteEffectEvidence<SessionRef, CodexGeneration>,
    ) -> DeliveryFuture<'_, ()>;
}

pub trait ScheduledRunExecution: Send + Sync {
    fn support(&self, destination: &ScheduleDestination) -> DeliveryFuture<'_, ScheduleSupport>;
    fn prepare_existing_target<'a>(
        &'a self,
        target: &SessionRef,
        sink: &'a dyn RunEvidenceSink,
    ) -> DeliveryFuture<'a, PreparedTarget>;
    fn prepare_fresh_session<'a>(
        &'a self,
        request: FreshSessionRequest,
        sink: &'a dyn RunEvidenceSink,
    ) -> DeliveryFuture<'a, PreparedTarget>;
    fn submit_run<'a>(
        &'a self,
        run: ScheduledRunSubmission,
        sink: &'a dyn RunEvidenceSink,
    ) -> DeliveryFuture<'a, RunSubmission>;
    fn observe_settlement(
        &self,
        recorded: &RouteEffectEvidence<SessionRef, CodexGeneration>,
    ) -> DeliveryFuture<'_, RunSettlement>;
    fn request_stop(
        &self,
        recorded: &RouteEffectEvidence<SessionRef, CodexGeneration>,
    ) -> DeliveryFuture<'_, StopRequestOutcome>;
    fn reconcile_run(
        &self,
        recorded: &RouteEffectEvidence<SessionRef, CodexGeneration>,
    ) -> DeliveryFuture<'_, RunReconciliation>;
}

pub trait ScheduledRunRoute: ScheduledRunExecution {
    fn supports_endpoint(&self, endpoint: &EndpointRef) -> bool;
}

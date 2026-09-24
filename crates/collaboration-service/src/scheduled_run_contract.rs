//! Scheduled work uses the same selected route throughout one recorded run.
use crate::{DeliveryFuture, DeliveryPrecondition};
use agent_automation::{CapturedRunInputs, RouteEffectEvidence, RunId, RunPhase};
use collaboration_protocol::{
    ChangeId, CodexGeneration, DeliveryReceipt, DeliveryRejection, DestinationPreparation,
    EndpointRef, MessageText, NonEmptyText, OperationId, RunExecution, ScheduleFailure,
    ScheduleFailureKind, ScheduleId, SessionRef,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug)]
pub enum ScheduleDestination {
    Existing {
        target: SessionRef,
    },
    Fresh {
        endpoint: EndpointRef,
    },
    Fork {
        source: SessionRef,
        through_turn_id: NonEmptyText,
    },
}

#[derive(Clone, Debug)]
pub struct FreshSessionRequest {
    pub endpoint: EndpointRef,
    pub working_directory: String,
    pub message: MessageText,
    pub inputs: CapturedRunInputs<SessionRef, EndpointRef>,
}

#[derive(Clone, Debug)]
pub struct PreparedTarget {
    pub target: SessionRef,
    pub evidence: RouteEffectEvidence<SessionRef, CodexGeneration>,
}

#[derive(Clone, Debug)]
pub struct SchedulePreparationRequest {
    pub operation_id: OperationId,
    pub schedule_id: ScheduleId,
    pub expected_change_id: ChangeId,
    pub destination: DestinationPreparation,
    pub instruction_text: String,
    pub model: Option<String>,
    pub effort: String,
}

#[derive(Clone, Debug)]
pub struct SchedulePreparationFailure {
    pub kind: ScheduleFailureKind,
    pub explanation: String,
    pub evidence: RouteEffectEvidence<SessionRef, CodexGeneration>,
    pub uncertain: bool,
}

#[derive(Clone, Debug)]
pub enum SchedulePreparationOutcome {
    Prepared(PreparedTarget),
    Failed(SchedulePreparationFailure),
}

pub trait PreparationEvidenceSink: Send + Sync {
    fn record_intent(
        &self,
        evidence: RouteEffectEvidence<SessionRef, CodexGeneration>,
    ) -> DeliveryFuture<'_, bool>;

    fn record_prepared<'a>(
        &'a self,
        prepared: &'a PreparedTarget,
    ) -> DeliveryFuture<'a, automation_storage::ScheduleInspection<SessionRef, EndpointRef>>;

    fn record_failure<'a>(
        &'a self,
        failure: &'a ScheduleFailure,
        evidence: RouteEffectEvidence<SessionRef, CodexGeneration>,
        uncertain: bool,
    ) -> DeliveryFuture<'a, ()>;
}

#[derive(Clone, Debug)]
pub struct ScheduledRunSubmission {
    pub run_id: RunId,
    pub target: SessionRef,
    pub message: MessageText,
    pub precondition: DeliveryPrecondition,
    pub inputs: CapturedRunInputs<SessionRef, EndpointRef>,
    pub recorded: RouteEffectEvidence<SessionRef, CodexGeneration>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunAcceptance {
    pub execution: RunExecution,
    pub receipt: DeliveryReceipt,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum RunSubmission {
    Started(Box<RunAcceptance>),
    NotStartedBusy,
    Rejected(DeliveryRejection),
    Unknown,
}

#[derive(Clone, Debug)]
pub struct RunObservationContext {
    pub run_id: RunId,
    pub phase: RunPhase,
    pub recorded: RouteEffectEvidence<SessionRef, CodexGeneration>,
    pub inputs: CapturedRunInputs<SessionRef, EndpointRef>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SettlementEvidence {
    TurnCompletion,
    OperationSettlement,
    WriteOnly,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ScheduleCapability {
    ReadTarget,
    StartRun,
    StopRun,
    CreateSession,
    ForkSession,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum ScheduleSupport {
    Supported { settlement: SettlementEvidence },
    Unsupported { missing: Vec<ScheduleCapability> },
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
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum RunSettlement {
    Pending,
    Completed {
        summary_source: Option<RunSummarySource>,
    },
    Failed {
        reason: String,
    },
    Interrupted,
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
    ) -> DeliveryFuture<'_, RunEvidenceDisposition>;
    fn record_stop_intent(&self) -> DeliveryFuture<'_, RunEvidenceDisposition>;
}

#[derive(Clone, Debug)]
pub enum RunEvidenceDisposition {
    Recorded {
        timing: Option<agent_automation::ExecutionTiming>,
    },
    AdmissionRefused,
}

pub trait ScheduledRunExecution: Send + Sync {
    fn support(&self, destination: &ScheduleDestination) -> DeliveryFuture<'_, ScheduleSupport>;
    fn initial_evidence(
        &self,
        destination: &ScheduleDestination,
    ) -> DeliveryFuture<'_, RouteEffectEvidence<SessionRef, CodexGeneration>>;
    fn prepare_destination<'a>(
        &'a self,
        request: SchedulePreparationRequest,
        sink: &'a dyn PreparationEvidenceSink,
    ) -> DeliveryFuture<'a, SchedulePreparationOutcome>;
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
        context: RunObservationContext,
    ) -> DeliveryFuture<'_, RunSettlement>;
    fn summary_source(
        &self,
        context: RunObservationContext,
    ) -> DeliveryFuture<'_, RunSummarySource>;
    fn request_stop<'a>(
        &'a self,
        context: RunObservationContext,
        sink: &'a dyn RunEvidenceSink,
    ) -> DeliveryFuture<'a, StopRequestOutcome>;
    fn reconcile_run(
        &self,
        context: RunObservationContext,
    ) -> DeliveryFuture<'_, RunReconciliation>;
}

pub trait ScheduledRunRoute: ScheduledRunExecution {
    fn supports_endpoint(&self, endpoint: &EndpointRef) -> bool;
}

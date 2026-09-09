//! Reusable scheduling and reminder rules, independent of storage and Host.
mod run_lifecycle;
pub use run_lifecycle::RunPhase;
mod automation_identity;
pub use automation_identity::{
    AttemptId, AutomationIdentityError, ChangeId, DeliveryId, EventId, InstructionId, OccurrenceId,
    OperationId, RevisionId, RunId, ScheduleId, SubscriptionId, ThreadBindingId, WakeupId,
};
mod instruction_document;
pub use instruction_document::{InstructionDocument, InstructionText, InstructionTextError};
mod timing_calculation;
pub use timing_calculation::{TimingError, TimingRule};
mod schedule_definition;
pub use schedule_definition::{
    CapturedRunInputs, ContinuityInput, ExecutionDestination, FrozenExecutionConfiguration,
    ScheduleDefinition, ScheduleRecord, SummarySource,
};
mod wakeup_definition;
pub use wakeup_definition::{
    DurableMessage, ExpiryRule, FirstFire, WakeDefinition, WakeRecord, WakeState,
};
mod delivery_state;
pub use delivery_state::DeliveryStatus;
mod native_effects;
pub use native_effects::{
    AttemptOutcome, CessationEvidence, DeliveryAttempt, NativeEffectEvidence, PreparationEffect,
    SubmissionEffect,
};
mod run_execution;
pub use run_execution::{ExecutionTiming, RunExecutionEvidence, RunRecord, WorkerOutcome};
mod summary_attempt;
pub use summary_attempt::{SummaryAttempt, SummaryPhase};

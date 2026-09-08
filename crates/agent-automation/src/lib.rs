//! Reusable scheduling and reminder rules, independent of storage and Host.
mod run_lifecycle;
pub use run_lifecycle::RunPhase;
mod automation_identity;
pub use automation_identity::{
    AttemptId, AutomationIdentityError, ChangeId, DeliveryId, EventId, InstructionId, OccurrenceId,
    OperationId, RevisionId, RunId, ScheduleId, ThreadBindingId, WakeupId,
};
mod instruction_document;
pub use instruction_document::{InstructionDocument, InstructionText, InstructionTextError};

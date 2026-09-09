//! SQLite persistence for local automation, independent of the Host executable.
mod automation_connection;
pub use automation_connection::{AutomationStore, StorageError};
mod instruction_repository;
mod instruction_revision_read;
mod instruction_updates;
pub use instruction_revision_read::InstructionRevisionRecord;
mod schema_initialization;
pub use instruction_updates::InstructionUpdate;
mod run_admission;
pub use run_admission::RunAdmission;
mod schedule_repository;
pub use schedule_repository::ScheduleCreate;
mod run_inventory;
mod schedule_evaluation;
mod wakeup_repository;
pub use wakeup_repository::WakeCreate;
mod local_operation_receipts;
mod wakeup_evaluation;
pub use wakeup_evaluation::WakeEvaluation;
mod wakeup_mutations;
pub use wakeup_mutations::{RetainedDelivery, WakeAction, WakeMutation, WakeMutationResult};
mod delivery_claims;
pub use delivery_claims::DeliveryClaim;
mod delivery_outcomes;
pub use delivery_outcomes::{DeliveryCompletion, DeliveryResult};
mod delivery_preparation;
mod delivery_recovery;
mod wakeup_worker_inventory;
pub use delivery_preparation::DeliveryPreparation;
mod delivery_inspection;
pub use delivery_inspection::DeliveryRecord;
mod wakeup_observation;
pub use wakeup_observation::WakeTransition;
mod wakeup_listing;
pub use wakeup_listing::{WakeListPage, WakeListPosition};
mod schedule_inspection;
pub use schedule_inspection::ScheduleInspection;
mod schedule_mutations;
pub use schedule_mutations::{ScheduleEdit, ScheduleMutation};
mod thread_binding_repository;
pub use thread_binding_repository::ThreadBindingClaim;
mod external_operation_receipts;
mod operation_receipt_read;
pub use external_operation_receipts::{
    ExternalAdmission, ExternalAdmissionResult, ExternalOperationRecord,
};
pub use operation_receipt_read::{StoredOperationRecord, StoredOperationState};

mod preparation_completion;
pub use preparation_completion::{PreparationIntent, PreparedThread};
mod preparation_failures;
pub use preparation_failures::{PreparationFailureDisposition, PreparationFailureRecord};
mod schedule_admission_queries;
pub use schedule_admission_queries::BindingAddress;
mod run_dispatch_state;
mod run_inspection;
pub use run_dispatch_state::RunDispatchIntent;
mod run_submission_outcomes;
pub use run_submission_outcomes::{RunSubmissionOutcome, RunSubmissionResult};
mod run_completion_state;
pub use run_completion_state::RunCompletion;

mod run_preparation_state;
pub use run_preparation_state::{RunPreparationIntent, RunPreparedTarget};
mod run_uncertainty_state;
pub use run_uncertainty_state::RunUncertainty;
mod run_stop_state;
mod schedule_worker_inventory;
mod summary_admission;
pub use summary_admission::SummaryAdmission;
mod summary_progress;
pub use summary_progress::SummaryProgress;
mod summary_completion;
pub use summary_completion::SummaryCompletion;
mod summary_recovery;
pub use summary_recovery::{SummaryRecoveryAction, SummaryRecoveryRequest};
mod configuration_receipts;
pub use configuration_receipts::{
    ConfigurationAdmission, ConfigurationOperation, ConfigurationProgress,
};
mod attempt_history_read;
mod automation_collection_listing;
mod automation_event_bounds;
mod automation_event_history;
pub use attempt_history_read::{
    AttemptCollection, AttemptHistoryPosition, AttemptHistoryQuery, AttemptHistoryRead,
    StoredAttemptPage, StoredAttemptRecord,
};
pub use automation_collection_listing::{
    AutomationCollection, AutomationKeyPage, AutomationListKey, AutomationListPosition,
};
pub use automation_event_history::{
    EventHistoryPage, EventHistoryQuery, EventHistoryRead, EventPosition, StoredAutomationEvent,
};
mod schedule_export;
mod schedule_import;
pub use schedule_import::ScheduleImport;

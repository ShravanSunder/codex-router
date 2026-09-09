//! SQLite persistence for local automation, independent of the Host executable.
mod automation_connection;
pub use automation_connection::{AutomationStore, StorageError};
mod instruction_repository;
mod instruction_updates;
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
pub use external_operation_receipts::{
    ExternalAdmission, ExternalAdmissionResult, ExternalOperationRecord,
};

mod preparation_completion;
pub use preparation_completion::{PreparationIntent, PreparedThread};
mod preparation_failures;
pub use preparation_failures::{PreparationFailureDisposition, PreparationFailureRecord};

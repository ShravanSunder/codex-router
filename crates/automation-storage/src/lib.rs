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

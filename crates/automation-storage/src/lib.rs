//! SQLite persistence for local automation, independent of the Host executable.
mod automation_connection;
pub use automation_connection::{AutomationStore, StorageError};
mod instruction_repository;
mod instruction_updates;
mod schema_initialization;
pub use instruction_updates::InstructionUpdate;

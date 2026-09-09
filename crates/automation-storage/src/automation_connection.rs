//! Database connection lifecycle; schema setup precedes every repository operation.
use sqlx::{
    Connection, SqliteConnection,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteSynchronous},
};
use std::{path::Path, time::Duration};

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("automation storage unavailable")]
    Database(#[from] sqlx::Error),
    #[error("automation database has unsupported schema version {0}")]
    UnsupportedVersion(i64),
    #[error("automation database contains an unexpected schema; existing data was not replaced")]
    InvalidSchema,
    #[error("operation identity already belongs to a different request")]
    OperationConflict,
    #[error("automation record is invalid or inconsistent")]
    InvalidRecord,
    #[error("event cursor must reference a retained sequence with a consistent observation time")]
    InvalidEventCursor,
    #[error(
        "attempt cursor must preserve its observation time, latest attempt and last emitted identity"
    )]
    InvalidAttemptCursor,
    #[error("workflow Run was not found in this automation database")]
    RunNotFound,
    #[error("delivery was not found in this automation database")]
    DeliveryNotFound,
    #[error("instruction was not found in this automation database")]
    InstructionNotFound,
    #[error("instruction changed; inspect the current revision before editing")]
    RevisionConflict,
    #[error("schedule was not found in this automation database")]
    ScheduleNotFound,
    #[error("schedule UUID already exists; pass --overwrite for an intentional replacement")]
    ScheduleImportExists,
    #[error(
        "imported instruction differs from existing shared instruction {instruction_id:?}; explicitly edit it or use a new instruction identity"
    )]
    InstructionImportConflict {
        instruction_id: agent_automation::InstructionId,
        schedule_ids: Vec<agent_automation::ScheduleId>,
    },
    #[error(transparent)]
    InvalidPackage(#[from] agent_automation::PackageError),
    #[error("schedule changed; inspect the latest change identity before editing")]
    ScheduleChangeConflict,
    #[error(
        "native endpoint or execution capabilities unavailable; inspect endpoint before enabling this schedule"
    )]
    ActivationUnavailable,
    #[error("native thread is already owned by schedule {schedule_id:?}")]
    ThreadOwnershipConflict {
        schedule_id: agent_automation::ScheduleId,
    },
    #[error("multiple conflicting Runs prevent safe admission")]
    RunInvariantConflict {
        run_ids: Vec<agent_automation::RunId>,
    },
    #[error("the previous Run has no required continuity summary; inspect summary recovery")]
    MissingContinuity,
    #[error("invalid schedule {field}: {reason}")]
    InvalidSchedule {
        field: &'static str,
        reason: &'static str,
    },
    #[error(transparent)]
    InvalidTiming(#[from] agent_automation::TimingError),
    #[error("wake-up was not found in this automation database")]
    WakeNotFound,
    #[error(
        "wake-up lifecycle does not permit this action in state {state:?}; inspect or create another reminder"
    )]
    WakeLifecycleConflict { state: agent_automation::WakeState },
    #[error("invalid wake-up {field}: {reason}")]
    InvalidWake {
        field: &'static str,
        reason: &'static str,
    },
}

pub struct AutomationStore {
    pub(crate) connection: SqliteConnection,
}
impl AutomationStore {
    pub async fn open(path: &Path) -> Result<Self, StorageError> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Delete)
            .synchronous(SqliteSynchronous::Full)
            .busy_timeout(Duration::from_secs(1));
        let mut connection = SqliteConnection::connect_with(&options).await?;
        crate::schema_initialization::initialize(&mut connection).await?;
        Ok(Self { connection })
    }
    pub async fn close(self) -> Result<(), StorageError> {
        self.connection.close().await?;
        Ok(())
    }
}

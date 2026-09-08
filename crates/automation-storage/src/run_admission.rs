//! Run admission is serialized independently of schedule configuration and timer cursors.
use crate::{AutomationStore, StorageError};
use agent_automation::{
    CapturedRunInputs, ChangeId, ContinuityInput, EventId, ExecutionDestination,
    FrozenExecutionConfiguration, InstructionText, RevisionId, RunId, ScheduleDefinition,
    ScheduleId, SummarySource,
};
use serde::{Serialize, de::DeserializeOwned};
use sqlx::{Connection, Row, SqliteConnection};

pub enum RunAdmission<TTarget, TEndpoint> {
    NoWaitingRun,
    Occupied {
        run_id: RunId,
    },
    Admitted {
        run_id: RunId,
        inputs: CapturedRunInputs<TTarget, TEndpoint>,
    },
}
impl AutomationStore {
    pub async fn admit_waiting_run<TTarget, TEndpoint>(
        &mut self,
        schedule_id: &ScheduleId,
        now_ms: i64,
    ) -> Result<RunAdmission<TTarget, TEndpoint>, StorageError>
    where
        TTarget: Serialize + DeserializeOwned,
        TEndpoint: Serialize + DeserializeOwned,
    {
        if now_ms < 0 {
            return Err(StorageError::InvalidRecord);
        }
        let mut transaction = self.connection.begin_with("BEGIN IMMEDIATE").await?;
        let schedule = sqlx::query("SELECT s.change_id,s.definition_json,s.imported_continuity_json,i.current_revision_id,i.instruction_text FROM schedule_definitions s JOIN instruction_documents i ON i.instruction_id=s.instruction_id WHERE s.schedule_id=?")
            .bind(schedule_id.as_str()).fetch_optional(&mut *transaction).await?.ok_or(StorageError::ScheduleNotFound)?;
        let inventory = crate::run_inventory::load(&mut transaction, schedule_id).await?;
        if let Some(run_id) = inventory.occupying {
            transaction.commit().await?;
            return Ok(RunAdmission::Occupied { run_id });
        }
        let Some(run_id) = inventory.waiting else {
            transaction.commit().await?;
            return Ok(RunAdmission::NoWaitingRun);
        };
        let definition: ScheduleDefinition<TTarget, TEndpoint> =
            serde_json::from_str(&schedule.try_get::<String, _>("definition_json")?)
                .map_err(|_| StorageError::InvalidRecord)?;
        if matches!(definition.destination, ExecutionDestination::Unprepared) {
            return Err(StorageError::InvalidRecord);
        }
        let require_summary = matches!(
            definition.destination,
            ExecutionDestination::FreshEachRun { .. }
        );
        let imported: ContinuityInput<TTarget> =
            serde_json::from_str(&schedule.try_get::<String, _>("imported_continuity_json")?)
                .map_err(|_| StorageError::InvalidRecord)?;
        if !matches!(
            imported,
            ContinuityInput::None | ContinuityInput::ImportedSummary { .. }
        ) {
            return Err(StorageError::InvalidRecord);
        }
        let continuity =
            select_continuity(&mut transaction, schedule_id, imported, require_summary).await?;
        let inputs = CapturedRunInputs {
            schedule_change_id: ChangeId::try_from(schedule.try_get::<String, _>("change_id")?)
                .map_err(|_| StorageError::InvalidRecord)?,
            instruction_revision_id: RevisionId::try_from(
                schedule.try_get::<String, _>("current_revision_id")?,
            )
            .map_err(|_| StorageError::InvalidRecord)?,
            instruction_text: InstructionText::try_from(
                schedule.try_get::<String, _>("instruction_text")?,
            )
            .map_err(|_| StorageError::InvalidRecord)?,
            continuity,
            execution_configuration: FrozenExecutionConfiguration {
                destination: definition.destination,
                execution_timeout_seconds: definition.execution_timeout_seconds,
            },
        };
        let captured = serde_json::to_string(&inputs).map_err(|_| StorageError::InvalidRecord)?;
        sqlx::query("UPDATE workflow_runs SET run_status='preparing',captured_inputs_json=? WHERE run_id=? AND run_status='waiting'")
            .bind(captured).bind(run_id.as_str()).execute(&mut *transaction).await?;
        sqlx::query("INSERT INTO automation_events(event_id,subject_kind,subject_id,event_kind,event_body_json,recorded_at_ms) VALUES (?,'run',?,'admitted',?,?)")
            .bind(EventId::generate().as_str()).bind(run_id.as_str())
            .bind(r#"{"kind":"stateChange","before":"waiting","after":"preparing"}"#)
            .bind(now_ms).execute(&mut *transaction).await?;
        transaction.commit().await?;
        Ok(RunAdmission::Admitted { run_id, inputs })
    }
}

async fn select_continuity<TTarget: DeserializeOwned>(
    connection: &mut SqliteConnection,
    schedule_id: &ScheduleId,
    imported: ContinuityInput<TTarget>,
    required: bool,
) -> Result<ContinuityInput<TTarget>, StorageError> {
    let prior = sqlx::query("SELECT run_id,summary_text,summary_source_json FROM workflow_runs WHERE schedule_id=? AND run_status='finished' ORDER BY completed_at_ms DESC,run_id DESC LIMIT 1")
        .bind(schedule_id.as_str()).fetch_optional(connection).await?;
    let Some(prior) = prior else {
        return Ok(imported);
    };
    let source: Option<String> = prior.try_get("summary_source_json")?;
    let Some(source) = source else {
        return if required {
            Err(StorageError::MissingContinuity)
        } else {
            Ok(ContinuityInput::None)
        };
    };
    match serde_json::from_str::<SummarySource<TTarget>>(&source)
        .map_err(|_| StorageError::InvalidRecord)?
    {
        SummarySource::Skipped { reason } => Ok(ContinuityInput::Omitted { reason }),
        SummarySource::Completed {
            source_target,
            source_turn_id,
            ..
        } => {
            let text: String = prior
                .try_get::<Option<String>, _>("summary_text")?
                .ok_or(StorageError::MissingContinuity)?;
            let source_run_id = RunId::try_from(prior.try_get::<String, _>("run_id")?)
                .map_err(|_| StorageError::InvalidRecord)?;
            Ok(ContinuityInput::LocalSummary {
                text,
                source_run_id,
                source_target,
                source_turn_id,
            })
        }
    }
}

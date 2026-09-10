//! One shared occupancy query prevents different admission paths classifying Runs differently.
use crate::StorageError;
use agent_automation::{RunId, RunPhase, ScheduleId};
use sqlx::{Row, SqliteConnection};
pub(crate) struct RunInventory {
    pub occupying: Option<RunId>,
    pub waiting: Option<RunId>,
}
pub(crate) async fn load(
    connection: &mut SqliteConnection,
    schedule_id: &ScheduleId,
) -> Result<RunInventory, StorageError> {
    let rows=sqlx::query("SELECT run_id,run_status FROM workflow_runs WHERE schedule_id=? AND run_status NOT IN ('finished','preparationFailed') ORDER BY due_at_ms,run_id")
        .bind(schedule_id.as_str()).fetch_all(connection).await?;
    let mut occupying = Vec::new();
    let mut waiting = Vec::new();
    for row in rows {
        let id = RunId::try_from(row.try_get::<String, _>("run_id")?)
            .map_err(|_| StorageError::InvalidRecord)?;
        let phase: RunPhase =
            serde_json::from_value(serde_json::Value::String(row.try_get("run_status")?))
                .map_err(|_| StorageError::InvalidRecord)?;
        if phase.occupies_execution() {
            occupying.push(id);
        } else if phase == RunPhase::Waiting {
            waiting.push(id);
        }
    }
    if occupying.len() > 1 || waiting.len() > 1 {
        occupying.extend(waiting);
        return Err(StorageError::RunInvariantConflict { run_ids: occupying });
    }
    Ok(RunInventory {
        occupying: occupying.pop(),
        waiting: waiting.pop(),
    })
}

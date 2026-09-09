//! Bounded scheduler inventories; Run admission and dispatch remain separately guarded transactions.
use crate::{AutomationStore, StorageError};
use agent_automation::{RunId, ScheduleId};
impl AutomationStore {
    pub async fn due_schedule_ids(&mut self, now_ms: i64) -> Result<Vec<ScheduleId>, StorageError> {
        let ids:Vec<String>=sqlx::query_scalar("SELECT s.schedule_id FROM schedule_definitions s JOIN schedule_timing_state t ON t.schedule_id=s.schedule_id WHERE s.enabled=1 AND t.next_due_at_ms<=? ORDER BY t.next_due_at_ms,s.schedule_id LIMIT 100").bind(now_ms).fetch_all(&mut self.connection).await?;
        ids.into_iter()
            .map(|id| id.try_into().map_err(|_| StorageError::InvalidRecord))
            .collect()
    }
    pub async fn waiting_schedule_ids(&mut self) -> Result<Vec<ScheduleId>, StorageError> {
        let ids:Vec<String>=sqlx::query_scalar("SELECT DISTINCT schedule_id FROM workflow_runs WHERE run_status='waiting' ORDER BY schedule_id LIMIT 100").fetch_all(&mut self.connection).await?;
        ids.into_iter()
            .map(|id| id.try_into().map_err(|_| StorageError::InvalidRecord))
            .collect()
    }
    pub async fn observable_run_ids(&mut self) -> Result<Vec<RunId>, StorageError> {
        let ids:Vec<String>=sqlx::query_scalar("SELECT run_id FROM workflow_runs WHERE run_status IN ('preparing','executing','stopping','summaryRequired','summaryRunning','summaryBlocked') OR (run_status='uncertain' AND native_turn_id IS NOT NULL) ORDER BY due_at_ms,run_id LIMIT 100").fetch_all(&mut self.connection).await?;
        ids.into_iter()
            .map(|id| id.try_into().map_err(|_| StorageError::InvalidRecord))
            .collect()
    }
}

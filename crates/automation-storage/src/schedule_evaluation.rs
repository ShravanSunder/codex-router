//! Due-work creation advances the timer watermark in the same transaction as waiting work.
use crate::{AutomationStore, StorageError};
use agent_automation::{EventId, RunId, ScheduleDefinition, ScheduleId};
use serde::{Serialize, de::DeserializeOwned};
use sqlx::{Connection, Row};

impl AutomationStore {
    pub async fn enqueue_due_run<TTarget, TEndpoint>(
        &mut self,
        schedule_id: &ScheduleId,
        now_ms: i64,
    ) -> Result<Option<RunId>, StorageError>
    where
        TTarget: Serialize + DeserializeOwned,
        TEndpoint: Serialize + DeserializeOwned,
    {
        if now_ms < 0 {
            return Err(StorageError::InvalidRecord);
        }
        let now = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(now_ms)
            .ok_or(StorageError::InvalidRecord)?;
        let mut transaction = self.connection.begin_with("BEGIN IMMEDIATE").await?;
        let row=sqlx::query("SELECT s.enabled,s.change_id,s.definition_json,t.applied_change_id,t.anchor_at_ms,t.next_due_at_ms FROM schedule_definitions s JOIN schedule_timing_state t ON t.schedule_id=s.schedule_id WHERE s.schedule_id=?")
            .bind(schedule_id.as_str()).fetch_optional(&mut *transaction).await?.ok_or(StorageError::ScheduleNotFound)?;
        if row.try_get::<String, _>("change_id")?
            != row.try_get::<String, _>("applied_change_id")?
        {
            return Err(StorageError::InvalidRecord);
        }
        let due: Option<i64> = row.try_get("next_due_at_ms")?;
        let Some(due) = due.filter(|due| *due <= now_ms) else {
            transaction.commit().await?;
            return Ok(None);
        };
        if !row.try_get::<bool, _>("enabled")? {
            transaction.commit().await?;
            return Ok(None);
        }
        let definition: ScheduleDefinition<TTarget, TEndpoint> =
            serde_json::from_str(&row.try_get::<String, _>("definition_json")?)
                .map_err(|_| StorageError::InvalidRecord)?;
        let anchor =
            chrono::DateTime::<chrono::Utc>::from_timestamp_millis(row.try_get("anchor_at_ms")?)
                .ok_or(StorageError::InvalidRecord)?;
        let next = definition
            .timing
            .next_due(anchor, Some(now))?
            .map(|value| value.timestamp_millis());
        let inventory = crate::run_inventory::load(&mut transaction, schedule_id).await?;
        let (run_id, created) = match inventory.waiting {
            Some(id) => (id, false),
            None => (RunId::generate(), true),
        };
        if created {
            // No native effect exists yet. This is the closed empty RunExecutionEvidence shape.
            let evidence = serde_json::json!({"native":{"target":null,"generation":null,"clientUserMessageId":null,"nativeTurnId":null,"nativeSubmissionId":null,"allocation":"notRequested","resume":"notRequested","submission":"notDispatched","cessation":"notApplicable"},"timing":null,"acceptance":null});
            sqlx::query("INSERT INTO workflow_runs(run_id,schedule_id,due_at_ms,run_status,execution_evidence_json) VALUES (?,?,?,'waiting',?)")
                .bind(run_id.as_str()).bind(schedule_id.as_str()).bind(due).bind(evidence.to_string()).execute(&mut *transaction).await?;
        }
        sqlx::query("UPDATE schedule_timing_state SET evaluated_through_ms=?,next_due_at_ms=? WHERE schedule_id=?")
            .bind(now_ms).bind(next).bind(schedule_id.as_str()).execute(&mut *transaction).await?;
        let event = serde_json::json!({"kind":"stateChange","before":if created {None}else{Some("waiting")},"after":"waiting"});
        sqlx::query("INSERT INTO automation_events(event_id,subject_kind,subject_id,event_kind,event_body_json,recorded_at_ms) VALUES (?,'run',?,?,?,?)")
            .bind(EventId::generate().as_str()).bind(run_id.as_str()).bind(if created {"queued"}else{"coalesced"}).bind(event.to_string()).bind(now_ms).execute(&mut *transaction).await?;
        transaction.commit().await?;
        Ok(Some(run_id))
    }
}

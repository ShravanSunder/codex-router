//! Ordered committed transitions let a waiter observe pause even after immediate resume.
use crate::{AutomationStore, StorageError};
use agent_automation::WakeupId;
use sqlx::Row;
#[derive(Clone, Debug)]
pub struct WakeTransition {
    pub sequence: i64,
    pub recorded_at_ms: i64,
    pub kind: String,
    pub body: serde_json::Value,
}
impl AutomationStore {
    pub async fn wake_transitions_after(
        &mut self,
        id: &WakeupId,
        sequence: i64,
    ) -> Result<Vec<WakeTransition>, StorageError> {
        if sequence < 0 {
            return Err(StorageError::InvalidRecord);
        }
        let rows=sqlx::query("SELECT event_sequence,recorded_at_ms,event_kind,event_body_json FROM automation_events WHERE subject_kind='wake' AND subject_id=? AND event_sequence>? ORDER BY event_sequence LIMIT 100")
            .bind(id.as_str()).bind(sequence).fetch_all(&mut self.connection).await?;
        rows.into_iter()
            .map(|row| {
                Ok(WakeTransition {
                    sequence: row.try_get("event_sequence")?,
                    recorded_at_ms: row.try_get("recorded_at_ms")?,
                    kind: row.try_get("event_kind")?,
                    body: serde_json::from_str(&row.try_get::<String, _>("event_body_json")?)
                        .map_err(|_| StorageError::InvalidRecord)?,
                })
            })
            .collect()
    }
}

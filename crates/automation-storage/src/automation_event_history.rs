//! Event navigation validates retained boundaries independently of physical cleanup timing.
use crate::{AutomationStore, StorageError};
use sqlx::{Connection, Row};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventPosition {
    pub sequence: i64,
    pub observed_at_ms: i64,
}
pub struct EventHistoryQuery {
    pub after: Option<EventPosition>,
    pub now_ms: i64,
    pub limit: u32,
}
pub struct StoredAutomationEvent {
    pub event_id: agent_automation::EventId,
    pub sequence: i64,
    pub subject_kind: String,
    pub subject_id: String,
    pub event_kind: String,
    pub body: serde_json::Value,
    pub recorded_at_ms: i64,
}
pub struct EventHistoryPage {
    pub records: Vec<StoredAutomationEvent>,
    pub next: EventPosition,
    pub earliest: EventPosition,
}
pub enum EventHistoryRead {
    Page(EventHistoryPage),
    Expired { earliest: EventPosition },
}
impl AutomationStore {
    pub async fn read_event_history(
        &mut self,
        request: &EventHistoryQuery,
    ) -> Result<EventHistoryRead, StorageError> {
        if !(1..=100).contains(&request.limit) || request.now_ms < 0 {
            return Err(StorageError::InvalidEventCursor);
        }
        let cutoff = crate::automation_event_bounds::retention_cutoff(request.now_ms)?;
        let mut transaction = self.connection.begin().await?;
        let maximum: i64 = sqlx::query_scalar(
            "SELECT COALESCE((SELECT seq FROM sqlite_sequence WHERE name='automation_events'),0)",
        )
        .fetch_one(&mut *transaction)
        .await?;
        let floor: Option<(i64,i64)> = sqlx::query_as("SELECT event_sequence,recorded_at_ms FROM automation_events WHERE recorded_at_ms>=? ORDER BY event_sequence LIMIT 1").bind(cutoff).fetch_optional(&mut *transaction).await?;
        let earliest = floor.map_or(
            EventPosition {
                sequence: maximum,
                observed_at_ms: request.now_ms,
            },
            |(sequence, recorded)| EventPosition {
                sequence: sequence - 1,
                observed_at_ms: recorded,
            },
        );
        let position = request.after.as_ref().unwrap_or(&earliest);
        if position.sequence < 0
            || position.sequence > maximum
            || position.observed_at_ms > request.now_ms
        {
            return Err(StorageError::InvalidEventCursor);
        }
        if position.observed_at_ms < cutoff || position.sequence < earliest.sequence {
            transaction.commit().await?;
            return Ok(EventHistoryRead::Expired { earliest });
        }
        if request.after.is_some() && *position != earliest {
            let recorded: Option<i64> = sqlx::query_scalar(
                "SELECT recorded_at_ms FROM automation_events WHERE event_sequence=?",
            )
            .bind(position.sequence)
            .fetch_optional(&mut *transaction)
            .await?;
            let next_recorded: Option<i64> = sqlx::query_scalar("SELECT recorded_at_ms FROM automation_events WHERE event_sequence>? ORDER BY event_sequence LIMIT 1").bind(position.sequence).fetch_optional(&mut *transaction).await?;
            if recorded.is_none() && position.sequence != maximum {
                transaction.commit().await?;
                return Ok(EventHistoryRead::Expired { earliest });
            }
            // Exact event cursors and snapshots taken before the next committed event are both valid.
            if recorded.is_some_and(|recorded| position.observed_at_ms < recorded)
                || (position.sequence != maximum
                    && recorded != Some(position.observed_at_ms)
                    && next_recorded.is_some_and(|next| position.observed_at_ms > next))
            {
                return Err(StorageError::InvalidEventCursor);
            }
        }
        let rows = sqlx::query("SELECT * FROM automation_events WHERE event_sequence>? AND recorded_at_ms>=? ORDER BY event_sequence LIMIT ?")
            .bind(position.sequence).bind(cutoff).bind(request.limit).fetch_all(&mut *transaction).await?;
        let records = rows
            .into_iter()
            .map(|row| {
                Ok(StoredAutomationEvent {
                    event_id: row
                        .try_get::<String, _>("event_id")?
                        .try_into()
                        .map_err(|_| StorageError::InvalidRecord)?,
                    sequence: row.try_get("event_sequence")?,
                    subject_kind: row.try_get("subject_kind")?,
                    subject_id: row.try_get("subject_id")?,
                    event_kind: row.try_get("event_kind")?,
                    recorded_at_ms: row.try_get("recorded_at_ms")?,
                    body: serde_json::from_str(&row.try_get::<String, _>("event_body_json")?)
                        .map_err(|_| StorageError::InvalidRecord)?,
                })
            })
            .collect::<Result<Vec<_>, StorageError>>()?;
        let next = records.last().map_or_else(
            || position.clone(),
            |event| EventPosition {
                sequence: event.sequence,
                observed_at_ms: event.recorded_at_ms,
            },
        );
        let next = if next.sequence == maximum {
            EventPosition {
                sequence: maximum,
                observed_at_ms: request.now_ms,
            }
        } else {
            next
        };
        transaction.commit().await?;
        Ok(EventHistoryRead::Page(EventHistoryPage {
            records,
            next,
            earliest,
        }))
    }
}

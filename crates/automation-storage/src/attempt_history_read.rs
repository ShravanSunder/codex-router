//! Retained prior attempts merge with a pinned latest attempt; owner evidence outlives events.
use crate::{AutomationStore, StorageError};
use agent_automation::{AttemptId, DeliveryId, RunId, RunPhase};
use sqlx::{Connection, Row, SqliteConnection};

pub enum AttemptCollection {
    Delivery(DeliveryId),
    Summary(RunId),
}
#[derive(Clone)]
pub struct AttemptHistoryPosition {
    pub as_of_ms: i64,
    pub latest_attempt_id: AttemptId,
    pub last_started_at_ms: i64,
    pub last_attempt_id: AttemptId,
}
pub struct AttemptHistoryQuery {
    pub collection: AttemptCollection,
    pub position: Option<AttemptHistoryPosition>,
    pub now_ms: i64,
    pub limit: u32,
}
pub struct StoredAttemptRecord {
    pub attempt_id: AttemptId,
    pub started_at_ms: i64,
    pub body: serde_json::Value,
}
pub struct StoredAttemptPage {
    pub records: Vec<StoredAttemptRecord>,
    pub as_of_ms: i64,
    pub history_from_ms: i64,
    pub latest_attempt_id: Option<AttemptId>,
    pub current_attempt_id: Option<AttemptId>,
    pub owner_phase: Option<RunPhase>,
    pub has_more: bool,
}
pub enum AttemptHistoryRead {
    Page(StoredAttemptPage),
    Expired,
}
impl AutomationStore {
    pub async fn read_attempt_history(
        &mut self,
        request: &AttemptHistoryQuery,
    ) -> Result<AttemptHistoryRead, StorageError> {
        if !(1..=100).contains(&request.limit) || request.now_ms < 0 {
            return Err(StorageError::InvalidAttemptCursor);
        }
        let as_of = request
            .position
            .as_ref()
            .map_or(request.now_ms, |position| position.as_of_ms);
        if as_of < 0 || as_of > request.now_ms {
            return Err(StorageError::InvalidAttemptCursor);
        }
        let current_cutoff = crate::automation_event_bounds::retention_cutoff(request.now_ms)?;
        if as_of < current_cutoff {
            return Ok(AttemptHistoryRead::Expired);
        }
        let history_from = crate::automation_event_bounds::retention_cutoff(as_of)?;
        let (kind, id, sql, missing) = match &request.collection {
            AttemptCollection::Delivery(id) => (
                "delivery",
                id.as_str(),
                "SELECT latest_attempt_json AS attempt_json,NULL AS owner_phase FROM mailbox_deliveries WHERE delivery_id=?",
                StorageError::DeliveryNotFound,
            ),
            AttemptCollection::Summary(id) => (
                "run",
                id.as_str(),
                "SELECT summary_attempt_json AS attempt_json,run_status AS owner_phase FROM workflow_runs WHERE run_id=?",
                StorageError::RunNotFound,
            ),
        };
        let mut transaction = self.connection.begin().await?;
        let owner = sqlx::query(sql)
            .bind(id)
            .fetch_optional(&mut *transaction)
            .await?
            .ok_or(missing)?;
        let current = owner
            .try_get::<Option<String>, _>("attempt_json")?
            .map(decode_attempt)
            .transpose()?;
        let current_id = current.as_ref().map(|attempt| attempt.attempt_id.clone());
        let owner_phase = owner
            .try_get::<Option<String>, _>("owner_phase")?
            .map(|phase| {
                serde_json::from_value(serde_json::Value::String(phase))
                    .map_err(|_| StorageError::InvalidRecord)
            })
            .transpose()?;
        let pinned_id = request
            .position
            .as_ref()
            .map(|position| position.latest_attempt_id.clone())
            .or_else(|| current_id.clone());
        let Some(pinned_id) = pinned_id else {
            transaction.commit().await?;
            return Ok(AttemptHistoryRead::Page(StoredAttemptPage {
                records: Vec::new(),
                as_of_ms: as_of,
                history_from_ms: history_from,
                latest_attempt_id: None,
                current_attempt_id: current_id,
                owner_phase,
                has_more: false,
            }));
        };
        let pinned = match current {
            Some(current) if current.attempt_id == pinned_id => Some(current),
            _ => {
                find_retained_attempt(&mut transaction, kind, id, &pinned_id, current_cutoff)
                    .await?
            }
        };
        let Some(pinned) = pinned else {
            transaction.commit().await?;
            return Ok(AttemptHistoryRead::Expired);
        };
        if let Some(position) = &request.position {
            if position.last_started_at_ms < 0
                || (position.last_started_at_ms, &position.last_attempt_id) >= (as_of, &pinned_id)
            {
                return Err(StorageError::InvalidAttemptCursor);
            }
            let last = if position.last_attempt_id == pinned_id {
                Some(StoredAttemptRecord {
                    attempt_id: pinned.attempt_id.clone(),
                    started_at_ms: pinned.started_at_ms,
                    body: pinned.body.clone(),
                })
            } else {
                find_retained_attempt(
                    &mut transaction,
                    kind,
                    id,
                    &position.last_attempt_id,
                    current_cutoff,
                )
                .await?
            };
            match last {
                Some(last) if last.started_at_ms == position.last_started_at_ms => {}
                Some(_) => return Err(StorageError::InvalidAttemptCursor),
                None => {
                    transaction.commit().await?;
                    return Ok(AttemptHistoryRead::Expired);
                }
            }
        }
        let last_started = request
            .position
            .as_ref()
            .map(|position| position.last_started_at_ms);
        let last_id = request
            .position
            .as_ref()
            .map(|position| position.last_attempt_id.as_str());
        let rows = sqlx::query(
            "WITH candidates AS (
                SELECT event_sequence,json_extract(event_body_json,'$.attempt') AS attempt_json,
                    json_extract(event_body_json,'$.attempt.attemptId') AS attempt_id,
                    json_extract(event_body_json,'$.attempt.startedAtMs') AS started_at_ms
                FROM automation_events WHERE subject_kind=? AND subject_id=? AND recorded_at_ms>=? AND recorded_at_ms<=?
                    AND event_kind IN ('attemptArchived','attemptCompleted','dispatchInterrupted','summaryAttemptArchived')
            ), versions AS (SELECT attempt_id,MAX(event_sequence) AS event_sequence FROM candidates GROUP BY attempt_id)
            SELECT c.attempt_json FROM candidates c JOIN versions v ON c.event_sequence=v.event_sequence
                WHERE c.attempt_id<>? AND (c.started_at_ms,c.attempt_id)<=(?,?)
                    AND (? IS NULL OR (c.started_at_ms,c.attempt_id)>(?,?))
                ORDER BY c.started_at_ms,c.attempt_id LIMIT ?")
            .bind(kind).bind(id).bind(history_from.max(current_cutoff)).bind(as_of)
            .bind(pinned_id.as_str()).bind(as_of).bind(pinned_id.as_str())
            .bind(last_started).bind(last_started).bind(last_id).bind(i64::from(request.limit)+1)
            .fetch_all(&mut *transaction).await?;
        let mut records = rows
            .into_iter()
            .map(|row| decode_attempt(row.try_get("attempt_json")?))
            .collect::<Result<Vec<_>, StorageError>>()?;
        if request.position.as_ref().is_none_or(|position| {
            (pinned.started_at_ms, &pinned.attempt_id)
                > (position.last_started_at_ms, &position.last_attempt_id)
        }) {
            records.push(pinned);
        }
        records.sort_by(|left, right| {
            (left.started_at_ms, &left.attempt_id).cmp(&(right.started_at_ms, &right.attempt_id))
        });
        let limit = usize::try_from(request.limit).map_err(|_| StorageError::InvalidRecord)?;
        let has_more = records.len() > limit;
        records.truncate(limit);
        transaction.commit().await?;
        Ok(AttemptHistoryRead::Page(StoredAttemptPage {
            records,
            as_of_ms: as_of,
            history_from_ms: history_from,
            latest_attempt_id: Some(pinned_id),
            current_attempt_id: current_id,
            owner_phase,
            has_more,
        }))
    }
}
async fn find_retained_attempt(
    connection: &mut SqliteConnection,
    kind: &str,
    id: &str,
    attempt_id: &AttemptId,
    cutoff: i64,
) -> Result<Option<StoredAttemptRecord>, StorageError> {
    let body: Option<String> = sqlx::query_scalar("SELECT json_extract(event_body_json,'$.attempt') FROM automation_events WHERE subject_kind=? AND subject_id=? AND recorded_at_ms>=? AND json_extract(event_body_json,'$.attempt.attemptId')=? ORDER BY event_sequence DESC LIMIT 1")
        .bind(kind).bind(id).bind(cutoff).bind(attempt_id.as_str()).fetch_optional(connection).await?;
    body.map(decode_attempt).transpose()
}
fn decode_attempt(body: String) -> Result<StoredAttemptRecord, StorageError> {
    let body: serde_json::Value =
        serde_json::from_str(&body).map_err(|_| StorageError::InvalidRecord)?;
    let attempt_id = body
        .get("attemptId")
        .and_then(serde_json::Value::as_str)
        .ok_or(StorageError::InvalidRecord)?
        .to_owned()
        .try_into()
        .map_err(|_| StorageError::InvalidRecord)?;
    let started_at_ms = body
        .get("startedAtMs")
        .and_then(serde_json::Value::as_i64)
        .filter(|time| *time >= 0)
        .ok_or(StorageError::InvalidRecord)?;
    Ok(StoredAttemptRecord {
        attempt_id,
        started_at_ms,
        body,
    })
}

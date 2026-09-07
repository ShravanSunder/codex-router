//! Consistent bounds and endpoint-filtered cursor scans, independent of live subscriptions.
use crate::{JournalError, JournalPosition, ObservationJournal};
use communication_protocol::{EndpointRef, LifecycleObservation};
pub use communication_protocol::{JournalBounds, JournalPage, LifecycleRecord};
use sqlx::{Connection, Row};
impl ObservationJournal {
    pub async fn bounds(&mut self) -> Result<JournalBounds, JournalError> {
        let row=sqlx::query("SELECT last_sequence,(SELECT sequence FROM journal_checkpoint WHERE singleton=1) AS checkpoint FROM journal_metadata WHERE singleton=1").fetch_one(&mut self.connection).await?;
        Ok(JournalBounds {
            journal_id: self.journal_id.clone(),
            earliest_sequence: u64::try_from(row.try_get::<i64, _>("checkpoint")?)
                .map_err(|_| JournalError::InvalidStorage)?
                + 1,
            last_sequence: u64::try_from(row.try_get::<i64, _>("last_sequence")?)
                .map_err(|_| JournalError::InvalidStorage)?,
        })
    }

    pub async fn read_page(
        &mut self,
        endpoint: &EndpointRef,
        after: JournalPosition,
        page_size: u32,
    ) -> Result<JournalPage, JournalError> {
        if !(1..=100).contains(&page_size) || after.sequence > 9_007_199_254_740_991 {
            return Err(JournalError::InvalidRecord);
        }
        if after.journal_id != self.journal_id {
            return Err(JournalError::JournalChanged);
        }
        let mut transaction = self.connection.begin().await?;
        let state=sqlx::query("SELECT last_sequence,(SELECT sequence FROM journal_checkpoint WHERE singleton=1) AS checkpoint FROM journal_metadata WHERE singleton=1").fetch_one(&mut *transaction).await?;
        let last = u64::try_from(state.try_get::<i64, _>("last_sequence")?)
            .map_err(|_| JournalError::InvalidStorage)?;
        let checkpoint = u64::try_from(state.try_get::<i64, _>("checkpoint")?)
            .map_err(|_| JournalError::InvalidStorage)?;
        if after.sequence < checkpoint {
            return Err(JournalError::HistoryExpired);
        }
        if after.sequence > last {
            return Err(JournalError::InvalidRecord);
        }
        let bounds = JournalBounds {
            journal_id: self.journal_id.clone(),
            earliest_sequence: checkpoint + 1,
            last_sequence: last,
        };
        let rows=sqlx::query("SELECT sequence,observation_json FROM lifecycle_records WHERE sequence>? ORDER BY sequence LIMIT 1000").bind(i64::try_from(after.sequence).map_err(|_|JournalError::InvalidRecord)?).fetch_all(&mut *transaction).await?;
        let mut next = after;
        let mut records = Vec::new();
        let mut bytes = 0_usize;
        for row in rows {
            let sequence = u64::try_from(row.try_get::<i64, _>("sequence")?)
                .map_err(|_| JournalError::InvalidStorage)?;
            if sequence != next.sequence + 1 {
                return Err(JournalError::InvalidStorage);
            }
            let observation: LifecycleObservation =
                serde_json::from_str(&row.try_get::<String, _>("observation_json")?)
                    .map_err(|_| JournalError::InvalidStorage)?;
            observation
                .validate()
                .map_err(|_| JournalError::InvalidStorage)?;
            if observation.scope.endpoint == *endpoint {
                let record = LifecycleRecord {
                    journal_id: self.journal_id.clone(),
                    sequence,
                    schema_version: 1,
                    observation,
                };
                let size = serde_json::to_vec(&record)
                    .map_err(|_| JournalError::InvalidStorage)?
                    .len();
                // Leave room for response envelope, bounds and cursor.
                if size > 1024 * 1024 - 4096 {
                    return Err(JournalError::Capacity);
                }
                if bytes + size > 1024 * 1024 - 4096 {
                    break;
                }
                bytes += size;
                records.push(record);
            }
            next.sequence = sequence;
            if records.len() >= page_size as usize {
                break;
            }
        }
        transaction.commit().await?;
        Ok(JournalPage {
            caught_up: next.sequence == last,
            bounds,
            records,
            next,
        })
    }
}

//! Checkpoint-before-prefix expiry, atomically preserving remembered addresses.
use crate::{JournalError, JournalPosition, ObservationJournal};
use communication_protocol::{LifecycleObservation, LifecycleSubject};
use sqlx::{Connection, Row};

impl ObservationJournal {
    /// Applies rolling 30-day expiry using Unix seconds and the persisted logical clock.
    /// Returns the checkpoint sequence, including when no additional records expire.
    pub async fn expire_history(&mut self, now: i64) -> Result<u64, JournalError> {
        if now < 0 {
            return Err(JournalError::InvalidRecord);
        }
        let mut transaction = self.connection.begin_with("BEGIN IMMEDIATE").await?;
        let clock: i64 =
            sqlx::query_scalar("SELECT retention_clock FROM journal_metadata WHERE singleton=1")
                .fetch_one(&mut *transaction)
                .await?;
        let clock = clock.max(now);
        let cutoff = clock.saturating_sub(30 * 86400);
        let old: i64 =
            sqlx::query_scalar("SELECT sequence FROM journal_checkpoint WHERE singleton=1")
                .fetch_one(&mut *transaction)
                .await?;
        let eligible: Option<i64> = sqlx::query_scalar(
            "SELECT MAX(sequence) FROM lifecycle_records WHERE retention_at < ?",
        )
        .bind(cutoff)
        .fetch_one(&mut *transaction)
        .await?;
        let end = eligible.unwrap_or(old);
        let mut cursor = old;
        while cursor < end {
            let records=sqlx::query("SELECT sequence,observation_json FROM lifecycle_records WHERE sequence>? AND sequence<=? ORDER BY sequence LIMIT 100").bind(cursor).bind(end).fetch_all(&mut *transaction).await?;
            if records.is_empty() {
                return Err(JournalError::InvalidStorage);
            }
            for row in records {
                let sequence: i64 = row.try_get("sequence")?;
                if sequence != cursor + 1 {
                    return Err(JournalError::InvalidStorage);
                }
                let text: String = row.try_get("observation_json")?;
                let observation: LifecycleObservation =
                    serde_json::from_str(&text).map_err(|_| JournalError::InvalidStorage)?;
                observation
                    .validate()
                    .map_err(|_| JournalError::InvalidStorage)?;
                if let LifecycleSubject::Thread { address } = &observation.subject {
                    let key =
                        serde_json::to_string(address).map_err(|_| JournalError::InvalidStorage)?;
                    let prior: Option<String> = sqlx::query_scalar(
                        "SELECT entry_json FROM checkpoint_addresses WHERE address_key=?",
                    )
                    .bind(&key)
                    .fetch_optional(&mut *transaction)
                    .await?;
                    let prior = prior
                        .map(|value| {
                            serde_json::from_str(&value).map_err(|_| JournalError::InvalidStorage)
                        })
                        .transpose()?;
                    let entry = crate::thread_address_book::reduce_address_observation(
                        prior,
                        &observation,
                        JournalPosition {
                            journal_id: self.journal_id.clone(),
                            sequence: u64::try_from(sequence)
                                .map_err(|_| JournalError::InvalidStorage)?,
                        },
                    )?;
                    let entry =
                        serde_json::to_string(&entry).map_err(|_| JournalError::InvalidStorage)?;
                    sqlx::query("INSERT INTO checkpoint_addresses VALUES (?,?) ON CONFLICT(address_key) DO UPDATE SET entry_json=excluded.entry_json").bind(key).bind(entry).execute(&mut *transaction).await?;
                }
                cursor = sequence;
            }
        }
        let removed:i64=sqlx::query_scalar("SELECT COALESCE(SUM(length(CAST(observation_json AS BLOB))),0) FROM lifecycle_records WHERE sequence<=?").bind(end).fetch_one(&mut *transaction).await?;
        sqlx::query("UPDATE journal_checkpoint SET sequence=? WHERE singleton=1")
            .bind(end)
            .execute(&mut *transaction)
            .await?;
        sqlx::query("DELETE FROM lifecycle_records WHERE sequence<=?")
            .bind(end)
            .execute(&mut *transaction)
            .await?;
        sqlx::query("UPDATE journal_metadata SET retention_clock=?,payload_bytes=payload_bytes-? WHERE singleton=1").bind(clock).bind(removed).execute(&mut *transaction).await?;
        transaction.commit().await?;
        u64::try_from(end).map_err(|_| JournalError::InvalidStorage)
    }
}

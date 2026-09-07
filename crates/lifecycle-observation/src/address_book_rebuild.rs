//! Transactional reconstruction from checkpoint plus retained observations.
use crate::{AddressEntry, JournalError, JournalPosition, ObservationJournal};
use communication_protocol::{LifecycleObservation, LifecycleSubject};
use sqlx::{Connection, Row};

impl ObservationJournal {
    /// Restores historical projection only; this never establishes live observer coverage.
    pub async fn rebuild_addresses(&mut self) -> Result<(), JournalError> {
        let mut transaction = self.connection.begin_with("BEGIN IMMEDIATE").await?;
        let checkpoint: i64 =
            sqlx::query_scalar("SELECT sequence FROM journal_checkpoint WHERE singleton=1")
                .fetch_one(&mut *transaction)
                .await?;
        let last: i64 =
            sqlx::query_scalar("SELECT last_sequence FROM journal_metadata WHERE singleton=1")
                .fetch_one(&mut *transaction)
                .await?;
        if checkpoint < 0 || checkpoint > last {
            return Err(JournalError::InvalidStorage);
        }
        sqlx::query("DELETE FROM thread_addresses")
            .execute(&mut *transaction)
            .await?;
        let mut key = String::new();
        let mut total = 0_i64;
        loop {
            let rows=sqlx::query("SELECT address_key,entry_json FROM checkpoint_addresses WHERE address_key>? ORDER BY address_key LIMIT 100").bind(&key).fetch_all(&mut *transaction).await?;
            if rows.is_empty() {
                break;
            }
            for row in rows {
                key = row.try_get("address_key")?;
                let text: String = row.try_get("entry_json")?;
                let entry: AddressEntry =
                    serde_json::from_str(&text).map_err(|_| JournalError::InvalidStorage)?;
                if entry.last_observation.journal_id != self.journal_id
                    || entry.last_observation.sequence
                        > u64::try_from(checkpoint).map_err(|_| JournalError::InvalidStorage)?
                    || serde_json::to_string(&entry.address)
                        .map_err(|_| JournalError::InvalidStorage)?
                        != key
                {
                    return Err(JournalError::InvalidStorage);
                }
                let size = i64::try_from(text.len()).map_err(|_| JournalError::Capacity)?;
                total = total
                    .checked_add(size)
                    .filter(|v| *v <= 64 * 1024 * 1024)
                    .ok_or(JournalError::Capacity)?;
                sqlx::query("INSERT INTO thread_addresses VALUES (?,?,?)")
                    .bind(&key)
                    .bind(text)
                    .bind(size)
                    .execute(&mut *transaction)
                    .await?;
            }
        }
        let mut cursor = checkpoint;
        while cursor < last {
            let rows=sqlx::query("SELECT sequence,observation_json FROM lifecycle_records WHERE sequence>? ORDER BY sequence LIMIT 100").bind(cursor).fetch_all(&mut *transaction).await?;
            if rows.is_empty() {
                return Err(JournalError::InvalidStorage);
            }
            for row in rows {
                let sequence: i64 = row.try_get("sequence")?;
                if sequence != cursor + 1 || sequence > last {
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
                    let prior = sqlx::query(
                        "SELECT entry_json,payload_bytes FROM thread_addresses WHERE address_key=?",
                    )
                    .bind(&key)
                    .fetch_optional(&mut *transaction)
                    .await?;
                    let prior_size = prior
                        .as_ref()
                        .map(|r| r.try_get::<i64, _>("payload_bytes"))
                        .transpose()?
                        .unwrap_or(0);
                    let prior = prior
                        .map(|r| r.try_get::<String, _>("entry_json"))
                        .transpose()?
                        .map(|text| {
                            serde_json::from_str(&text).map_err(|_| JournalError::InvalidStorage)
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
                    let text =
                        serde_json::to_string(&entry).map_err(|_| JournalError::InvalidStorage)?;
                    let size = i64::try_from(text.len()).map_err(|_| JournalError::Capacity)?;
                    total = total
                        .checked_sub(prior_size)
                        .and_then(|v| v.checked_add(size))
                        .filter(|v| *v <= 64 * 1024 * 1024)
                        .ok_or(JournalError::Capacity)?;
                    sqlx::query("INSERT INTO thread_addresses VALUES (?,?,?) ON CONFLICT(address_key) DO UPDATE SET entry_json=excluded.entry_json,payload_bytes=excluded.payload_bytes").bind(key).bind(text).bind(size).execute(&mut *transaction).await?;
                }
                cursor = sequence;
            }
        }
        transaction.commit().await?;
        Ok(())
    }
}

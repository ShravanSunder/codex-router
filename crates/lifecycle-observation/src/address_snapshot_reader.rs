//! Immutable historical address snapshot captured with its exact journal watermark.
use crate::{AddressEntry, JournalError, JournalPosition, ObservationJournal};
use communication_protocol::EndpointRef;
use sqlx::{Connection, Row};

pub struct CapturedAddresses {
    pub watermark: JournalPosition,
    pub entries: Vec<AddressEntry>,
    pub serialized_bytes: usize,
}
impl ObservationJournal {
    pub async fn capture_addresses(
        &mut self,
        endpoint: &EndpointRef,
    ) -> Result<CapturedAddresses, JournalError> {
        let mut transaction = self.connection.begin().await?;
        let last: i64 =
            sqlx::query_scalar("SELECT last_sequence FROM journal_metadata WHERE singleton=1")
                .fetch_one(&mut *transaction)
                .await?;
        let mut entries = Vec::new();
        let mut bytes = 0_usize;
        let mut cursor = String::new();
        loop {
            let rows=sqlx::query("SELECT address_key,entry_json FROM thread_addresses WHERE address_key>? ORDER BY address_key LIMIT 100").bind(&cursor).fetch_all(&mut *transaction).await?;
            if rows.is_empty() {
                break;
            }
            for row in rows {
                cursor = row.try_get("address_key")?;
                let text: String = row.try_get("entry_json")?;
                let entry: AddressEntry =
                    serde_json::from_str(&text).map_err(|_| JournalError::InvalidStorage)?;
                if entry.address.endpoint != *endpoint {
                    continue;
                }
                bytes = bytes
                    .checked_add(text.len())
                    .filter(|size| *size <= 32 * 1024 * 1024)
                    .ok_or(JournalError::Capacity)?;
                if entry.last_observation.journal_id != self.journal_id
                    || entry.last_observation.sequence
                        > u64::try_from(last).map_err(|_| JournalError::InvalidStorage)?
                {
                    return Err(JournalError::InvalidStorage);
                }
                entries.push(entry);
            }
        }
        transaction.commit().await?;
        entries.sort_by(|left, right| {
            left.address
                .native_thread_id
                .cmp(&right.address.native_thread_id)
        });
        Ok(CapturedAddresses {
            watermark: JournalPosition {
                journal_id: self.journal_id.clone(),
                sequence: u64::try_from(last).map_err(|_| JournalError::InvalidStorage)?,
            },
            entries,
            serialized_bytes: bytes,
        })
    }
}

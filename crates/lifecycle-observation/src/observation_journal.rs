//! Transactional lifecycle append storage. Public projection/replay admission is separate.
use communication_protocol::AddressEntry;
use communication_protocol::JournalPosition;
use communication_protocol::{LifecycleObservation, LifecycleSubject, ThreadAddress, UuidIdentity};
use sqlx::{
    Connection, Row, SqliteConnection,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteSynchronous},
};
use std::{path::Path, time::Duration};

#[derive(Debug, thiserror::Error)]
pub enum JournalError {
    #[error("address snapshot expired")]
    SnapshotExpired,
    #[error("journal identity changed")]
    JournalChanged,
    #[error("requested lifecycle history has expired")]
    HistoryExpired,
    #[error("lifecycle storage unavailable")]
    Storage(#[from] sqlx::Error),
    #[error("invalid lifecycle record")]
    InvalidRecord,
    #[error("unsupported or inconsistent lifecycle storage")]
    InvalidStorage,
    #[error("lifecycle storage capacity exceeded")]
    Capacity,
}
pub struct JournalRow {
    pub sequence: u64,
    pub retention_at: i64,
    pub observation: LifecycleObservation,
}
pub struct ObservationJournal {
    pub(crate) connection: SqliteConnection,
    pub(crate) journal_id: UuidIdentity,
}
impl ObservationJournal {
    pub async fn open(path: &Path, new_identity: UuidIdentity) -> Result<Self, JournalError> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Delete)
            .synchronous(SqliteSynchronous::Full)
            .busy_timeout(Duration::from_secs(1));
        let mut connection = SqliteConnection::connect_with(&options).await?;
        let mut transaction = connection.begin().await?;
        sqlx::query("CREATE TABLE IF NOT EXISTS journal_metadata (singleton INTEGER PRIMARY KEY CHECK(singleton=1), version INTEGER NOT NULL, journal_id TEXT NOT NULL, last_sequence INTEGER NOT NULL, retention_clock INTEGER NOT NULL, payload_bytes INTEGER NOT NULL)").execute(&mut *transaction).await?;
        sqlx::query("CREATE TABLE IF NOT EXISTS lifecycle_records (sequence INTEGER PRIMARY KEY, retention_at INTEGER NOT NULL, observation_json TEXT NOT NULL)").execute(&mut *transaction).await?;
        sqlx::query("CREATE TABLE IF NOT EXISTS thread_addresses (address_key TEXT PRIMARY KEY, entry_json TEXT NOT NULL, payload_bytes INTEGER NOT NULL)").execute(&mut *transaction).await?;
        sqlx::query("CREATE TABLE IF NOT EXISTS journal_checkpoint (singleton INTEGER PRIMARY KEY CHECK(singleton=1), sequence INTEGER NOT NULL)").execute(&mut *transaction).await?;
        sqlx::query("INSERT OR IGNORE INTO journal_checkpoint VALUES (1,0)")
            .execute(&mut *transaction)
            .await?;
        sqlx::query("CREATE TABLE IF NOT EXISTS checkpoint_addresses (address_key TEXT PRIMARY KEY, entry_json TEXT NOT NULL)").execute(&mut *transaction).await?;
        sqlx::query("INSERT OR IGNORE INTO journal_metadata VALUES (1,1,?,0,0,0)")
            .bind(String::from(new_identity))
            .execute(&mut *transaction)
            .await?;
        let row = sqlx::query("SELECT version,journal_id FROM journal_metadata WHERE singleton=1")
            .fetch_one(&mut *transaction)
            .await?;
        if row.try_get::<i64, _>("version")? != 1 {
            return Err(JournalError::InvalidStorage);
        }
        let journal_id = UuidIdentity::try_from(row.try_get::<String, _>("journal_id")?)
            .map_err(|_| JournalError::InvalidStorage)?;
        transaction.commit().await?;
        Ok(Self {
            connection,
            journal_id,
        })
    }
    #[must_use]
    pub fn journal_id(&self) -> &UuidIdentity {
        &self.journal_id
    }
    pub async fn append(
        &mut self,
        observation: &LifecycleObservation,
        now: i64,
    ) -> Result<JournalRow, JournalError> {
        observation
            .validate()
            .map_err(|_| JournalError::InvalidRecord)?;
        if now < 0 {
            return Err(JournalError::InvalidRecord);
        }
        let payload =
            serde_json::to_string(observation).map_err(|_| JournalError::InvalidRecord)?;
        let mut transaction = self.connection.begin_with("BEGIN IMMEDIATE").await?;
        let state=sqlx::query("SELECT last_sequence,retention_clock,payload_bytes FROM journal_metadata WHERE singleton=1").fetch_one(&mut *transaction).await?;
        let previous = state.try_get::<i64, _>("last_sequence")?;
        if !(0..9_007_199_254_740_991).contains(&previous) {
            return Err(JournalError::Capacity);
        }
        let sequence = previous + 1;
        let retention_at = now.max(state.try_get("retention_clock")?);
        let size = i64::try_from(payload.len()).map_err(|_| JournalError::Capacity)?;
        let total = state
            .try_get::<i64, _>("payload_bytes")?
            .checked_add(size)
            .filter(|bytes| *bytes <= 256 * 1024 * 1024)
            .ok_or(JournalError::Capacity)?;
        sqlx::query("INSERT INTO lifecycle_records VALUES (?,?,?)")
            .bind(sequence)
            .bind(retention_at)
            .bind(payload)
            .execute(&mut *transaction)
            .await?;
        sqlx::query("UPDATE journal_metadata SET last_sequence=?,retention_clock=?,payload_bytes=? WHERE singleton=1").bind(sequence).bind(retention_at).bind(total).execute(&mut *transaction).await?;
        if let LifecycleSubject::Thread { address } = &observation.subject {
            let key = serde_json::to_string(address).map_err(|_| JournalError::InvalidRecord)?;
            let prior = sqlx::query(
                "SELECT entry_json,payload_bytes FROM thread_addresses WHERE address_key=?",
            )
            .bind(&key)
            .fetch_optional(&mut *transaction)
            .await?;
            let prior_size = prior
                .as_ref()
                .map(|row| row.try_get::<i64, _>("payload_bytes"))
                .transpose()?
                .unwrap_or(0);
            let previous = prior
                .map(|row| row.try_get::<String, _>("entry_json"))
                .transpose()?
                .map(|text| serde_json::from_str(&text).map_err(|_| JournalError::InvalidStorage))
                .transpose()?;
            let entry = crate::thread_address_book::reduce_address_observation(
                previous,
                observation,
                JournalPosition {
                    journal_id: self.journal_id.clone(),
                    sequence: u64::try_from(sequence).map_err(|_| JournalError::InvalidStorage)?,
                },
            )?;
            let serialized =
                serde_json::to_string(&entry).map_err(|_| JournalError::InvalidRecord)?;
            let entry_size = i64::try_from(serialized.len()).map_err(|_| JournalError::Capacity)?;
            let used: i64 =
                sqlx::query_scalar("SELECT COALESCE(SUM(payload_bytes),0) FROM thread_addresses")
                    .fetch_one(&mut *transaction)
                    .await?;
            if used - prior_size + entry_size > 64 * 1024 * 1024 {
                return Err(JournalError::Capacity);
            }
            sqlx::query("INSERT INTO thread_addresses VALUES (?,?,?) ON CONFLICT(address_key) DO UPDATE SET entry_json=excluded.entry_json,payload_bytes=excluded.payload_bytes").bind(key).bind(serialized).bind(entry_size).execute(&mut *transaction).await?;
        }
        transaction.commit().await?;
        Ok(JournalRow {
            sequence: u64::try_from(sequence).map_err(|_| JournalError::InvalidStorage)?,
            retention_at,
            observation: observation.clone(),
        })
    }
    pub async fn read_after(
        &mut self,
        after: u64,
        limit: u32,
    ) -> Result<Vec<JournalRow>, JournalError> {
        if after > 9_007_199_254_740_991 || !(1..=100).contains(&limit) {
            return Err(JournalError::InvalidRecord);
        }
        let mut transaction = self.connection.begin().await?;
        let checkpoint: i64 =
            sqlx::query_scalar("SELECT sequence FROM journal_checkpoint WHERE singleton=1")
                .fetch_one(&mut *transaction)
                .await?;
        if i64::try_from(after).map_err(|_| JournalError::InvalidRecord)? < checkpoint {
            return Err(JournalError::HistoryExpired);
        }
        let rows=sqlx::query("SELECT sequence,retention_at,observation_json FROM lifecycle_records WHERE sequence>? ORDER BY sequence LIMIT ?").bind(i64::try_from(after).map_err(|_|JournalError::InvalidRecord)?).bind(limit).fetch_all(&mut *transaction).await?;
        transaction.commit().await?;
        rows.into_iter()
            .map(|row| {
                let observation: LifecycleObservation =
                    serde_json::from_str(&row.try_get::<String, _>("observation_json")?)
                        .map_err(|_| JournalError::InvalidStorage)?;
                observation
                    .validate()
                    .map_err(|_| JournalError::InvalidStorage)?;
                Ok(JournalRow {
                    sequence: u64::try_from(row.try_get::<i64, _>("sequence")?)
                        .map_err(|_| JournalError::InvalidStorage)?,
                    retention_at: row.try_get("retention_at")?,
                    observation,
                })
            })
            .collect()
    }
    pub async fn address_entry(
        &mut self,
        address: &ThreadAddress,
    ) -> Result<Option<AddressEntry>, JournalError> {
        let key = serde_json::to_string(address).map_err(|_| JournalError::InvalidRecord)?;
        let row = sqlx::query("SELECT entry_json FROM thread_addresses WHERE address_key=?")
            .bind(key)
            .fetch_optional(&mut self.connection)
            .await?;
        row.map(|row| {
            let text: String = row.try_get("entry_json")?;
            serde_json::from_str(&text).map_err(|_| JournalError::InvalidStorage)
        })
        .transpose()
    }
    pub async fn close(self) {
        let _closed = self.connection.close().await;
    }
}

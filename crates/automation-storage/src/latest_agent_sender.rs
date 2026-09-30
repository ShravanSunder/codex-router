//! Durable latest direct Agent-message sender per recipient SessionRef.
use crate::{AutomationStore, StorageError};
use chrono::{DateTime, Duration, Utc};
use collaboration_protocol::{EndpointId, EndpointRef, SessionId, SessionRef, UuidIdentity};
use sqlx::Row;

const RETENTION_DAYS: i64 = 30;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LatestAgentSenderRecord {
    pub recipient: SessionRef,
    pub sender: SessionRef,
    pub delivered_at: DateTime<Utc>,
}

impl LatestAgentSenderRecord {
    pub fn new(
        recipient: SessionRef,
        sender: SessionRef,
        delivered_at: DateTime<Utc>,
    ) -> Result<Self, StorageError> {
        if delivered_at.timestamp_millis() < 0 {
            return Err(StorageError::InvalidRecord);
        }
        Ok(Self {
            recipient,
            sender,
            delivered_at,
        })
    }
}

impl AutomationStore {
    /// Atomically replaces the recipient's latest accepted direct Agent sender.
    pub async fn store_latest_agent_sender(
        &mut self,
        record: &LatestAgentSenderRecord,
    ) -> Result<(), StorageError> {
        sqlx::query(
            "INSERT INTO latest_agent_senders (
                recipient_service_id,recipient_endpoint_id,recipient_session_id,
                sender_service_id,sender_endpoint_id,sender_session_id,delivered_at_ms
            ) VALUES (?,?,?,?,?,?,?)
            ON CONFLICT(recipient_service_id,recipient_endpoint_id,recipient_session_id)
            DO UPDATE SET sender_service_id=excluded.sender_service_id,
                sender_endpoint_id=excluded.sender_endpoint_id,
                sender_session_id=excluded.sender_session_id,
                delivered_at_ms=excluded.delivered_at_ms",
        )
        .bind(String::from(record.recipient.endpoint.service_id.clone()))
        .bind(String::from(record.recipient.endpoint.endpoint_id.clone()))
        .bind(String::from(record.recipient.session_id.clone()))
        .bind(String::from(record.sender.endpoint.service_id.clone()))
        .bind(String::from(record.sender.endpoint.endpoint_id.clone()))
        .bind(String::from(record.sender.session_id.clone()))
        .bind(record.delivered_at.timestamp_millis())
        .execute(&mut self.connection)
        .await?;
        Ok(())
    }

    /// Removes the recipient's reply address after a failed latest-sender update.
    pub async fn remove_latest_agent_sender_for(
        &mut self,
        recipient: &SessionRef,
    ) -> Result<(), StorageError> {
        sqlx::query(
            "DELETE FROM latest_agent_senders
            WHERE recipient_service_id=? AND recipient_endpoint_id=? AND recipient_session_id=?",
        )
        .bind(String::from(recipient.endpoint.service_id.clone()))
        .bind(String::from(recipient.endpoint.endpoint_id.clone()))
        .bind(String::from(recipient.session_id.clone()))
        .execute(&mut self.connection)
        .await?;
        Ok(())
    }

    /// Reads the latest sender only while its 30-day retention window is open.
    pub async fn latest_agent_sender_for(
        &mut self,
        recipient: &SessionRef,
        now: DateTime<Utc>,
    ) -> Result<Option<LatestAgentSenderRecord>, StorageError> {
        let cutoff = retention_cutoff(now)?;
        let Some(row) = sqlx::query(
            "SELECT sender_service_id,sender_endpoint_id,sender_session_id,delivered_at_ms
            FROM latest_agent_senders
            WHERE recipient_service_id=? AND recipient_endpoint_id=? AND recipient_session_id=?
                AND delivered_at_ms>=?",
        )
        .bind(String::from(recipient.endpoint.service_id.clone()))
        .bind(String::from(recipient.endpoint.endpoint_id.clone()))
        .bind(String::from(recipient.session_id.clone()))
        .bind(cutoff.timestamp_millis())
        .fetch_optional(&mut self.connection)
        .await?
        else {
            return Ok(None);
        };
        let sender_service_id: String = row.try_get("sender_service_id")?;
        let sender_endpoint_id: String = row.try_get("sender_endpoint_id")?;
        let sender_session_id: String = row.try_get("sender_session_id")?;
        let delivered_at_ms: i64 = row.try_get("delivered_at_ms")?;
        let sender = SessionRef {
            endpoint: EndpointRef {
                service_id: UuidIdentity::try_from(sender_service_id)
                    .map_err(|_| StorageError::InvalidRecord)?,
                endpoint_id: EndpointId::try_from(sender_endpoint_id)
                    .map_err(|_| StorageError::InvalidRecord)?,
            },
            session_id: SessionId::try_from(sender_session_id)
                .map_err(|_| StorageError::InvalidRecord)?,
        };
        let delivered_at = DateTime::<Utc>::from_timestamp_millis(delivered_at_ms)
            .ok_or(StorageError::InvalidRecord)?;
        LatestAgentSenderRecord::new(recipient.clone(), sender, delivered_at).map(Some)
    }

    /// Prunes one bounded batch of sender records older than the 30-day window.
    pub async fn prune_latest_agent_senders(
        &mut self,
        now: DateTime<Utc>,
        batch_limit: u32,
    ) -> Result<u64, StorageError> {
        if !(1..=1000).contains(&batch_limit) {
            return Err(StorageError::InvalidRecord);
        }
        let cutoff = retention_cutoff(now)?;
        let result = sqlx::query(
            "DELETE FROM latest_agent_senders WHERE rowid IN (
                SELECT rowid FROM latest_agent_senders
                WHERE delivered_at_ms<?
                ORDER BY delivered_at_ms,recipient_service_id,recipient_endpoint_id,recipient_session_id
                LIMIT ?
            )",
        )
        .bind(cutoff.timestamp_millis())
        .bind(batch_limit)
        .execute(&mut self.connection)
        .await?;
        Ok(result.rows_affected())
    }
}

fn retention_cutoff(now: DateTime<Utc>) -> Result<DateTime<Utc>, StorageError> {
    now.checked_sub_signed(Duration::days(RETENTION_DAYS))
        .ok_or(StorageError::InvalidRecord)
}

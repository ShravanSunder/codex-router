//! Materialized delivery snapshot shared by inspection and atomic wake lifecycle receipts.
use crate::{AutomationStore, StorageError};
use agent_automation::{DeliveryAttempt, DeliveryId, DeliveryStatus, OccurrenceId, WakeupId};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sqlx::{Row, SqliteConnection};
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeliveryRecord<TTarget, TGeneration, TReceipt> {
    pub delivery_id: DeliveryId,
    pub wakeup_id: WakeupId,
    pub occurrence_id: OccurrenceId,
    pub target: TTarget,
    pub mode: String,
    pub status: DeliveryStatus,
    pub eligible_at_ms: i64,
    pub expires_at_ms: Option<i64>,
    pub attempt: Option<DeliveryAttempt<TTarget, TGeneration>>,
    pub receipt: Option<TReceipt>,
}
impl AutomationStore {
    /// Read the delivery's frozen content, never a subsequently changed wake definition.
    pub async fn read_delivery_content<TContent: DeserializeOwned>(
        &mut self,
        id: &DeliveryId,
    ) -> Result<TContent, StorageError> {
        let text: String =
            sqlx::query_scalar("SELECT message_json FROM mailbox_deliveries WHERE delivery_id=?")
                .bind(id.as_str())
                .fetch_optional(&mut self.connection)
                .await?
                .ok_or(StorageError::InvalidRecord)?;
        serde_json::from_str(&text).map_err(|_| StorageError::InvalidRecord)
    }
    pub async fn read_delivery<
        TTarget: DeserializeOwned,
        TGeneration: DeserializeOwned,
        TReceipt: DeserializeOwned,
    >(
        &mut self,
        id: &DeliveryId,
    ) -> Result<DeliveryRecord<TTarget, TGeneration, TReceipt>, StorageError> {
        read_current(&mut self.connection, id).await
    }
}
pub(crate) async fn read_current<
    TTarget: DeserializeOwned,
    TGeneration: DeserializeOwned,
    TReceipt: DeserializeOwned,
>(
    connection: &mut SqliteConnection,
    id: &DeliveryId,
) -> Result<DeliveryRecord<TTarget, TGeneration, TReceipt>, StorageError> {
    let row = sqlx::query("SELECT * FROM mailbox_deliveries WHERE delivery_id=?")
        .bind(id.as_str())
        .fetch_optional(connection)
        .await?
        .ok_or(StorageError::InvalidRecord)?;
    fn decode<TValue: DeserializeOwned>(text: String) -> Result<TValue, StorageError> {
        serde_json::from_str(&text).map_err(|_| StorageError::InvalidRecord)
    }
    Ok(DeliveryRecord {
        delivery_id: id.clone(),
        wakeup_id: row
            .try_get::<String, _>("wakeup_id")?
            .try_into()
            .map_err(|_| StorageError::InvalidRecord)?,
        occurrence_id: row
            .try_get::<String, _>("occurrence_id")?
            .try_into()
            .map_err(|_| StorageError::InvalidRecord)?,
        target: decode(row.try_get("target_json")?)?,
        mode: row.try_get("delivery_mode")?,
        status: serde_json::from_value(serde_json::Value::String(row.try_get("delivery_status")?))
            .map_err(|_| StorageError::InvalidRecord)?,
        eligible_at_ms: row.try_get("eligible_at_ms")?,
        expires_at_ms: row.try_get("expires_at_ms")?,
        attempt: row
            .try_get::<Option<String>, _>("latest_attempt_json")?
            .map(decode)
            .transpose()?,
        receipt: row
            .try_get::<Option<String>, _>("accepted_receipt_json")?
            .map(decode)
            .transpose()?,
    })
}

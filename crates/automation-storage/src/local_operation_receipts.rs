//! Replay helpers for atomic local mutations; external in-progress operations use their own states.
use crate::StorageError;
use agent_automation::OperationId;
use serde::de::DeserializeOwned;
use sqlx::{Row, SqliteConnection};
pub(crate) struct LocalOperation<'a> {
    pub id: &'a OperationId,
    pub method: &'a str,
    pub canonical: &'a [u8],
}
pub(crate) struct CompletedLocalOperation<'a> {
    pub operation: LocalOperation<'a>,
    pub resource_id: &'a str,
    pub result_json: &'a str,
    pub now_ms: i64,
}
pub(crate) async fn replay<TResult: DeserializeOwned>(
    connection: &mut SqliteConnection,
    operation: LocalOperation<'_>,
) -> Result<Option<TResult>, StorageError> {
    let Some(row)=sqlx::query("SELECT method_name,canonical_request,operation_status,final_result_json FROM operation_receipts WHERE operation_id=?").bind(operation.id.as_str()).fetch_optional(connection).await? else{return Ok(None);};
    if row.try_get::<String, _>("method_name")? != operation.method
        || row.try_get::<Vec<u8>, _>("canonical_request")? != operation.canonical
    {
        return Err(StorageError::OperationConflict);
    }
    if row.try_get::<String, _>("operation_status")? != "succeeded" {
        return Err(StorageError::InvalidRecord);
    }
    let encoded = row
        .try_get::<Option<String>, _>("final_result_json")?
        .ok_or(StorageError::InvalidRecord)?;
    serde_json::from_str(&encoded)
        .map(Some)
        .map_err(|_| StorageError::InvalidRecord)
}
pub(crate) async fn record(
    connection: &mut SqliteConnection,
    receipt: CompletedLocalOperation<'_>,
) -> Result<(), StorageError> {
    sqlx::query("INSERT INTO operation_receipts(operation_id,method_name,canonical_request,resource_id,operation_status,effect_evidence_json,final_result_json,final_error_json,committed_at_ms) VALUES (?,?,?,?,'succeeded',?,?,NULL,?)")
        .bind(receipt.operation.id.as_str()).bind(receipt.operation.method).bind(receipt.operation.canonical).bind(receipt.resource_id)
        .bind(r#"{"kind":"local","mutation":"committed"}"#).bind(receipt.result_json).bind(receipt.now_ms).execute(connection).await?;
    Ok(())
}

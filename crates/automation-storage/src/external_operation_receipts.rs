//! External mutations commit admission and intent before I/O; retries inspect retained evidence.
use crate::{AutomationStore, StorageError};
use agent_automation::OperationId;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sqlx::{Connection, Row};
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExternalOperationRecord<TState, TResult> {
    pub operation_id: OperationId,
    pub method: String,
    pub resource_id: String,
    pub status: String,
    pub evidence: TState,
    pub result: Option<TResult>,
    pub failure: Option<serde_json::Value>,
    pub admitted_at_ms: i64,
}
pub struct ExternalAdmission<TState> {
    pub operation_id: OperationId,
    pub schedule_id: agent_automation::ScheduleId,
    pub canonical_request: Vec<u8>,
    pub evidence: TState,
    pub now_ms: i64,
}
pub enum ExternalAdmissionResult<TState, TResult> {
    New,
    Existing(ExternalOperationRecord<TState, TResult>),
}
impl AutomationStore {
    pub async fn admit_schedule_preparation<
        TState: Serialize + DeserializeOwned,
        TResult: DeserializeOwned,
    >(
        &mut self,
        request: &ExternalAdmission<TState>,
    ) -> Result<ExternalAdmissionResult<TState, TResult>, StorageError> {
        if request.now_ms < 0 || request.canonical_request.len() > 1_048_576 {
            return Err(StorageError::InvalidRecord);
        }
        let mut transaction = self.connection.begin_with("BEGIN IMMEDIATE").await?;
        let existing = sqlx::query("SELECT * FROM operation_receipts WHERE operation_id=?")
            .bind(request.operation_id.as_str())
            .fetch_optional(&mut *transaction)
            .await?;
        if let Some(row) = existing {
            if row.try_get::<String, _>("method_name")? != "schedule/prepare"
                || row.try_get::<String, _>("resource_id")? != request.schedule_id.as_str()
                || row.try_get::<Vec<u8>, _>("canonical_request")? != request.canonical_request
            {
                return Err(StorageError::OperationConflict);
            }
            let status: String = row.try_get("operation_status")?;
            if !matches!(
                status.as_str(),
                "admitted" | "inProgress" | "uncertain" | "succeeded" | "failed"
            ) {
                return Err(StorageError::InvalidRecord);
            }
            let evidence = serde_json::from_str(&row.try_get::<String, _>("effect_evidence_json")?)
                .map_err(|_| StorageError::InvalidRecord)?;
            let result = row
                .try_get::<Option<String>, _>("final_result_json")?
                .map(|value| serde_json::from_str(&value).map_err(|_| StorageError::InvalidRecord))
                .transpose()?;
            let record = ExternalOperationRecord {
                operation_id: request.operation_id.clone(),
                method: "schedule/prepare".into(),
                resource_id: request.schedule_id.as_str().to_owned(),
                status,
                evidence,
                result,
                failure: row
                    .try_get::<Option<String>, _>("final_error_json")?
                    .map(|text| {
                        serde_json::from_str(&text).map_err(|_| StorageError::InvalidRecord)
                    })
                    .transpose()?,
                admitted_at_ms: row.try_get("committed_at_ms")?,
            };
            transaction.commit().await?;
            return Ok(ExternalAdmissionResult::Existing(record));
        }
        let other:Option<String>=sqlx::query_scalar("SELECT operation_id FROM operation_receipts WHERE resource_id=? AND method_name='schedule/prepare' AND operation_status IN ('admitted','inProgress','uncertain') LIMIT 1").bind(request.schedule_id.as_str()).fetch_optional(&mut *transaction).await?;
        if other.is_some() {
            return Err(StorageError::OperationConflict);
        }
        let evidence =
            serde_json::to_string(&request.evidence).map_err(|_| StorageError::InvalidRecord)?;
        sqlx::query("INSERT INTO operation_receipts(operation_id,method_name,canonical_request,resource_id,operation_status,effect_evidence_json,final_result_json,final_error_json,committed_at_ms) VALUES (?,'schedule/prepare',?,?,'admitted',?,NULL,NULL,?)")
            .bind(request.operation_id.as_str()).bind(&request.canonical_request).bind(request.schedule_id.as_str()).bind(evidence).bind(request.now_ms).execute(&mut *transaction).await?;
        transaction.commit().await?;
        Ok(ExternalAdmissionResult::New)
    }
}

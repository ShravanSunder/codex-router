//! Original command results survive event cleanup; absent final data cannot become success.
use crate::{AutomationStore, StorageError};
use agent_automation::OperationId;
use sqlx::Row;
pub enum StoredOperationState {
    Admitted {
        effects: serde_json::Value,
    },
    InProgress {
        effects: serde_json::Value,
    },
    Uncertain {
        effects: serde_json::Value,
        failure: Option<serde_json::Value>,
    },
    Succeeded {
        result: serde_json::Value,
    },
    Failed {
        effects: serde_json::Value,
        failure: serde_json::Value,
    },
}
pub struct StoredOperationRecord {
    pub operation_id: OperationId,
    pub method: String,
    pub resource_id: String,
    pub admitted_at_ms: i64,
    pub state: StoredOperationState,
}
impl AutomationStore {
    pub async fn read_operation(
        &mut self,
        operation_id: &OperationId,
    ) -> Result<StoredOperationRecord, StorageError> {
        let row = sqlx::query("SELECT * FROM operation_receipts WHERE operation_id=?")
            .bind(operation_id.as_str())
            .fetch_optional(&mut self.connection)
            .await?
            .ok_or(StorageError::OperationNotFound)?;
        let status: String = row.try_get("operation_status")?;
        let effects = decode(row.try_get("effect_evidence_json")?)?;
        let result = row
            .try_get::<Option<String>, _>("final_result_json")?
            .map(decode)
            .transpose()?;
        let failure = row
            .try_get::<Option<String>, _>("final_error_json")?
            .map(decode)
            .transpose()?;
        let state = match (status.as_str(), result, failure) {
            ("admitted", None, None) => StoredOperationState::Admitted { effects },
            ("inProgress", None, None) => StoredOperationState::InProgress { effects },
            ("uncertain", None, failure) => StoredOperationState::Uncertain { effects, failure },
            ("succeeded", Some(result), None) => StoredOperationState::Succeeded { result },
            ("failed", None, Some(failure)) => StoredOperationState::Failed { effects, failure },
            _ => return Err(StorageError::InvalidRecord),
        };
        Ok(StoredOperationRecord {
            operation_id: operation_id.clone(),
            method: row.try_get("method_name")?,
            resource_id: row.try_get("resource_id")?,
            admitted_at_ms: row.try_get("committed_at_ms")?,
            state,
        })
    }
}
fn decode(value: String) -> Result<serde_json::Value, StorageError> {
    serde_json::from_str(&value).map_err(|_| StorageError::InvalidRecord)
}

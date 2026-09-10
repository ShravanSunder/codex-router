//! Configuration-file recovery uses durable command receipts, without adding a settings table.
use crate::{AutomationStore, StorageError};
use agent_automation::OperationId;
use serde::{Serialize, de::DeserializeOwned};
use sqlx::{Connection, Row};
pub struct ConfigurationOperation<TConfiguration> {
    pub operation_id: OperationId,
    pub configuration: TConfiguration,
}
pub enum ConfigurationAdmission<TConfiguration> {
    New,
    Existing(TConfiguration),
    Pending(ConfigurationOperation<TConfiguration>),
}
pub enum ConfigurationProgress {
    Writing,
    WriteUncertain,
    ReceiptUncertain,
}
impl AutomationStore {
    pub async fn record_configuration_progress<TConfiguration: Serialize>(
        &mut self,
        id: &OperationId,
        configuration: &TConfiguration,
        progress: ConfigurationProgress,
    ) -> Result<bool, StorageError> {
        let canonical =
            serde_json::to_vec(configuration).map_err(|_| StorageError::InvalidRecord)?;
        let (status, file_state) = match progress {
            ConfigurationProgress::Writing => ("inProgress", "unknown"),
            ConfigurationProgress::WriteUncertain => ("uncertain", "unknown"),
            ConfigurationProgress::ReceiptUncertain => ("uncertain", "replaced"),
        };
        let evidence = serde_json::json!({"kind":"configuration","fileState":file_state,"intended":configuration});
        let updated = sqlx::query("UPDATE operation_receipts SET operation_status=?,effect_evidence_json=? WHERE operation_id=? AND method_name='automation/configure' AND canonical_request=? AND operation_status IN ('admitted','inProgress','uncertain')")
            .bind(status).bind(evidence.to_string()).bind(id.as_str()).bind(canonical).execute(&mut self.connection).await?.rows_affected();
        Ok(updated == 1)
    }
    pub async fn admit_configuration<TConfiguration: Serialize + DeserializeOwned>(
        &mut self,
        id: &OperationId,
        configuration: &TConfiguration,
        now_ms: i64,
    ) -> Result<ConfigurationAdmission<TConfiguration>, StorageError> {
        let canonical =
            serde_json::to_vec(configuration).map_err(|_| StorageError::InvalidRecord)?;
        let mut transaction = self.connection.begin_with("BEGIN IMMEDIATE").await?;
        let existing=sqlx::query("SELECT method_name,canonical_request,operation_status,final_result_json FROM operation_receipts WHERE operation_id=?").bind(id.as_str()).fetch_optional(&mut *transaction).await?;
        if let Some(row) = existing {
            if row.try_get::<String, _>("method_name")? != "automation/configure"
                || row.try_get::<Vec<u8>, _>("canonical_request")? != canonical
            {
                return Err(StorageError::OperationConflict);
            }
            let status: String = row.try_get("operation_status")?;
            let result = if status == "succeeded" {
                ConfigurationAdmission::Existing(
                    serde_json::from_str(&row.try_get::<String, _>("final_result_json")?)
                        .map_err(|_| StorageError::InvalidRecord)?,
                )
            } else if matches!(status.as_str(), "admitted" | "inProgress" | "uncertain") {
                ConfigurationAdmission::Pending(ConfigurationOperation {
                    operation_id: id.clone(),
                    configuration: serde_json::from_slice(&canonical)
                        .map_err(|_| StorageError::InvalidRecord)?,
                })
            } else {
                return Err(StorageError::InvalidRecord);
            };
            transaction.commit().await?;
            return Ok(result);
        }
        let pending:i64=sqlx::query_scalar("SELECT COUNT(*) FROM operation_receipts WHERE method_name='automation/configure' AND operation_status IN ('admitted','inProgress','uncertain')").fetch_one(&mut *transaction).await?;
        if pending != 0 {
            return Err(StorageError::OperationConflict);
        }
        let evidence = serde_json::json!({"kind":"configuration","fileState":"notReplaced","intended":configuration});
        sqlx::query("INSERT INTO operation_receipts(operation_id,method_name,canonical_request,resource_id,operation_status,effect_evidence_json,committed_at_ms) VALUES (?,'automation/configure',?,'automationConfiguration','admitted',?,?)")
            .bind(id.as_str()).bind(canonical).bind(evidence.to_string()).bind(now_ms).execute(&mut *transaction).await?;
        transaction.commit().await?;
        Ok(ConfigurationAdmission::New)
    }
    pub async fn pending_configuration<TConfiguration: DeserializeOwned>(
        &mut self,
    ) -> Result<Option<ConfigurationOperation<TConfiguration>>, StorageError> {
        let rows=sqlx::query("SELECT operation_id,canonical_request FROM operation_receipts WHERE method_name='automation/configure' AND operation_status IN ('admitted','inProgress','uncertain') LIMIT 2").fetch_all(&mut self.connection).await?;
        if rows.len() > 1 {
            return Err(StorageError::InvalidRecord);
        }
        rows.into_iter()
            .next()
            .map(|row| {
                Ok(ConfigurationOperation {
                    operation_id: row
                        .try_get::<String, _>("operation_id")?
                        .try_into()
                        .map_err(|_| StorageError::InvalidRecord)?,
                    configuration: serde_json::from_slice(
                        &row.try_get::<Vec<u8>, _>("canonical_request")?,
                    )
                    .map_err(|_| StorageError::InvalidRecord)?,
                })
            })
            .transpose()
    }
    pub async fn latest_configuration<TConfiguration: DeserializeOwned>(
        &mut self,
    ) -> Result<Option<ConfigurationOperation<TConfiguration>>, StorageError> {
        let row=sqlx::query("SELECT operation_id,final_result_json FROM operation_receipts WHERE method_name='automation/configure' AND operation_status='succeeded' ORDER BY rowid DESC LIMIT 1").fetch_optional(&mut self.connection).await?;
        row.map(|row| {
            Ok(ConfigurationOperation {
                operation_id: row
                    .try_get::<String, _>("operation_id")?
                    .try_into()
                    .map_err(|_| StorageError::InvalidRecord)?,
                configuration: serde_json::from_str(
                    &row.try_get::<String, _>("final_result_json")?,
                )
                .map_err(|_| StorageError::InvalidRecord)?,
            })
        })
        .transpose()
    }
    pub async fn complete_configuration<TConfiguration: Serialize>(
        &mut self,
        id: &OperationId,
        configuration: &TConfiguration,
        now_ms: i64,
    ) -> Result<(), StorageError> {
        let encoded =
            serde_json::to_string(configuration).map_err(|_| StorageError::InvalidRecord)?;
        let canonical =
            serde_json::to_vec(configuration).map_err(|_| StorageError::InvalidRecord)?;
        let evidence = serde_json::json!({"kind":"configuration","fileState":"replaced","intended":configuration});
        let mut transaction = self.connection.begin_with("BEGIN IMMEDIATE").await?;
        let changed=sqlx::query("UPDATE operation_receipts SET operation_status='succeeded',effect_evidence_json=?,final_result_json=? WHERE operation_id=? AND method_name='automation/configure' AND canonical_request=? AND operation_status IN ('admitted','inProgress','uncertain')")
            .bind(evidence.to_string()).bind(encoded).bind(id.as_str()).bind(canonical).execute(&mut *transaction).await?.rows_affected();
        if changed != 1 {
            return Err(StorageError::OperationConflict);
        }
        let event = serde_json::json!({"kind":"configurationChange","configuration":configuration});
        sqlx::query("INSERT INTO automation_events(event_id,subject_kind,subject_id,event_kind,event_body_json,recorded_at_ms) VALUES (?,'configuration','automationConfiguration','configured',?,?)").bind(agent_automation::EventId::generate().as_str()).bind(event.to_string()).bind(now_ms).execute(&mut *transaction).await?;
        transaction.commit().await?;
        Ok(())
    }
}

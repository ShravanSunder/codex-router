//! Interruption intent belongs to the exact recorded turn; it never releases execution ownership.
use crate::{AutomationStore, StorageError};
use agent_automation::{RunId, RunPhase};
use serde::{Serialize, de::DeserializeOwned};
use sqlx::Connection;
impl AutomationStore {
    pub async fn begin_run_stop<
        TTarget: Serialize + DeserializeOwned,
        TEndpoint: DeserializeOwned,
        TGeneration: Serialize + DeserializeOwned,
        TReceipt: Serialize + DeserializeOwned,
    >(
        &mut self,
        id: &RunId,
        turn_id: &str,
    ) -> Result<bool, StorageError> {
        let mut transaction = self.connection.begin_with("BEGIN IMMEDIATE").await?;
        let mut record =
            crate::run_inspection::read_current::<TTarget, TEndpoint, TGeneration, TReceipt>(
                &mut transaction,
                id,
            )
            .await?;
        if !matches!(record.phase, RunPhase::Executing | RunPhase::Uncertain)
            || record.native_turn_id.as_deref() != Some(turn_id)
        {
            transaction.commit().await?;
            return Ok(false);
        }
        record.evidence.native.cessation = agent_automation::CessationEvidence::Unconfirmed;
        sqlx::query("UPDATE workflow_runs SET run_status='stopping',execution_evidence_json=? WHERE run_id=?").bind(serde_json::to_string(&record.evidence).map_err(|_|StorageError::InvalidRecord)?).bind(id.as_str()).execute(&mut *transaction).await?;
        transaction.commit().await?;
        Ok(true)
    }
}

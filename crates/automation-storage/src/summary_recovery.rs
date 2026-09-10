//! Retry and skip recover the same occupying Run; they never admit a successor or force-stop uncertainty.
use crate::local_operation_receipts::{self, CompletedLocalOperation, LocalOperation};
use crate::{AutomationStore, StorageError};
use agent_automation::{OperationId, RunId, RunRecord};
use serde::{Serialize, de::DeserializeOwned};
use sqlx::Connection;
#[derive(Clone, Copy)]
pub enum SummaryRecoveryAction {
    Retry { timeout_seconds: u32 },
    Skip,
}
#[derive(Clone)]
pub struct SummaryRecoveryRequest {
    pub operation_id: OperationId,
    pub run_id: RunId,
    pub action: SummaryRecoveryAction,
    pub now_ms: i64,
}
impl AutomationStore {
    pub async fn recover_summary<
        TTarget: Clone + Serialize + DeserializeOwned,
        TEndpoint: Serialize + DeserializeOwned,
        TGeneration: Serialize + DeserializeOwned,
        TReceipt: Serialize + DeserializeOwned,
    >(
        &mut self,
        request: &SummaryRecoveryRequest,
    ) -> Result<RunRecord<TTarget, TEndpoint, TGeneration, TReceipt>, StorageError> {
        if request.now_ms < 0 {
            return Err(StorageError::InvalidRecord);
        }
        let method = match request.action {
            SummaryRecoveryAction::Retry { .. } => "run/summaryRetry",
            SummaryRecoveryAction::Skip => "run/summarySkip",
        };
        // Configured timeout is admission context, not caller payload; replay keeps its original budget.
        let canonical =
            serde_json::to_vec(&request.run_id).map_err(|_| StorageError::InvalidRecord)?;
        let mut transaction = self.connection.begin_with("BEGIN IMMEDIATE").await?;
        if let Some(result) = local_operation_receipts::replay(
            &mut transaction,
            LocalOperation {
                id: &request.operation_id,
                method,
                canonical: &canonical,
            },
        )
        .await?
        {
            transaction.commit().await?;
            return Ok(result);
        }
        let mut record =
            crate::run_inspection::read_current::<TTarget, TEndpoint, TGeneration, TReceipt>(
                &mut transaction,
                &request.run_id,
            )
            .await?;
        if !matches!(
            record.phase,
            agent_automation::RunPhase::SummaryRequired
                | agent_automation::RunPhase::SummaryBlocked
        ) {
            return Err(StorageError::InvalidRecord);
        }
        let inventory = crate::run_inventory::load(&mut transaction, &record.schedule_id).await?;
        if inventory.occupying.as_ref() != Some(&request.run_id) {
            return Err(StorageError::InvalidRecord);
        }
        if let Some(attempt) = &record.summary_attempt {
            let not_submitted = matches!(
                attempt.effects.submission,
                agent_automation::SubmissionEffect::NotDispatched
                    | agent_automation::SubmissionEffect::Rejected
            ) && attempt.effects.allocation
                != agent_automation::PreparationEffect::Unknown
                && attempt.effects.resume != agent_automation::PreparationEffect::Unknown;
            if attempt.effects.cessation != agent_automation::CessationEvidence::Confirmed
                && !not_submitted
            {
                return Err(StorageError::InvalidRecord);
            }
            let event = serde_json::json!({"kind":"summaryAttempt","attempt":attempt});
            sqlx::query("INSERT INTO automation_events(event_id,subject_kind,subject_id,event_kind,event_body_json,recorded_at_ms) VALUES (?,'run',?,'summaryAttemptArchived',?,?)").bind(agent_automation::EventId::generate().as_str()).bind(request.run_id.as_str()).bind(event.to_string()).bind(request.now_ms).execute(&mut *transaction).await?;
        }
        match request.action {
            SummaryRecoveryAction::Retry { timeout_seconds } => {
                let attempt: agent_automation::SummaryAttempt<TTarget, TGeneration> =
                    crate::summary_admission::make_attempt(
                        crate::summary_admission::SummaryAttemptSeed {
                            source_target: record
                                .evidence
                                .native
                                .target
                                .clone()
                                .ok_or(StorageError::InvalidRecord)?,
                            source_turn_id: record
                                .native_turn_id
                                .clone()
                                .ok_or(StorageError::InvalidRecord)?,
                            timeout_seconds,
                            now_ms: request.now_ms,
                        },
                    )?;
                sqlx::query("UPDATE workflow_runs SET run_status='summaryRunning',summary_attempt_json=? WHERE run_id=?").bind(serde_json::to_string(&attempt).map_err(|_|StorageError::InvalidRecord)?).bind(request.run_id.as_str()).execute(&mut *transaction).await?;
            }
            SummaryRecoveryAction::Skip => {
                if let Some(attempt) = &mut record.summary_attempt {
                    attempt.phase = agent_automation::SummaryPhase::Skipped;
                    attempt.explanation =
                        Some("Summary intentionally omitted by explicit command.".into());
                }
                let source = agent_automation::SummarySource::<TTarget>::Skipped {
                    reason: "Summary intentionally omitted by explicit command.".into(),
                };
                sqlx::query("UPDATE workflow_runs SET run_status='finished',summary_attempt_json=?,summary_text=NULL,summary_source_json=?,completed_at_ms=? WHERE run_id=?").bind(record.summary_attempt.as_ref().map(serde_json::to_string).transpose().map_err(|_|StorageError::InvalidRecord)?).bind(serde_json::to_string(&source).map_err(|_|StorageError::InvalidRecord)?).bind(request.now_ms).bind(request.run_id.as_str()).execute(&mut *transaction).await?;
            }
        }
        let result =
            crate::run_inspection::read_current::<TTarget, TEndpoint, TGeneration, TReceipt>(
                &mut transaction,
                &request.run_id,
            )
            .await?;
        let encoded = serde_json::to_string(&result).map_err(|_| StorageError::InvalidRecord)?;
        local_operation_receipts::record(
            &mut transaction,
            CompletedLocalOperation {
                operation: LocalOperation {
                    id: &request.operation_id,
                    method,
                    canonical: &canonical,
                },
                resource_id: request.run_id.as_str(),
                result_json: &encoded,
                now_ms: request.now_ms,
            },
        )
        .await?;
        transaction.commit().await?;
        Ok(result)
    }
}

//! Export takes one local snapshot; native logs and live destination state are not copied.
use crate::{AutomationStore, StorageError};
use agent_automation::{
    InstructionText, PortableContinuity, PortableInstruction, PortableSchedulePackage, ScheduleId,
    ScheduleRecord,
};
use serde::de::DeserializeOwned;
use sqlx::{Connection, Row};
impl AutomationStore {
    pub async fn export_schedule<TTarget: DeserializeOwned, TEndpoint: DeserializeOwned>(
        &mut self,
        id: &ScheduleId,
    ) -> Result<PortableSchedulePackage<TTarget, TEndpoint>, StorageError> {
        let mut transaction = self.connection.begin().await?;
        let record: ScheduleRecord<TTarget, TEndpoint> =
            crate::schedule_inspection::read_current(&mut transaction, id).await?;
        let instruction=sqlx::query("SELECT current_revision_id,instruction_text FROM instruction_documents WHERE instruction_id=?").bind(record.definition.instruction_id.as_str()).fetch_one(&mut *transaction).await?;
        let prior=sqlx::query("SELECT run_id,summary_text,summary_source_json FROM workflow_runs WHERE schedule_id=? AND run_status='finished' ORDER BY completed_at_ms DESC,run_id DESC LIMIT 1").bind(id.as_str()).fetch_optional(&mut *transaction).await?;
        let continuity = if let Some(prior) = prior {
            match (
                prior.try_get::<Option<String>, _>("summary_text")?,
                prior.try_get::<Option<String>, _>("summary_source_json")?,
            ) {
                (Some(text), Some(source)) => {
                    match serde_json::from_str::<agent_automation::SummarySource<TTarget>>(&source)
                        .map_err(|_| StorageError::InvalidRecord)?
                    {
                        agent_automation::SummarySource::Completed { source_target, .. } => {
                            Some(PortableContinuity {
                                text: text.try_into().map_err(|_| StorageError::InvalidRecord)?,
                                source_run_id: prior.try_get("run_id")?,
                                source_target,
                            })
                        }
                        _ => return Err(StorageError::InvalidRecord),
                    }
                }
                (None, _) => None,
                _ => return Err(StorageError::InvalidRecord),
            }
        } else {
            match record.imported_continuity {
                agent_automation::ContinuityInput::ImportedSummary {
                    text,
                    source_run_id,
                    source_target,
                    ..
                } => Some(PortableContinuity {
                    text: text.try_into().map_err(|_| StorageError::InvalidRecord)?,
                    source_run_id,
                    source_target,
                }),
                agent_automation::ContinuityInput::None => None,
                _ => return Err(StorageError::InvalidRecord),
            }
        };
        let instruction = PortableInstruction {
            instruction_id: record.definition.instruction_id.clone(),
            text: InstructionText::try_from(instruction.try_get::<String, _>("instruction_text")?)
                .map_err(|_| StorageError::InvalidRecord)?,
            source_revision_id: instruction.try_get("current_revision_id")?,
        };
        transaction.commit().await?;
        Ok(PortableSchedulePackage {
            schedule_id: record.schedule_id,
            source_change_id: record.change_id.as_str().into(),
            instruction,
            definition: record.definition,
            continuity,
        })
    }
}

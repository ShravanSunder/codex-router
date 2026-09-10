//! Each collection keeps its existing typed record projection and native evidence validation.
use automation_storage::{AutomationCollection, AutomationStore, StorageError};
use communication_protocol::{CodexGeneration, EndpointRef, NativeSendReceipt, SessionRef};
use serde_json::Value;

pub(crate) async fn project_record(
    store: &mut AutomationStore,
    collection: &AutomationCollection,
    id: &str,
) -> Result<Value, StorageError> {
    let result = match collection {
        AutomationCollection::Instructions => {
            let id = id
                .to_owned()
                .try_into()
                .map_err(|_| StorageError::InvalidRecord)?;
            let record = store.read_instruction(&id).await?;
            serde_json::to_value(
                crate::instruction_dispatch::snapshot(record)
                    .map_err(|_| StorageError::InvalidRecord)?,
            )
        }
        AutomationCollection::Schedules => {
            let id = id
                .to_owned()
                .try_into()
                .map_err(|_| StorageError::InvalidRecord)?;
            let record = store
                .inspect_schedule::<SessionRef, EndpointRef>(&id)
                .await?;
            serde_json::to_value(
                crate::schedule_projection::snapshot(record)
                    .map_err(|_| StorageError::InvalidRecord)?,
            )
        }
        AutomationCollection::Runs(schedule_id) => {
            let id = id
                .to_owned()
                .try_into()
                .map_err(|_| StorageError::InvalidRecord)?;
            let record = store
                .read_run::<SessionRef, EndpointRef, CodexGeneration, NativeSendReceipt>(&id)
                .await?;
            if record.schedule_id != *schedule_id {
                return Err(StorageError::InvalidRecord);
            }
            serde_json::to_value(
                crate::run_projection::snapshot(record).map_err(|_| StorageError::InvalidRecord)?,
            )
        }
        AutomationCollection::Deliveries(wakeup_id) => {
            let id = id
                .to_owned()
                .try_into()
                .map_err(|_| StorageError::InvalidRecord)?;
            let record = store
                .read_delivery::<SessionRef, CodexGeneration, NativeSendReceipt>(&id)
                .await?;
            if wakeup_id.as_ref().is_some_and(|id| *id != record.wakeup_id) {
                return Err(StorageError::InvalidRecord);
            }
            serde_json::to_value(
                crate::delivery_projection::snapshot(record)
                    .map_err(|_| StorageError::InvalidRecord)?,
            )
        }
        AutomationCollection::Revisions(instruction_id) => {
            let id = id
                .to_owned()
                .try_into()
                .map_err(|_| StorageError::InvalidRecord)?;
            let record = store.read_instruction_revision(&id).await?;
            if record.instruction_id != *instruction_id {
                return Err(StorageError::InvalidRecord);
            }
            serde_json::to_value(communication_protocol::RevisionRecord {
                instruction_id: record.instruction_id,
                revision_id: record.revision_id,
                text: record.text.as_str().into(),
                source_revision_id: record.source_revision_id,
                recorded_at: timestamp(record.recorded_at_ms)?,
            })
        }
    };
    result.map_err(|_| StorageError::InvalidRecord)
}
fn timestamp(value: i64) -> Result<communication_protocol::ObservationTimestamp, StorageError> {
    crate::wakeup_projection::timestamp(value).map_err(|_| StorageError::InvalidRecord)
}

//! Read-only admission facts; native capability policy remains in the service adapter.
use crate::{AutomationStore, StorageError};
use agent_automation::{OperationId, ScheduleId};
pub struct BindingAddress<'a> {
    pub schedule_id: &'a ScheduleId,
    pub service_id: &'a str,
    pub endpoint_id: &'a str,
    pub thread_id: &'a str,
}
impl AutomationStore {
    pub async fn has_operation_receipt(&mut self, id: &OperationId) -> Result<bool, StorageError> {
        let exists: i64 = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM operation_receipts WHERE operation_id=?)",
        )
        .bind(id.as_str())
        .fetch_one(&mut self.connection)
        .await?;
        Ok(exists != 0)
    }
    pub async fn owns_thread_address(
        &mut self,
        address: BindingAddress<'_>,
    ) -> Result<bool, StorageError> {
        let exists:i64=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM thread_bindings WHERE schedule_id=? AND service_id=? AND endpoint_id=? AND thread_id=?)").bind(address.schedule_id.as_str()).bind(address.service_id).bind(address.endpoint_id).bind(address.thread_id).fetch_one(&mut self.connection).await?;
        Ok(exists != 0)
    }
}

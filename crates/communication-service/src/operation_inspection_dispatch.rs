//! Durable command inspection reports original outcomes without repeating any mutation.
use crate::automation_inspection_failure as failure;
use automation_storage::{AutomationStore, StorageError};
use communication_protocol::{OperationShowRequest, UuidIdentity};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::Mutex;
pub(crate) struct OperationRequest<'a> {
    pub reconcile: bool,
    pub configuration_backend: Option<&'a Arc<dyn crate::AutomationConfigurationBackend>>,
    pub id: Value,
    pub params: Value,
    pub service_id: &'a UuidIdentity,
    pub store: Option<&'a Arc<Mutex<AutomationStore>>>,
}
pub(crate) async fn dispatch(request: OperationRequest<'_>) -> Value {
    let Some(store) = request.store else {
        return failure::response(request.id, failure::unavailable());
    };
    let params: OperationShowRequest = match serde_json::from_value(request.params) {
        Ok(params) => params,
        Err(_) => {
            return failure::response(
                request.id,
                failure::invalid(
                    "operationId",
                    "Provide an exact UUIDv7 operation identity with no additional fields.",
                ),
            );
        }
    };
    let mut record = match store
        .lock()
        .await
        .read_operation(&params.operation_id)
        .await
    {
        Ok(record) => record,
        Err(error) => {
            let mut error = failure::storage(error);
            error.resource_id = Some(params.operation_id.as_str().into());
            return failure::response(request.id, error);
        }
    };
    let mut explanation = None;
    if request.reconcile
        && record.method == "automation/configure"
        && !matches!(
            record.state,
            automation_storage::StoredOperationState::Succeeded { .. }
                | automation_storage::StoredOperationState::Failed { .. }
        )
    {
        if let Some(backend) = request.configuration_backend {
            if let Err(error) = backend.reconcile(params.operation_id.clone()).await {
                explanation = Some(error.message);
            }
            record = match store
                .lock()
                .await
                .read_operation(&params.operation_id)
                .await
            {
                Ok(record) => record,
                Err(error) => return failure::response(request.id, failure::storage(error)),
            };
        } else {
            explanation = Some("The Host configuration backend is unavailable. Reconnect to the owning Host and reconcile this operation ID; no native work was resubmitted.".into());
        }
    }
    let mut snapshot =
        match crate::operation_receipt_projection::snapshot(record, request.service_id) {
            Ok(snapshot) => snapshot,
            Err(error) => return failure::response(request.id, failure::storage(error)),
        };
    if let (Some(message), communication_protocol::OperationState::Uncertain { explanation, .. }) =
        (explanation, &mut snapshot.state)
    {
        *explanation = message;
    }
    let response = json!({"jsonrpc":"2.0","id":request.id,"result":snapshot});
    match serde_json::to_vec(&response) {
        Ok(bytes) if bytes.len() <= communication_protocol::MAX_CONTROL_FRAME_BYTES => response,
        Ok(_) => {
            let mut error = failure::invalid(
                "operationId",
                "Original receipt exceeds the Control frame budget; inspect the affected resource. No request was resubmitted.",
            );
            error.resource_id = Some(snapshot.resource_id);
            failure::response(request.id, error)
        }
        Err(_) => failure::response(request.id, failure::storage(StorageError::InvalidRecord)),
    }
}

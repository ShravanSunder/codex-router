//! Durable operation inspection Control dispatch over the typed automation inspection
//! operations; it reports original outcomes without repeating any mutation.
use crate::ServiceIdentity;
use crate::automation_inspection_failure as failure;
use crate::collaboration_application::AutomationOperations;
use collaboration_protocol::OperationShowRequest;
use serde_json::{Value, json};

pub(crate) async fn dispatch(
    id: Value,
    reconcile: bool,
    params: Value,
    identity: &ServiceIdentity,
) -> Value {
    let Ok(request) = serde_json::from_value::<OperationShowRequest>(params) else {
        return failure::response(
            id,
            failure::invalid(
                "operationId",
                "Provide an exact UUIDv7 operation identity with no additional fields.",
            ),
        );
    };
    let budget = crate::control_connection::control_result_budget(&id);
    let automation = AutomationOperations::new(identity);
    let result = if reconcile {
        automation.operation_reconcile(request, budget).await
    } else {
        automation.operation_show(request, budget).await
    };
    match result {
        Ok(snapshot) => json!({"jsonrpc":"2.0","id":id,"result":snapshot}),
        Err(error) => failure::response(id, error),
    }
}

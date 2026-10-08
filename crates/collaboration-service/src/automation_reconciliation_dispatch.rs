//! Reconciliation Control dispatch over the typed automation inspection operations; it reads
//! exact native evidence and never repeats a native mutation.
use crate::ServiceIdentity;
use crate::automation_inspection_failure as failure;
use crate::collaboration_application::AutomationOperations;
use collaboration_protocol::{DeliveryShowRequest, RunShowRequest};
use serde_json::{Value, json};

pub(crate) async fn delivery(id: Value, params: Value, identity: &ServiceIdentity) -> Value {
    let Ok(request) = serde_json::from_value::<DeliveryShowRequest>(params) else {
        return failure::response(
            id,
            failure::invalid(
                "deliveryId",
                "Provide the exact UUIDv7 delivery identity; reconciliation never resends input.",
            ),
        );
    };
    let budget = crate::control_connection::control_result_budget(&id);
    match AutomationOperations::new(identity)
        .delivery_reconcile(request, budget)
        .await
    {
        Ok(snapshot) => json!({"jsonrpc":"2.0","id":id,"result":snapshot}),
        Err(error) => failure::response(id, error),
    }
}
pub(crate) async fn run(id: Value, params: Value, identity: &ServiceIdentity) -> Value {
    let Ok(request) = serde_json::from_value::<RunShowRequest>(params) else {
        return failure::response(
            id,
            failure::invalid(
                "runId",
                "Provide the exact UUIDv7 Run identity; reconciliation never starts or interrupts work.",
            ),
        );
    };
    let budget = crate::control_connection::control_result_budget(&id);
    match AutomationOperations::new(identity)
        .run_reconcile(request, budget)
        .await
    {
        Ok(snapshot) => json!({"jsonrpc":"2.0","id":id,"result":snapshot}),
        Err(error) => failure::response(id, error),
    }
}

//! Attempt history Control dispatch over the typed automation inspection operations.
use crate::ServiceIdentity;
use crate::automation_inspection_failure as failure;
use crate::collaboration_application::AutomationOperations;
use collaboration_protocol::{DeliveryAttemptsRequest, RunSummariesRequest};
use serde_json::{Value, json};

pub(crate) async fn dispatch(
    id: Value,
    method: &str,
    params: Value,
    identity: &ServiceIdentity,
) -> Value {
    let automation = AutomationOperations::new(identity);
    let budget = crate::control_connection::control_result_budget(&id);
    let invalid = || {
        failure::invalid(
            "request",
            "Provide the exact owning resource, nullable cursor and limit 1..100.",
        )
    };
    let result = match method {
        "delivery/attempts" => {
            let Ok(request) = serde_json::from_value::<DeliveryAttemptsRequest>(params) else {
                return failure::response(id, invalid());
            };
            automation
                .delivery_attempts(request, budget)
                .await
                .map(|page| json!(page))
        }
        "run/summaries" => {
            let Ok(request) = serde_json::from_value::<RunSummariesRequest>(params) else {
                return failure::response(id, invalid());
            };
            automation
                .run_summaries(request, budget)
                .await
                .map(|page| json!(page))
        }
        _ => return failure::response(id, invalid()),
    };
    match result {
        Ok(page) => json!({"jsonrpc":"2.0","id":id,"result":page}),
        Err(error) => failure::response(id, error),
    }
}

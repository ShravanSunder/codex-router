//! Automation collection Control dispatch over the typed automation inspection operations.
use crate::ServiceIdentity;
use crate::automation_inspection_failure as failure;
use crate::collaboration_application::AutomationOperations;
use collaboration_protocol::{
    AutomationPageRequest, DeliveryListRequest, RevisionListRequest, RunListRequest,
};
use serde_json::{Value, json};

pub(crate) async fn dispatch(
    id: Value,
    method: &str,
    params: Value,
    identity: &ServiceIdentity,
) -> Value {
    let automation = AutomationOperations::new(identity);
    let budget = crate::control_connection::control_result_budget(&id);
    macro_rules! list {
        ($request:ty, $operation:ident) => {{
            let Ok(request) = serde_json::from_value::<$request>(params) else {
                return failure::response(
                    id,
                    failure::invalid(
                        "request",
                        "Use the published closed fields, nullable cursor and limit 1..100.",
                    ),
                );
            };
            automation
                .$operation(request, budget)
                .await
                .map(|page| json!(page))
        }};
    }
    let result = match method {
        "instruction/list" => list!(AutomationPageRequest, instruction_list),
        "schedule/list" => list!(AutomationPageRequest, schedule_list),
        "run/list" => list!(RunListRequest, run_list),
        "revision/list" => list!(RevisionListRequest, revision_list),
        "delivery/list" => list!(DeliveryListRequest, delivery_list),
        _ => {
            return failure::response(
                id,
                failure::invalid(
                    "request",
                    "Use the published closed fields, nullable cursor and limit 1..100.",
                ),
            );
        }
    };
    match result {
        Ok(page) => json!({"jsonrpc":"2.0","id":id,"result":page}),
        Err(error) => failure::response(id, error),
    }
}

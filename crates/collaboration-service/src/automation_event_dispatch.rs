//! Automation event history Control dispatch over the typed automation inspection operations.
use crate::ServiceIdentity;
use crate::automation_inspection_failure as failure;
use crate::collaboration_application::AutomationOperations;
use collaboration_protocol::AutomationEventsRequest;
use serde_json::{Value, json};

pub(crate) async fn dispatch(id: Value, params: Value, identity: &ServiceIdentity) -> Value {
    let Ok(request) = serde_json::from_value::<AutomationEventsRequest>(params) else {
        return failure::response(
            id,
            failure::invalid(
                "request",
                "Provide nullable after and a limit from 1 through 100.",
            ),
        );
    };
    let budget = crate::control_connection::control_result_budget(&id);
    match AutomationOperations::new(identity)
        .automation_events(request, budget)
        .await
    {
        Ok(page) => json!({"jsonrpc":"2.0","id":id,"result":page}),
        Err(error) => failure::response(id, error),
    }
}

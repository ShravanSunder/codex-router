//! Run Control dispatch: inspection and explicit same-Run summary recovery over the typed
//! automation operations; no automatic native resend.
use crate::ServiceIdentity;
use crate::collaboration_application::{AutomationOperations, run_failure_context};
use collaboration_protocol::{RunFailure, RunRecoveryRequest, RunShowRequest};
use serde_json::{Value, json};
pub(crate) async fn dispatch(
    id: Value,
    method: &str,
    params: Value,
    identity: &ServiceIdentity,
) -> Value {
    let context = run_failure_context(
        params
            .get("operationId")
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok()),
        params
            .get("runId")
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok()),
    );
    let automation = AutomationOperations::new(identity);
    let result = match method {
        "run/show" => {
            let Ok(params) = serde_json::from_value::<RunShowRequest>(params) else {
                return failure(id, context);
            };
            automation.run_show(params).await
        }
        "run/summaryRetry" | "run/summarySkip" => {
            let Ok(params) = serde_json::from_value::<RunRecoveryRequest>(params) else {
                return failure(id, context);
            };
            if method == "run/summaryRetry" {
                automation.run_summary_retry(params).await
            } else {
                automation.run_summary_skip(params).await
            }
        }
        _ => return failure(id, context),
    };
    match result {
        Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
        Err(data) => failure(id, data),
    }
}
fn failure(id: Value, data: RunFailure) -> Value {
    crate::control_connection::rejection_response(id, &data)
}

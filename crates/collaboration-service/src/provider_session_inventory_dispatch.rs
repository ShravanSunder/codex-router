//! Provider session inventory Control dispatch over the typed session operations.
use crate::ServiceIdentity;
use crate::collaboration_application::{ProviderInventoryFailure, SessionOperations};
use collaboration_protocol::ProviderSessionListParams;
use serde_json::{Value, json};

pub(crate) async fn dispatch(id: Value, params: Value, identity: &ServiceIdentity) -> Value {
    let Ok(params) = serde_json::from_value::<ProviderSessionListParams>(params) else {
        return crate::control_connection::rejection_response(
            id,
            &ProviderInventoryFailure::InvalidRequest(
                "Invalid provider session inventory parameters",
            ),
        );
    };
    let budget = crate::control_connection::control_result_budget(&id);
    match SessionOperations::new(identity)
        .provider_session_list(params, budget)
        .await
    {
        Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
        Err(failure) => crate::control_connection::rejection_response(id, &failure),
    }
}

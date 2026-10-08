//! Wake listing Control dispatch: pagination binds its opaque cursor to the selected service
//! and collection.
use crate::collaboration_application::{WakeOperations, wake_invalid_field};
use crate::wakeup_dispatch::{WakeRequest, failure, failure_context, wake_response};
use collaboration_protocol::{AutomationPageRequest, LocalMutationState};
use serde_json::Value;

pub(crate) async fn dispatch(request: WakeRequest<'_>) -> Value {
    let Ok(params) = serde_json::from_value::<AutomationPageRequest>(request.params.clone()) else {
        return failure(
            request.id,
            failure_context(&request.params),
            wake_invalid_field(
                "request",
                "Provide nullable cursor and a limit between1 and100.",
            ),
            LocalMutationState::None,
        );
    };
    let budget = crate::control_connection::control_result_budget(&request.id);
    let result = WakeOperations::new(request.service_id, request.store)
        .wake_list(params, budget)
        .await;
    wake_response(request.id, result)
}

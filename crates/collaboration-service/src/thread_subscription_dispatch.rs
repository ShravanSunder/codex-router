//! Thread and Topic subscription Control dispatch over the typed board operations.
use crate::ServiceIdentity;
use crate::collaboration_application::{BoardOperations, ThreadWaitBudgetError};
#[cfg(test)]
use collaboration_protocol::{MAX_CONTROL_FRAME_BYTES, MAX_PUSH_LINE_BYTES};
use collaboration_protocol::{
    ThreadSubscribeRequest, ThreadSubscriptionWaitRequest, ThreadSubscriptionsRequest,
    ThreadUnsubscribeRequest,
};
use message_board::BoardError;
use serde_json::{Value, json};

pub(crate) async fn dispatch(
    id: Value,
    method: &str,
    params: Value,
    identity: &ServiceIdentity,
) -> Value {
    let board = BoardOperations::new(identity);
    macro_rules! decode {
        ($request:ty) => {
            match serde_json::from_value::<$request>(params.clone()) {
                Ok(request) => request,
                Err(_) => {
                    return crate::board_request_dispatch::failure(
                        id,
                        crate::board_request_validation::classify(method, &params),
                    );
                }
            }
        };
    }
    let result = match method {
        "board/threadSubscribe" => board
            .thread_subscribe(decode!(ThreadSubscribeRequest))
            .await
            .map(|view| json!(view)),
        "board/threadUnsubscribe" => board
            .thread_unsubscribe(decode!(ThreadUnsubscribeRequest))
            .await
            .map(|view| json!(view)),
        "board/threadSubscriptions" => board
            .thread_subscriptions(decode!(ThreadSubscriptionsRequest))
            .await
            .map(|result| json!(result)),
        "board/threadWait" => {
            let request = decode!(ThreadSubscriptionWaitRequest);
            let maximum_root_notice_bytes = match maximum_root_notice_bytes(&id) {
                Ok(maximum) => maximum,
                Err(error) => return crate::board_request_dispatch::failure(id, error),
            };
            board
                .thread_wait(request, maximum_root_notice_bytes)
                .await
                .map(|result| json!(result))
        }
        _ => {
            return json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Unknown thread subscription operation"}});
        }
    };
    match result {
        Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
        Err(error) => crate::board_request_dispatch::failure(id, error),
    }
}

fn maximum_root_notice_bytes(id: &Value) -> Result<usize, BoardError> {
    let budget = crate::control_connection::control_result_budget(id);
    crate::collaboration_application::thread_wait_root_notice_limit(budget).map_err(|error| {
        match error {
            ThreadWaitBudgetError::NoRoomForResponse => BoardError::invalid_field(
                "id",
                "leaves no room for a subscription wait response in a Control frame",
            ),
            ThreadWaitBudgetError::NoRoomForOneRoot => BoardError::invalid_field(
                "id",
                "leaves no room for one subscription root in a Control frame",
            ),
        }
    })
}

#[cfg(test)]
#[path = "thread_subscription_dispatch_tests.rs"]
mod tests;

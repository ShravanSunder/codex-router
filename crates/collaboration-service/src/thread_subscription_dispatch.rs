//! Thread and Topic subscription Control dispatch over the typed board operations.
use crate::ServiceIdentity;
use crate::collaboration_application::BoardOperations;
use collaboration_protocol::{
    MAX_CONTROL_FRAME_BYTES, MAX_PUSH_LINE_BYTES, ThreadSubscribeRequest,
    ThreadSubscriptionWaitRequest, ThreadSubscriptionsRequest, ThreadUnsubscribeRequest,
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
    const MAX_HELD_SINCE_WIRE_BYTES: usize = 64;
    let empty_roots = json!([]);
    let empty_roots_bytes =
        serde_json::to_vec(&empty_roots).map_err(|_| BoardError::board_unavailable())?;
    let notice_response = json!({
        "jsonrpc":"2.0",
        "id":id,
        "result":{
            "batch":{
                "kind":"notice",
                "pushId":"01890f2e-7b4c-7cc0-98c4-000000000002",
                "line":"\\".repeat(MAX_PUSH_LINE_BYTES),
                "held":true,
                "heldSince":"x".repeat(MAX_HELD_SINCE_WIRE_BYTES),
                "draining":true,
                "roots":empty_roots,
            }
        }
    });
    let fixed_response_bytes = serde_json::to_vec(&notice_response)
        .map_err(|_| BoardError::board_unavailable())?
        .len()
        .checked_sub(empty_roots_bytes.len())
        .ok_or_else(BoardError::board_unavailable)?;
    let maximum_root_notice_bytes = MAX_CONTROL_FRAME_BYTES
        .checked_sub(fixed_response_bytes)
        .ok_or_else(|| {
            BoardError::invalid_field(
                "id",
                "leaves no room for a subscription wait response in a Control frame",
            )
        })?;
    let largest_root = json!([{
        "rootId":"01890f2e-7b4c-7cc0-98c4-000000000002",
        "topicId":"01890f2e-7b4c-7cc0-98c4-000000000003",
        "fromSequence":i64::MAX,
        "throughSequence":i64::MAX,
        "messageCount":u64::MAX,
    }]);
    let minimum_root_notice_bytes = serde_json::to_vec(&largest_root)
        .map_err(|_| BoardError::board_unavailable())?
        .len();
    if maximum_root_notice_bytes < minimum_root_notice_bytes {
        return Err(BoardError::invalid_field(
            "id",
            "leaves no room for one subscription root in a Control frame",
        ));
    }
    Ok(maximum_root_notice_bytes)
}

#[cfg(test)]
#[path = "thread_subscription_dispatch_tests.rs"]
mod tests;

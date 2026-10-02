//! Control handlers for durable Thread and Topic subscriptions.
use crate::ServiceIdentity;
use collaboration_protocol::{
    MAX_CONTROL_FRAME_BYTES, MAX_PUSH_LINE_BYTES, SubscriptionWaitBatch, ThreadSubscribeRequest,
    ThreadSubscriptionPresence, ThreadSubscriptionState, ThreadSubscriptionView,
    ThreadSubscriptionWaitFilter as ControlWaitFilter, ThreadSubscriptionWaitRequest,
    ThreadSubscriptionWaitResult, ThreadSubscriptionsRequest, ThreadSubscriptionsResult,
    ThreadUnsubscribeRequest,
};
use message_board::{
    BoardError, BoardFailureKind, BoardFailureStage, BoardNextAction, Identity, SubscriptionState,
    ThreadSubscriptionRecord, ThreadSubscriptionSubscribeRequest,
    ThreadSubscriptionUnsubscribeRequest,
};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};

const TARGET_PRESENCE_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) async fn dispatch(
    id: Value,
    method: &str,
    params: Value,
    identity: &ServiceIdentity,
) -> Value {
    let Some(store) = identity.board.as_ref() else {
        return crate::board_request_dispatch::unavailable(id);
    };
    let Some(service) = identity.subscription_delivery.as_ref() else {
        return crate::board_request_dispatch::failure(id, BoardError::board_unavailable());
    };
    let Some(presence_probe) = identity.subscription_presence.as_ref() else {
        return crate::board_request_dispatch::failure(id, BoardError::board_unavailable());
    };

    match method {
        "board/threadSubscribe" => {
            let request = match serde_json::from_value::<ThreadSubscribeRequest>(params.clone()) {
                Ok(request) => request,
                Err(_) => {
                    return crate::board_request_dispatch::failure(
                        id,
                        crate::board_request_validation::classify(method, &params),
                    );
                }
            };
            let reader = request.actor.clone();
            let result = store
                .lock()
                .await
                .subscribe_thread_subscription(
                    ThreadSubscriptionSubscribeRequest {
                        reader: request.actor,
                        scope: request.scope,
                        policy: request.policy,
                    },
                    identity.subscription_clock.now(),
                )
                .await;
            match result {
                Ok(record) => {
                    if let Err(error) = service.reconcile_reader(reader.clone()).await {
                        tracing::warn!(error=%error,"subscription owner reconciliation failed after subscribe");
                        return crate::board_request_dispatch::failure(
                            id,
                            owner_reconciliation_failed(),
                        );
                    }
                    let presence = presence_for_reader(&reader, presence_probe).await;
                    let view = subscription_view(record, presence);
                    json!({"jsonrpc":"2.0","id":id,"result":view})
                }
                Err(error) => crate::board_request_dispatch::failure(id, error),
            }
        }
        "board/threadUnsubscribe" => {
            let request = match serde_json::from_value::<ThreadUnsubscribeRequest>(params.clone()) {
                Ok(request) => request,
                Err(_) => {
                    return crate::board_request_dispatch::failure(
                        id,
                        crate::board_request_validation::classify(method, &params),
                    );
                }
            };
            let reader = request.actor.clone();
            let result = store
                .lock()
                .await
                .unsubscribe_thread_subscription(
                    ThreadSubscriptionUnsubscribeRequest {
                        reader: request.actor,
                        scope: request.scope,
                    },
                    identity.subscription_clock.now(),
                )
                .await;
            match result {
                Ok(record) => {
                    if let Err(error) = service.reconcile_reader(reader.clone()).await {
                        tracing::warn!(error=%error,"subscription owner reconciliation failed after unsubscribe");
                        return crate::board_request_dispatch::failure(
                            id,
                            owner_reconciliation_failed(),
                        );
                    }
                    let presence = presence_for_reader(&reader, presence_probe).await;
                    let view = subscription_view(record, presence);
                    json!({"jsonrpc":"2.0","id":id,"result":view})
                }
                Err(error) => crate::board_request_dispatch::failure(id, error),
            }
        }
        "board/threadSubscriptions" => {
            let request = match serde_json::from_value::<ThreadSubscriptionsRequest>(params.clone())
            {
                Ok(request) => request,
                Err(_) => {
                    return crate::board_request_dispatch::failure(
                        id,
                        crate::board_request_validation::classify(method, &params),
                    );
                }
            };
            let records = match store
                .lock()
                .await
                .list_reader_subscriptions(&request.actor, identity.subscription_clock.now())
                .await
            {
                Ok(records) => records,
                Err(error) => return crate::board_request_dispatch::failure(id, error),
            };
            if let Err(error) = service.reconcile_reader(request.actor.clone()).await {
                tracing::warn!(error=%error,"subscription owner reconciliation failed after listing subscriptions");
            }
            let presence = presence_for_reader(&request.actor, presence_probe).await;
            let subscriptions = records
                .into_iter()
                .map(|record| subscription_view(record, presence.clone()))
                .collect();
            json!({
                "jsonrpc":"2.0",
                "id":id,
                "result":ThreadSubscriptionsResult { subscriptions },
            })
        }
        "board/threadWait" => {
            let request =
                match serde_json::from_value::<ThreadSubscriptionWaitRequest>(params.clone()) {
                    Ok(request) => request,
                    Err(_) => {
                        return crate::board_request_dispatch::failure(
                            id,
                            crate::board_request_validation::classify(method, &params),
                        );
                    }
                };
            let maximum_root_notice_bytes = match maximum_root_notice_bytes(&id) {
                Ok(maximum) => maximum,
                Err(error) => return crate::board_request_dispatch::failure(id, error),
            };
            let filter = match request.filter {
                ControlWaitFilter::All {} => crate::SubscriptionWaitFilter::All,
                ControlWaitFilter::Roots { root_message_ids } => {
                    crate::SubscriptionWaitFilter::Roots(root_message_ids)
                }
                ControlWaitFilter::Topic { topic_id } => {
                    crate::SubscriptionWaitFilter::Topic(topic_id)
                }
            };
            match service
                .wait(
                    request.actor,
                    filter,
                    request.max_wait_seconds,
                    maximum_root_notice_bytes,
                )
                .await
            {
                Ok(result) => json!({
                    "jsonrpc":"2.0",
                    "id":id,
                    "result":subscription_wait_result(result),
                }),
                Err(error) => crate::board_request_dispatch::failure(id, error),
            }
        }
        _ => {
            json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Unknown thread subscription operation"}})
        }
    }
}

fn subscription_wait_result(
    result: Option<crate::SubscriptionWaitResult>,
) -> ThreadSubscriptionWaitResult {
    let batch = result.map(|result| match result {
        crate::SubscriptionWaitResult::Notice {
            push_id,
            line,
            batch,
        } => SubscriptionWaitBatch::Notice {
            push_id,
            line,
            held: batch.held,
            held_since: batch.held_since,
            draining: batch.draining,
            roots: batch.roots,
        },
        crate::SubscriptionWaitResult::Ranges { batch } => SubscriptionWaitBatch::Ranges {
            held: batch.held,
            held_since: batch.held_since,
            draining: batch.draining,
            roots: batch.roots,
        },
    });
    ThreadSubscriptionWaitResult { batch }
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

fn subscription_view(
    record: ThreadSubscriptionRecord,
    presence: ThreadSubscriptionPresence,
) -> ThreadSubscriptionView {
    let state = match record.state() {
        SubscriptionState::Active => ThreadSubscriptionState::Active,
        SubscriptionState::Draining => ThreadSubscriptionState::Draining,
        SubscriptionState::Ended { .. } => ThreadSubscriptionState::Ended,
    };
    let pending_count = record.roots().iter().map(|root| root.pending_count()).sum();
    let held_since = record
        .roots()
        .iter()
        .filter_map(|root| root.held_since())
        .min();
    let next_retry_at = record
        .roots()
        .iter()
        .filter_map(|root| root.next_retry_at())
        .min();
    ThreadSubscriptionView {
        scope: record.scope().clone(),
        policy: record.policy().clone(),
        state,
        end_reason: record.state().end_reason(),
        expires_at: record.expires_at(),
        pending_count,
        presence,
        held_since,
        next_retry_at,
        last_outcome: record.last_outcome().cloned(),
    }
}

async fn presence_for_reader(
    reader: &Identity,
    presence_probe: &Arc<dyn crate::TargetPresenceProbe>,
) -> ThreadSubscriptionPresence {
    let Identity::Session { session } = reader else {
        return ThreadSubscriptionPresence::Unreachable {
            reason: "no session target".to_owned(),
        };
    };
    let target = match serde_json::to_value(session)
        .and_then(serde_json::from_value::<collaboration_protocol::SessionRef>)
    {
        Ok(target) => target,
        Err(error) => {
            tracing::warn!(error=%error,"subscription target identity could not be projected for presence");
            return ThreadSubscriptionPresence::Unreachable {
                reason: "presence unavailable".to_owned(),
            };
        }
    };
    match tokio::time::timeout(
        TARGET_PRESENCE_PROBE_TIMEOUT,
        presence_probe.presence(&target),
    )
    .await
    {
        Ok(Ok(crate::TargetPresence::Running)) => ThreadSubscriptionPresence::Running {},
        Ok(Ok(crate::TargetPresence::Wakeable)) => ThreadSubscriptionPresence::Wakeable {},
        Ok(Ok(crate::TargetPresence::Unreachable { reason })) => {
            ThreadSubscriptionPresence::Unreachable { reason }
        }
        Ok(Err(error)) => {
            tracing::warn!(error=%error,"subscription target presence probe failed");
            ThreadSubscriptionPresence::Unreachable {
                reason: "presence unavailable".to_owned(),
            }
        }
        Err(_) => {
            tracing::warn!(
                timeout_seconds = TARGET_PRESENCE_PROBE_TIMEOUT.as_secs(),
                "subscription target presence probe timed out"
            );
            ThreadSubscriptionPresence::Unreachable {
                reason: "presence unavailable".to_owned(),
            }
        }
    }
}

fn owner_reconciliation_failed() -> BoardError {
    BoardError {
        kind: BoardFailureKind::OutcomeUnknown,
        stage: BoardFailureStage::Storage,
        message: "The subscription change was saved, but its delivery owner could not synchronize. Inspect the current subscription state before retrying.".to_owned(),
        next_action: BoardNextAction::RetryLater,
        details: message_board::BoardErrorDetails::None,
    }
}

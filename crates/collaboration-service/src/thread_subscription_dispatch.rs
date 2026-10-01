//! Control handlers for durable Thread and Topic subscriptions.
use crate::ServiceIdentity;
use collaboration_protocol::{
    ThreadSubscribeRequest, ThreadSubscriptionPresence, ThreadSubscriptionState,
    ThreadSubscriptionView, ThreadSubscriptionsRequest, ThreadSubscriptionsResult,
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
        _ => {
            json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Unknown thread subscription operation"}})
        }
    }
}

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

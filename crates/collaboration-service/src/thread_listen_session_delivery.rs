//! Background delivery, retry, and finalization for session-owned Thread Listens.
use super::{
    ThreadListenRegistry, ThreadListenState, publish_rejection_state, record_batch_progress,
    record_rejection,
};
use message_board::*;
use message_board_storage::BoardStore;
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::Instant;

pub(super) async fn run(
    registry: ThreadListenRegistry,
    listen_id: ListenId,
    store: Arc<Mutex<BoardStore>>,
    sink: Arc<dyn BatchSink>,
) {
    let Ok(state) = registry.state(&listen_id).await else {
        return;
    };
    let mark_total = match state.mode {
        ThreadListenMode::Once { .. } => 1_u8,
        ThreadListenMode::Repeating { lifetime_seconds }
            if lifetime_seconds > ThreadListenLifetime::Short.seconds() =>
        {
            3
        }
        ThreadListenMode::Repeating { .. } => 1,
    };
    let mut next_mark = Instant::now() + THREAD_LISTEN_MARK;
    let mut mark = 1_u8;
    let mut delivered_since_mark = false;
    let mut consecutive_rejections = 0_u8;
    let mut last_rejection = Value::Null;
    let mut wait = spawn_wait(&registry, &listen_id, &store);
    loop {
        tokio::select! {
            result = &mut wait => {
                let result = match result {
                    Ok(Ok(result)) => result,
                    _ => {
                        finish_session_delivery(&registry, &listen_id, &state, &sink, ThreadListenEndReason::Error, json!({"kind":"unavailable"})).await;
                        return;
                    }
                };
                if let Some(batch_set) = result.batch_set {
                    let mut batch_delivered = false;
                    match sink.deliver(ListenDeliveryRecord::Batch(batch_set.clone())).await {
                        Ok(()) => {
                            if store
                                .lock()
                                .await
                                .record_thread_listen_batch_delivery(&state.context, &batch_set)
                                .await
                                .is_err()
                            {
                                finish_session_delivery(
                                    &registry,
                                    &listen_id,
                                    &state,
                                    &sink,
                                    ThreadListenEndReason::Error,
                                    json!({"kind":"positionPersistenceFailed"}),
                                ).await;
                                return;
                            }
                            record_batch_progress(&state, &batch_set);
                            batch_delivered = true;
                            delivered_since_mark = true;
                            consecutive_rejections = 0;
                            publish_rejection_state(&state, consecutive_rejections, &last_rejection);
                            if state.acknowledge {
                                for batch in &batch_set.batches {
                                    if store.lock().await.acknowledge_inbox(InboxAcknowledgeRequest {
                                        actor: state.context.reader.clone(),
                                        acting_for: None,
                                        scope: ReadScope::Thread { root_message_id: batch.root_message_id.clone() },
                                        through_activity_sequence: batch.delivered_through,
                                    }).await.is_err() {
                                        finish_session_delivery(&registry, &listen_id, &state, &sink, ThreadListenEndReason::Error, json!({"kind":"acknowledgementFailed"})).await;
                                        return;
                                    }
                                }
                                state.acknowledged.store(true, std::sync::atomic::Ordering::Relaxed);
                            }
                        }
                        Err(BatchSinkFailure::Rejected { evidence }) => {
                            let terminal = record_rejection(&mut consecutive_rejections, &mut last_rejection, evidence);
                            publish_rejection_state(&state, consecutive_rejections, &last_rejection);
                            if terminal {
                                finish_session_delivery(&registry, &listen_id, &state, &sink, ThreadListenEndReason::Error, last_rejection).await;
                                return;
                            }
                        }
                        Err(BatchSinkFailure::Unavailable) => {
                            // Keep the selected batch pending. The next wait observes the same
                            // undelivered activity and retries it under the listen policy.
                        }
                    }
                    if batch_delivered && matches!(state.mode, ThreadListenMode::Once { .. }) {
                        finish_session_delivery(&registry, &listen_id, &state, &sink, ThreadListenEndReason::Emitted, Value::Null).await;
                        return;
                    }
                }
                if let Some(end) = result.end {
                    finish_session_delivery(&registry, &listen_id, &state, &sink, end.reason, Value::Null).await;
                    return;
                }
                wait = spawn_wait(&registry, &listen_id, &store);
            }
            _ = tokio::time::sleep_until(next_mark) => {
                if mark == mark_total {
                    wait.abort();
                    state.cancellation.cancel();
                    let reason = match state.mode {
                        ThreadListenMode::Once { .. } => ThreadListenEndReason::Timeout,
                        ThreadListenMode::Repeating { .. } => ThreadListenEndReason::Lifetime,
                    };
                    finish_session_delivery(&registry, &listen_id, &state, &sink, reason, Value::Null).await;
                    return;
                }
                if !delivered_since_mark {
                    let snapshot = state.snapshot(listen_id.clone());
                    let heartbeat_sequence = snapshot
                        .last_sequence
                        .or(Some(state.context.armed_after_sequence));
                    let heartbeat = ThreadListenHeartbeat {
                        kind: ThreadListenHeartbeatKind::ListenHeartbeat,
                        listen_id: listen_id.clone(),
                        last_sequence: heartbeat_sequence,
                        mark,
                        text: format!("nothing new since sequence {}, still listening; no action", heartbeat_sequence.map(ActivitySequence::get).unwrap_or(0)),
                    };
                    match sink.deliver(ListenDeliveryRecord::Heartbeat(heartbeat)).await {
                        Ok(()) => consecutive_rejections = 0,
                        Err(BatchSinkFailure::Rejected { evidence }) => {
                            let terminal = record_rejection(&mut consecutive_rejections, &mut last_rejection, evidence);
                            publish_rejection_state(&state, consecutive_rejections, &last_rejection);
                            if terminal {
                                finish_session_delivery(&registry, &listen_id, &state, &sink, ThreadListenEndReason::Error, last_rejection).await;
                                return;
                            }
                        }
                        Err(BatchSinkFailure::Unavailable) => {
                            finish_session_delivery(&registry, &listen_id, &state, &sink, ThreadListenEndReason::Error, json!({"kind":"unavailable"})).await;
                            return;
                        }
                    }
                }
                delivered_since_mark = false;
                mark += 1;
                next_mark += THREAD_LISTEN_MARK;
            }
        }
    }
}

fn spawn_wait(
    registry: &ThreadListenRegistry,
    listen_id: &ListenId,
    store: &Arc<Mutex<BoardStore>>,
) -> tokio::task::JoinHandle<Result<ThreadWaitResult, BoardError>> {
    let registry = registry.clone();
    let listen_id = listen_id.clone();
    let store = Arc::clone(store);
    tokio::spawn(async move {
        registry
            .wait(
                &listen_id,
                &store,
                collaboration_protocol::MAX_CONTROL_FRAME_BYTES,
            )
            .await
    })
}

async fn finish_session_delivery(
    registry: &ThreadListenRegistry,
    listen_id: &ListenId,
    state: &Arc<ThreadListenState>,
    sink: &Arc<dyn BatchSink>,
    reason: ThreadListenEndReason,
    rejection: serde_json::Value,
) {
    let snapshot = state.snapshot(listen_id.clone());
    let finalization = ThreadListenFinalization {
        kind: ThreadListenFinalizationKind::ListenEnd,
        listen_id: listen_id.clone(),
        reason,
        batches_delivered: snapshot.batches_delivered,
        first_sequence: snapshot.first_sequence,
        last_sequence: snapshot.last_sequence,
        catch_up: snapshot.catch_up,
        acknowledged: snapshot.acknowledged,
        last_rejection: (!rejection.is_null())
            .then_some(rejection)
            .or(snapshot.last_rejection),
    };
    let _ = sink
        .deliver(ListenDeliveryRecord::Finalization(finalization))
        .await;
    registry.remove(listen_id).await;
}

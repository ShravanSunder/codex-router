//! Long-poll Control fallback for process-owned Thread Listens.
use crate::ServiceIdentity;
use message_board::*;
use serde_json::{Value, json};
use std::sync::Arc;

pub(crate) async fn dispatch(
    id: Value,
    method: &str,
    params: Value,
    identity: &ServiceIdentity,
) -> Value {
    let Some(store) = identity.board.as_ref() else {
        return crate::board_request_dispatch::unavailable(id);
    };
    let result = match method {
        "board/threadListen" => {
            let request = match serde_json::from_value::<ThreadListenRequest>(params) {
                Ok(request) => request,
                Err(_) => {
                    return crate::board_request_dispatch::failure(
                        id,
                        BoardError::invalid_field(
                            "request",
                            "must contain Reader, Thread selection, Listen mode, optional fromActivitySequence, and acknowledge",
                        ),
                    );
                }
            };
            register_thread_listen(&request, identity)
                .await
                .map(|listen| serde_json::to_value(ThreadListenResult { listen }))
        }
        "board/threadWait" => {
            let request = match serde_json::from_value::<ThreadWaitRequest>(params) {
                Ok(request) => request,
                Err(_) => {
                    return crate::board_request_dispatch::failure(
                        id,
                        BoardError::invalid_field("listenId", "must identify an active Listen"),
                    );
                }
            };
            let maximum_batch_set_bytes = match maximum_batch_set_bytes(&id, &request.listen_id) {
                Ok(maximum) => maximum,
                Err(error) => return crate::board_request_dispatch::failure(id, error),
            };
            identity
                .thread_listens
                .wait(&request.listen_id, store, maximum_batch_set_bytes)
                .await
                .map(serde_json::to_value)
        }
        "board/threadListenShow" => {
            let request = match serde_json::from_value::<ThreadListenShowRequest>(params) {
                Ok(request) => request,
                Err(_) => {
                    return crate::board_request_dispatch::failure(
                        id,
                        BoardError::invalid_field("listenId", "must identify an active Listen"),
                    );
                }
            };
            identity
                .thread_listens
                .show(&request.listen_id)
                .await
                .map(|listen| serde_json::to_value(ThreadListenShowResult(listen)))
        }
        "board/threadListenCancel" => {
            let request = match serde_json::from_value::<ThreadListenCancelRequest>(params) {
                Ok(request) => request,
                Err(_) => {
                    return crate::board_request_dispatch::failure(
                        id,
                        BoardError::invalid_field("listenId", "must identify an active Listen"),
                    );
                }
            };
            identity
                .thread_listens
                .cancel(&request.listen_id)
                .await
                .map(|listen| serde_json::to_value(ThreadListenCancelResult(listen)))
        }
        _ => {
            return json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Unknown Thread Listen operation"}});
        }
    };
    match result {
        Ok(Ok(result)) => json!({"jsonrpc":"2.0","id":id,"result":result}),
        Ok(Err(_)) => crate::board_request_dispatch::failure(id, BoardError::board_unavailable()),
        Err(error) => crate::board_request_dispatch::failure(id, error),
    }
}

async fn register_thread_listen(
    request: &ThreadListenRequest,
    identity: &ServiceIdentity,
) -> Result<ThreadListenSnapshot, BoardError> {
    // The Participant joined gate belongs here, before any Watch or Listen state changes.
    let store = identity
        .board
        .as_ref()
        .ok_or_else(BoardError::board_unavailable)?;
    if request.delivery == ThreadListenDelivery::Session {
        let valid_lifetime = match request.mode {
            ThreadListenMode::Once { max_wait_seconds } => {
                max_wait_seconds == ThreadListenLifetime::Short.seconds()
            }
            ThreadListenMode::Repeating { lifetime_seconds } => matches!(
                lifetime_seconds,
                value if value == ThreadListenLifetime::Short.seconds()
                    || value == ThreadListenLifetime::Long.seconds()
            ),
        };
        if !valid_lifetime {
            return Err(BoardError::invalid_field(
                "mode",
                "session delivery requires the fixed short or long lifetime",
            ));
        }
    }
    let session_sink = if request.delivery == ThreadListenDelivery::Session {
        let Identity::Session { session } = &request.reader else {
            return Err(BoardError::invalid_field(
                "reader",
                "session delivery requires a codex-local session identity",
            ));
        };
        if session.endpoint.endpoint_id.as_str() != "codex-local"
            || session.endpoint.service_id.as_str() != String::from(identity.service_id.clone())
        {
            return Err(BoardError::invalid_field(
                "reader",
                "session delivery requires the calling codex-local session identity",
            ));
        }
        let target = serde_json::from_value(
            serde_json::to_value(session).map_err(|_| BoardError::board_unavailable())?,
        )
        .map_err(|_| BoardError::invalid_field("reader", "must be a valid SessionRef"))?;
        Some(crate::session_delivery_sink::SessionDeliverySink {
            service_id: identity.service_id.clone(),
            endpoints: identity.directory.clone(),
            backend: identity.native_backend.clone(),
            target,
        })
    } else {
        None
    };
    let context = store.lock().await.prepare_thread_listen(request).await?;
    let listen = identity.thread_listens.register(request, context).await?;
    if let Some(sink) = session_sink {
        identity.thread_listens.spawn_session_delivery(
            listen.listen_id.clone(),
            Arc::clone(store),
            Arc::new(sink),
        );
    }
    Ok(listen)
}

fn maximum_batch_set_bytes(id: &Value, listen_id: &ListenId) -> Result<usize, BoardError> {
    let empty_batch_set = ThreadListenBatchSet {
        kind: ThreadListenOutputKind::BatchSet,
        listen_id: listen_id.clone(),
        batches: Vec::new(),
        catch_up: false,
    };
    let encoded_batch_set_bytes = serde_json::to_vec(&empty_batch_set)
        .map_err(|_| BoardError::board_unavailable())?
        .len();
    [
        None,
        Some(ThreadListenEnd {
            listen_id: listen_id.clone(),
            reason: ThreadListenEndReason::Emitted,
        }),
    ]
    .into_iter()
    .map(|end| {
        let response = json!({
            "jsonrpc":"2.0",
            "id":id,
            "result":ThreadWaitResult {
                batch_set: Some(empty_batch_set.clone()),
                end,
            },
        });
        let envelope_bytes = serde_json::to_vec(&response)
            .map_err(|_| BoardError::board_unavailable())?
            .len()
            .checked_sub(encoded_batch_set_bytes)
            .ok_or_else(BoardError::board_unavailable)?;
        collaboration_protocol::MAX_CONTROL_FRAME_BYTES
            .checked_sub(envelope_bytes)
            .ok_or_else(BoardError::board_unavailable)
    })
    .collect::<Result<Vec<_>, _>>()?
    .into_iter()
    .min()
    .ok_or_else(BoardError::board_unavailable)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_set_budget_keeps_every_thread_wait_response_inside_the_control_frame() {
        let request_id = json!("thread-wait-request");
        let listen_id = ListenId::generate();
        let maximum = maximum_batch_set_bytes(&request_id, &listen_id).unwrap();
        let batch_set = ThreadListenBatchSet {
            kind: ThreadListenOutputKind::BatchSet,
            listen_id: listen_id.clone(),
            batches: Vec::new(),
            catch_up: false,
        };
        let encoded_batch_set = serde_json::to_vec(&batch_set).unwrap().len();
        assert!(encoded_batch_set <= maximum);

        for end in [
            None,
            Some(ThreadListenEnd {
                listen_id,
                reason: ThreadListenEndReason::Emitted,
            }),
        ] {
            let response = json!({
                "jsonrpc":"2.0",
                "id":request_id,
                "result":ThreadWaitResult {
                    batch_set: Some(batch_set.clone()),
                    end,
                },
            });
            let response_bytes = serde_json::to_vec(&response).unwrap().len();
            assert!(response_bytes <= collaboration_protocol::MAX_CONTROL_FRAME_BYTES);
            assert!(
                response_bytes - encoded_batch_set + maximum
                    <= collaboration_protocol::MAX_CONTROL_FRAME_BYTES
            );
        }
    }
}

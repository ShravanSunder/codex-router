//! Wake pagination binds its opaque cursor to the selected service and collection.
use crate::wakeup_dispatch::{FailureContext, WakeRequest, failure};
use automation_storage::WakeListPosition;
use communication_protocol::{
    AutomationPage, AutomationPageRequest, LocalMutationState, SavedMessage, WakeFailureReason,
};
use serde_json::{Value, json};
pub(crate) async fn dispatch(request: WakeRequest<'_>) -> Value {
    let context = FailureContext::from_params(&request.params);
    let Some(store) = request.store else {
        return failure(
            request.id,
            context,
            WakeFailureReason::AutomationUnavailable,
            LocalMutationState::None,
        );
    };
    let params = match serde_json::from_value::<AutomationPageRequest>(request.params) {
        Ok(params) => params,
        Err(_) => {
            return failure(
                request.id,
                context,
                invalid(
                    "request",
                    "Provide nullable cursor and a limit between1 and100.",
                ),
                LocalMutationState::None,
            );
        }
    };
    let digest = match crate::automation_collection_cursor::filter_digest(&json!({})) {
        Ok(digest) => digest,
        Err(()) => {
            return failure(
                request.id,
                context,
                WakeFailureReason::InvalidRecord,
                LocalMutationState::None,
            );
        }
    };
    let after = match params.cursor {
        None => None,
        Some(cursor) => {
            let decoded = crate::automation_collection_cursor::decode(
                &cursor,
                request.service_id,
                "wake/list",
                &digest,
            );
            match decoded {
                Ok(decoded) => {
                    let (Ok(upper), Ok(last)) = (
                        decoded.upper_key.1.try_into(),
                        decoded.last_key.1.try_into(),
                    ) else {
                        return failure(
                            request.id,
                            context,
                            invalid("cursor", "Cursor wake identities must be UUIDv7."),
                            LocalMutationState::None,
                        );
                    };
                    Some(WakeListPosition {
                        upper_created_at_ms: decoded.upper_key.0,
                        upper_wakeup_id: upper,
                        created_at_ms: decoded.last_key.0,
                        wakeup_id: last,
                    })
                }
                _ => {
                    return failure(
                        request.id,
                        context,
                        invalid(
                            "cursor",
                            "Cursor belongs to another service/collection or is malformed; start a fresh listing.",
                        ),
                        LocalMutationState::None,
                    );
                }
            }
        }
    };
    let page = store
        .lock()
        .await
        .list_wakeups::<SavedMessage>(after, u32::from(params.limit))
        .await;
    match page {
        Ok(page) => {
            let now = chrono::Utc::now().timestamp_millis();
            let projected = (|| {
                let positions: Vec<_> = page
                    .records
                    .iter()
                    .map(|record| {
                        (
                            record.definition.created_at_ms,
                            record.definition.wakeup_id.clone(),
                        )
                    })
                    .collect();
                let upper = page
                    .next
                    .as_ref()
                    .map(|position| {
                        (
                            position.upper_created_at_ms,
                            position.upper_wakeup_id.clone(),
                        )
                    })
                    .or_else(|| positions.last().cloned());
                let mut records = page
                    .records
                    .into_iter()
                    .map(|record| {
                        crate::wakeup_projection::snapshot(record, request.service_id, now)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let mut next = page.next;
                loop {
                    let next_cursor = next
                        .as_ref()
                        .map(|position| {
                            crate::automation_collection_cursor::encode(
                                &crate::automation_collection_cursor::CollectionCursor {
                                    version: 1,
                                    service_id: request.service_id.clone(),
                                    collection: "wake/list".into(),
                                    upper_key: (
                                        position.upper_created_at_ms,
                                        position.upper_wakeup_id.as_str().into(),
                                    ),
                                    last_key: (
                                        position.created_at_ms,
                                        position.wakeup_id.as_str().into(),
                                    ),
                                    filter_digest: digest.clone(),
                                },
                            )
                        })
                        .transpose()?;
                    let response = json!({"jsonrpc":"2.0","id":request.id,"result":AutomationPage {
                        records: records.clone(), next_cursor,
                    }});
                    if serde_json::to_vec(&response).map_err(|_| ())?.len()
                        <= communication_protocol::MAX_CONTROL_FRAME_BYTES
                    {
                        return Ok::<_, ()>(response);
                    }
                    records.pop();
                    let Some(index) = records.len().checked_sub(1) else {
                        return Err(());
                    };
                    let (created_at_ms, wakeup_id) = positions.get(index).ok_or(())?;
                    let (upper_created_at_ms, upper_wakeup_id) = upper.as_ref().ok_or(())?;
                    next = Some(WakeListPosition {
                        upper_created_at_ms: *upper_created_at_ms,
                        upper_wakeup_id: upper_wakeup_id.clone(),
                        created_at_ms: *created_at_ms,
                        wakeup_id: wakeup_id.clone(),
                    });
                }
            })();
            match projected {
                Ok(response) => response,
                Err(()) => failure(
                    request.id,
                    context,
                    WakeFailureReason::InvalidRecord,
                    LocalMutationState::None,
                ),
            }
        }
        Err(_) => failure(
            request.id,
            context,
            WakeFailureReason::AutomationUnavailable,
            LocalMutationState::None,
        ),
    }
}
fn invalid(field: &str, constraint: &str) -> WakeFailureReason {
    WakeFailureReason::InvalidField {
        field: field.into(),
        constraint: constraint.into(),
    }
}

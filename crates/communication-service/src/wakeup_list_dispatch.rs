//! Wake pagination binds its opaque cursor to the selected service and collection.
use crate::wakeup_dispatch::{FailureContext, WakeRequest, failure};
use automation_storage::WakeListPosition;
use communication_protocol::{
    AutomationPage, AutomationPageRequest, LocalMutationState, SavedMessage, UuidIdentity,
    WakeFailureReason,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WakeCursor {
    version: u8,
    service: UuidIdentity,
    collection: String,
    position: WakeListPosition,
}
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
    let after = match params.cursor {
        None => None,
        Some(cursor) => {
            let decoded = if cursor.len() <= 4096 {
                serde_json::from_str::<WakeCursor>(&cursor).ok()
            } else {
                None
            };
            match decoded {
                Some(decoded)
                    if decoded.version == 1
                        && decoded.service == *request.service_id
                        && decoded.collection == "wake/list" =>
                {
                    Some(decoded.position)
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
                let records = page
                    .records
                    .into_iter()
                    .map(|record| {
                        crate::wakeup_projection::snapshot(record, request.service_id, now)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let next_cursor = page
                    .next
                    .map(|position| {
                        serde_json::to_string(&WakeCursor {
                            version: 1,
                            service: request.service_id.clone(),
                            collection: "wake/list".into(),
                            position,
                        })
                    })
                    .transpose()
                    .map_err(|_| ())?;
                Ok::<_, ()>(AutomationPage {
                    records,
                    next_cursor,
                })
            })();
            match projected {
                Ok(page) => json!({"jsonrpc":"2.0","id":request.id,"result":page}),
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

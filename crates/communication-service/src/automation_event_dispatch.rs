//! Event history returns explicit retention gaps and complete typed records within the Control budget.
use crate::{automation_event_cursor as cursor, automation_inspection_failure as failure};
use automation_storage::{AutomationStore, EventHistoryQuery, EventHistoryRead, StorageError};
use communication_protocol::{
    AutomationEventsRequest, AutomationInspectionFailureKind, AutomationInspectionNextAction,
    AutomationInspectionStage, UuidIdentity,
};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::Mutex;
pub(crate) struct EventRequest<'a> {
    pub id: Value,
    pub params: Value,
    pub service_id: &'a UuidIdentity,
    pub store: Option<&'a Arc<Mutex<AutomationStore>>>,
}
pub(crate) async fn dispatch(request: EventRequest<'_>) -> Value {
    let Some(store) = request.store else {
        return failure::response(request.id, failure::unavailable());
    };
    let params: AutomationEventsRequest = match serde_json::from_value(request.params) {
        Ok(params) => params,
        Err(_) => {
            return failure::response(
                request.id,
                failure::invalid(
                    "request",
                    "Provide nullable after and a limit from 1 through 100.",
                ),
            );
        }
    };
    let after = match params
        .after
        .as_deref()
        .map(|value| cursor::decode(request.service_id, value))
        .transpose()
    {
        Ok(after) => after,
        Err(()) => {
            return failure::response(
                request.id,
                failure::invalid(
                    "after",
                    "Event cursor must belong to this service and contain a valid sequence/time pair.",
                ),
            );
        }
    };
    let mut store = store.lock().await;
    let history = store
        .read_event_history(&EventHistoryQuery {
            after,
            now_ms: chrono::Utc::now().timestamp_millis(),
            limit: params.limit.into(),
        })
        .await;
    let page = match history {
        Ok(EventHistoryRead::Page(page)) => page,
        Ok(EventHistoryRead::Expired { earliest }) => {
            let mut error = failure::invalid(
                "after",
                "Requested history is outside the retained two-calendar-month window; refresh current state or restart from the earliest retained cursor.",
            );
            error.kind = AutomationInspectionFailureKind::HistoryExpired;
            error.stage = AutomationInspectionStage::Inspection;
            error.next_action = AutomationInspectionNextAction::RefreshCurrentState;
            error.earliest_retained_cursor = cursor::encode(request.service_id, &earliest).ok();
            return failure::response(request.id, error);
        }
        Err(StorageError::InvalidEventCursor) => {
            return failure::response(
                request.id,
                failure::invalid(
                    "after",
                    "Cursor sequence/time pair is inconsistent with retained events or current observation.",
                ),
            );
        }
        Err(error) => return failure::response(request.id, failure::storage(error)),
    };
    let earliest = match cursor::encode(request.service_id, &page.earliest) {
        Ok(cursor) => cursor,
        Err(()) => {
            return failure::response(request.id, failure::storage(StorageError::InvalidRecord));
        }
    };
    let complete_cursor = match cursor::encode(request.service_id, &page.next) {
        Ok(cursor) => cursor,
        Err(()) => {
            return failure::response(request.id, failure::storage(StorageError::InvalidRecord));
        }
    };
    let total = page.records.len();
    let mut records = Vec::new();
    let mut next_cursor = complete_cursor.clone();
    for (index, event) in page.records.into_iter().enumerate() {
        let event = match crate::automation_event_projection::project(
            &mut store,
            event,
            request.service_id,
        )
        .await
        {
            Ok(event) => event,
            Err(error) => return failure::response(request.id, failure::storage(error)),
        };
        let candidate_cursor = if index + 1 == total {
            complete_cursor.clone()
        } else {
            event.cursor.clone()
        };
        records.push(event);
        let response = json!({"jsonrpc":"2.0","id":request.id,"result":{"records":records,"nextCursor":candidate_cursor,"earliestRetainedCursor":earliest}});
        if !serde_json::to_vec(&response)
            .is_ok_and(|bytes| bytes.len() <= communication_protocol::MAX_CONTROL_FRAME_BYTES)
        {
            records.pop();
            if records.is_empty() {
                return failure::response(
                    request.id,
                    failure::invalid(
                        "limit",
                        "A single event exceeds the Control frame budget; its content was not silently truncated.",
                    ),
                );
            }
            break;
        }
        next_cursor = candidate_cursor;
    }
    json!({"jsonrpc":"2.0","id":request.id,"result":{"records":records,"nextCursor":next_cursor,"earliestRetainedCursor":earliest}})
}

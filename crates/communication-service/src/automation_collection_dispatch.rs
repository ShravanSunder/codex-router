//! Collection pages share cursor and byte-budget mechanics, retaining each domain's typed projection.
use crate::{automation_collection_cursor as cursor, automation_inspection_failure as failure};
use automation_storage::{
    AutomationCollection, AutomationListKey, AutomationListPosition, AutomationStore,
};
use communication_protocol::{
    AutomationPageRequest, DeliveryListRequest, RevisionListRequest, RunListRequest, UuidIdentity,
};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::Mutex;

pub(crate) struct CollectionRequest<'a> {
    pub id: Value,
    pub method: &'a str,
    pub params: Value,
    pub service_id: &'a UuidIdentity,
    pub store: Option<&'a Arc<Mutex<AutomationStore>>>,
}
struct Selection {
    collection: AutomationCollection,
    cursor: Option<String>,
    limit: u32,
    filters: Value,
}
pub(crate) async fn dispatch(request: CollectionRequest<'_>) -> Value {
    let Some(store) = request.store else {
        return failure::response(request.id, failure::unavailable());
    };
    let selection = match parse(request.method, request.params) {
        Ok(selection) => selection,
        Err(()) => {
            return failure::response(
                request.id,
                failure::invalid(
                    "request",
                    "Use the published closed fields, nullable cursor and limit 1..100.",
                ),
            );
        }
    };
    let digest = match cursor::filter_digest(&selection.filters) {
        Ok(digest) => digest,
        Err(()) => {
            return failure::response(
                request.id,
                failure::invalid("filters", "Cannot encode collection filters."),
            );
        }
    };
    let position = match &selection.cursor {
        None => None,
        Some(value) => match cursor::decode(value, request.service_id, request.method, &digest) {
            Ok(cursor)
                if valid_id(&selection.collection, &cursor.upper_key.1)
                    && valid_id(&selection.collection, &cursor.last_key.1) =>
            {
                Some(AutomationListPosition {
                    upper: AutomationListKey {
                        created_at_ms: cursor.upper_key.0,
                        resource_id: cursor.upper_key.1,
                    },
                    last: AutomationListKey {
                        created_at_ms: cursor.last_key.0,
                        resource_id: cursor.last_key.1,
                    },
                })
            }
            _ => {
                return failure::response(
                    request.id,
                    failure::invalid(
                        "cursor",
                        "Cursor must match this service, collection, filter and stable UUIDv7 key range; start a fresh listing.",
                    ),
                );
            }
        },
    };
    if position
        .as_ref()
        .is_some_and(|position| position.validate_for(&selection.collection).is_err())
    {
        return failure::response(
            request.id,
            failure::invalid(
                "cursor",
                "Cursor creation times do not match its ordered resource identities.",
            ),
        );
    }
    let mut store = store.lock().await;
    let page = match store
        .list_collection_keys(&selection.collection, position, selection.limit)
        .await
    {
        Ok(page) => page,
        Err(error) => return failure::response(request.id, failure::storage(error)),
    };
    let mut records = Vec::new();
    let mut next_cursor = None;
    for (index, key) in page.records.iter().enumerate() {
        let record = match crate::automation_collection_projection::project_record(
            &mut store,
            &selection.collection,
            &key.resource_id,
        )
        .await
        {
            Ok(record) => record,
            Err(error) => return failure::response(request.id, failure::storage(error)),
        };
        records.push(record);
        let has_more = page.has_more || index + 1 < page.records.len();
        let candidate_cursor = if has_more {
            match next_token(
                request.service_id,
                request.method,
                &digest,
                page.upper.as_ref(),
                key,
            ) {
                Ok(cursor) => Some(cursor),
                Err(()) => {
                    return failure::response(
                        request.id,
                        failure::invalid("cursor", "Cannot encode the next page boundary."),
                    );
                }
            }
        } else {
            None
        };
        let response = json!({"jsonrpc":"2.0","id":request.id,"result":{"records":records,"nextCursor":candidate_cursor}});
        if !serde_json::to_vec(&response)
            .is_ok_and(|bytes| bytes.len() <= communication_protocol::MAX_CONTROL_FRAME_BYTES)
        {
            records.pop();
            if records.is_empty() {
                let mut error = failure::invalid(
                    "limit",
                    "One record exceeds the 1048576-byte Control response limit; no record was truncated.",
                );
                error.resource_id = Some(key.resource_id.clone());
                return failure::response(request.id, error);
            }
            // The preceding candidate already retained a cursor because this row was still pending.
            break;
        }
        next_cursor = candidate_cursor;
    }
    json!({"jsonrpc":"2.0","id":request.id,"result":{"records":records,"nextCursor":next_cursor}})
}
fn next_token(
    service_id: &UuidIdentity,
    method: &str,
    digest: &str,
    upper: Option<&AutomationListKey>,
    last: &AutomationListKey,
) -> Result<String, ()> {
    let upper = upper.ok_or(())?;
    cursor::encode(&cursor::CollectionCursor {
        version: 1,
        service_id: service_id.clone(),
        collection: method.into(),
        upper_key: (upper.created_at_ms, upper.resource_id.clone()),
        last_key: (last.created_at_ms, last.resource_id.clone()),
        filter_digest: digest.into(),
    })
}
fn parse(method: &str, params: Value) -> Result<Selection, ()> {
    match method {
        "instruction/list" | "schedule/list" => {
            let params: AutomationPageRequest = serde_json::from_value(params).map_err(|_| ())?;
            Ok(Selection {
                collection: if method == "instruction/list" {
                    AutomationCollection::Instructions
                } else {
                    AutomationCollection::Schedules
                },
                cursor: params.cursor,
                limit: params.limit.into(),
                filters: json!({}),
            })
        }
        "run/list" => {
            let params: RunListRequest = serde_json::from_value(params).map_err(|_| ())?;
            Ok(Selection {
                filters: json!({"scheduleId":params.schedule_id}),
                collection: AutomationCollection::Runs(params.schedule_id),
                cursor: params.cursor,
                limit: params.limit.into(),
            })
        }
        "revision/list" => {
            let params: RevisionListRequest = serde_json::from_value(params).map_err(|_| ())?;
            Ok(Selection {
                filters: json!({"instructionId":params.instruction_id}),
                collection: AutomationCollection::Revisions(params.instruction_id),
                cursor: params.cursor,
                limit: params.limit.into(),
            })
        }
        "delivery/list" => {
            let params: DeliveryListRequest = serde_json::from_value(params).map_err(|_| ())?;
            Ok(Selection {
                filters: json!({"wakeupId":params.wakeup_id}),
                collection: AutomationCollection::Deliveries(params.wakeup_id),
                cursor: params.cursor,
                limit: params.limit.into(),
            })
        }
        _ => Err(()),
    }
}
fn valid_id(collection: &AutomationCollection, value: &str) -> bool {
    match collection {
        AutomationCollection::Instructions => {
            agent_automation::InstructionId::try_from(value.to_owned()).is_ok()
        }
        AutomationCollection::Schedules => {
            agent_automation::ScheduleId::try_from(value.to_owned()).is_ok()
        }
        AutomationCollection::Runs(_) => {
            agent_automation::RunId::try_from(value.to_owned()).is_ok()
        }
        AutomationCollection::Revisions(_) => {
            agent_automation::RevisionId::try_from(value.to_owned()).is_ok()
        }
        AutomationCollection::Deliveries(_) => {
            agent_automation::DeliveryId::try_from(value.to_owned()).is_ok()
        }
    }
}

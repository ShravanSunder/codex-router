//! Attempt pages pin observation time and latest identity while disclosing expired prior evidence.
use crate::{automation_collection_cursor as cursor, automation_inspection_failure as failure};
use automation_storage::{
    AttemptCollection, AttemptHistoryPosition, AttemptHistoryQuery, AttemptHistoryRead,
    AutomationStore, StorageError,
};
use communication_protocol::{
    AttemptHistoryCoverage, DeliveryAttemptsRequest, EarlierAttempts, RunSummariesRequest,
    UuidIdentity,
};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::Mutex;
pub(crate) struct AttemptRequest<'a> {
    pub id: Value,
    pub method: &'a str,
    pub params: Value,
    pub service_id: &'a UuidIdentity,
    pub store: Option<&'a Arc<Mutex<AutomationStore>>>,
}
struct Selection {
    collection: AttemptCollection,
    cursor: Option<String>,
    limit: u32,
    filters: Value,
}
pub(crate) async fn dispatch(request: AttemptRequest<'_>) -> Value {
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
                    "Provide the exact owning resource, nullable cursor and limit 1..100.",
                ),
            );
        }
    };
    let digest = match cursor::filter_digest(&selection.filters) {
        Ok(digest) => digest,
        Err(()) => {
            return failure::response(
                request.id,
                failure::invalid("request", "Cannot encode owner filters."),
            );
        }
    };
    let position = match selection
        .cursor
        .as_deref()
        .map(|value| decode_position(value, request.service_id, request.method, &digest))
        .transpose()
    {
        Ok(position) => position,
        Err(()) => {
            return failure::response(
                request.id,
                failure::invalid(
                    "cursor",
                    "Cursor must match this service, owner and pinned attempt page.",
                ),
            );
        }
    };
    let mut store = store.lock().await;
    let history = store
        .read_attempt_history(&AttemptHistoryQuery {
            collection: selection.collection,
            position,
            now_ms: chrono::Utc::now().timestamp_millis(),
            limit: selection.limit,
        })
        .await;
    let page = match history {
        Ok(AttemptHistoryRead::Page(page)) => page,
        Ok(AttemptHistoryRead::Expired) => {
            let mut error = failure::invalid(
                "cursor",
                "Pinned attempt history is no longer retained; start a new page without a cursor to inspect durable latest evidence.",
            );
            error.kind = communication_protocol::AutomationInspectionFailureKind::HistoryExpired;
            error.stage = communication_protocol::AutomationInspectionStage::Inspection;
            error.next_action =
                communication_protocol::AutomationInspectionNextAction::RefreshCurrentState;
            return failure::response(request.id, error);
        }
        Err(StorageError::InvalidAttemptCursor) => {
            return failure::response(
                request.id,
                failure::invalid(
                    "cursor",
                    "Attempt cursor time or identity is inconsistent with retained evidence.",
                ),
            );
        }
        Err(error) => return failure::response(request.id, failure::storage(error)),
    };
    let mut coverage = match (
        crate::wakeup_projection::timestamp(page.history_from_ms),
        crate::wakeup_projection::timestamp(page.as_of_ms),
    ) {
        (Ok(history_from), Ok(as_of)) => AttemptHistoryCoverage {
            history_from,
            as_of,
            earlier_attempts: EarlierAttempts::MayBeUnavailable,
            latest_attempt_included: false,
        },
        _ => return failure::response(request.id, failure::storage(StorageError::InvalidRecord)),
    };
    let mut records = Vec::new();
    let mut next_cursor = None;
    let total = page.records.len();
    for (index, record) in page.records.into_iter().enumerate() {
        let projected = if request.method == "delivery/attempts" {
            let parsed = (|| {
                let id = selection
                    .filters
                    .get("deliveryId")
                    .cloned()
                    .ok_or(StorageError::InvalidRecord)?;
                let id = serde_json::from_value(id).map_err(|_| StorageError::InvalidRecord)?;
                let attempt =
                    serde_json::from_value(record.body).map_err(|_| StorageError::InvalidRecord)?;
                Ok::<_, StorageError>((id, attempt))
            })();
            match parsed {
                Ok((id, attempt)) => {
                    crate::attempt_history_projection::delivery(&mut store, &id, attempt)
                        .await
                        .and_then(|record| {
                            serde_json::to_value(record).map_err(|_| StorageError::InvalidRecord)
                        })
                }
                Err(error) => Err(error),
            }
        } else {
            (|| {
                let id = serde_json::from_value(
                    selection
                        .filters
                        .get("runId")
                        .cloned()
                        .ok_or(StorageError::InvalidRecord)?,
                )
                .map_err(|_| StorageError::InvalidRecord)?;
                let attempt =
                    serde_json::from_value(record.body).map_err(|_| StorageError::InvalidRecord)?;
                let retry = page.current_attempt_id.as_ref() == Some(&record.attempt_id)
                    && matches!(
                        page.owner_phase,
                        Some(
                            agent_automation::RunPhase::SummaryRequired
                                | agent_automation::RunPhase::SummaryBlocked
                        )
                    );
                serde_json::to_value(crate::attempt_history_projection::summary(
                    &id, attempt, retry,
                )?)
                .map_err(|_| StorageError::InvalidRecord)
            })()
        };
        let projected = match projected {
            Ok(projected) => projected,
            Err(error) => return failure::response(request.id, failure::storage(error)),
        };
        records.push(projected);
        let previous_included = coverage.latest_attempt_included;
        coverage.latest_attempt_included |=
            page.latest_attempt_id.as_ref() == Some(&record.attempt_id);
        let candidate_cursor = if page.has_more || index + 1 < total {
            let Some(latest) = &page.latest_attempt_id else {
                return failure::response(
                    request.id,
                    failure::storage(StorageError::InvalidRecord),
                );
            };
            match cursor::encode(&cursor::CollectionCursor {
                version: 1,
                service_id: request.service_id.clone(),
                collection: request.method.into(),
                upper_key: (page.as_of_ms, latest.as_str().into()),
                last_key: (record.started_at_ms, record.attempt_id.as_str().into()),
                filter_digest: digest.clone(),
            }) {
                Ok(cursor) => Some(cursor),
                Err(()) => {
                    return failure::response(
                        request.id,
                        failure::storage(StorageError::InvalidRecord),
                    );
                }
            }
        } else {
            None
        };
        let response = json!({"jsonrpc":"2.0","id":request.id,"result":{"records":records,"nextCursor":candidate_cursor,"coverage":coverage}});
        if !serde_json::to_vec(&response)
            .is_ok_and(|bytes| bytes.len() <= communication_protocol::MAX_CONTROL_FRAME_BYTES)
        {
            records.pop();
            coverage.latest_attempt_included = previous_included;
            if records.is_empty() {
                return failure::response(
                    request.id,
                    failure::invalid(
                        "limit",
                        "One complete attempt record exceeds the Control frame limit; it was not silently truncated.",
                    ),
                );
            }
            break;
        }
        next_cursor = candidate_cursor;
    }
    json!({"jsonrpc":"2.0","id":request.id,"result":{"records":records,"nextCursor":next_cursor,"coverage":coverage}})
}
fn decode_position(
    value: &str,
    service: &UuidIdentity,
    method: &str,
    digest: &str,
) -> Result<AttemptHistoryPosition, ()> {
    let decoded = cursor::decode(value, service, method, digest)?;
    Ok(AttemptHistoryPosition {
        as_of_ms: decoded.upper_key.0,
        latest_attempt_id: decoded.upper_key.1.try_into().map_err(|_| ())?,
        last_started_at_ms: decoded.last_key.0,
        last_attempt_id: decoded.last_key.1.try_into().map_err(|_| ())?,
    })
}
fn parse(method: &str, params: Value) -> Result<Selection, ()> {
    match method {
        "delivery/attempts" => {
            let params: DeliveryAttemptsRequest = serde_json::from_value(params).map_err(|_| ())?;
            Ok(Selection {
                filters: json!({"deliveryId":params.delivery_id}),
                collection: AttemptCollection::Delivery(params.delivery_id),
                cursor: params.cursor,
                limit: params.limit.into(),
            })
        }
        "run/summaries" => {
            let params: RunSummariesRequest = serde_json::from_value(params).map_err(|_| ())?;
            Ok(Selection {
                filters: json!({"runId":params.run_id}),
                collection: AttemptCollection::Summary(params.run_id),
                cursor: params.cursor,
                limit: params.limit.into(),
            })
        }
        _ => Err(()),
    }
}

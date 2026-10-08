//! Automation history: pinned attempt pages and the retained automation event stream, each
//! disclosing what retention no longer holds.
use super::automation_inspection_operations::{InspectionResult, typed_records};
use super::{AutomationOperations, ResultByteBudget};
use crate::{automation_collection_cursor as cursor, automation_inspection_failure as failure};
use automation_storage::{
    AttemptCollection, AttemptHistoryPosition, AttemptHistoryQuery, AttemptHistoryRead,
    AutomationStore, EventHistoryQuery, EventHistoryRead, StorageError,
};
use collaboration_protocol::{
    AttemptHistoryCoverage, AttemptHistoryPage, AttemptInspection, AutomationEventsPage,
    AutomationEventsRequest, AutomationInspectionFailureKind, AutomationInspectionNextAction,
    AutomationInspectionStage, DeliveryAttemptsRequest, EarlierAttempts, RunSummariesRequest,
    SummaryInspection, UuidIdentity,
};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

impl<'service> AutomationOperations<'service> {
    /// Pages one delivery's attempts as of a pinned observation time.
    pub async fn delivery_attempts(
        &self,
        request: DeliveryAttemptsRequest,
        budget: ResultByteBudget,
    ) -> InspectionResult<AttemptHistoryPage<AttemptInspection>> {
        self.attempt_history(
            AttemptSelection {
                scope: "delivery/attempts",
                filters: json!({"deliveryId":request.delivery_id}),
                collection: AttemptCollection::Delivery(request.delivery_id),
                cursor: request.cursor,
                limit: request.limit.into(),
            },
            budget,
        )
        .await
    }

    /// Pages one Run's summary attempts as of a pinned observation time.
    pub async fn run_summaries(
        &self,
        request: RunSummariesRequest,
        budget: ResultByteBudget,
    ) -> InspectionResult<AttemptHistoryPage<SummaryInspection>> {
        self.attempt_history(
            AttemptSelection {
                scope: "run/summaries",
                filters: json!({"runId":request.run_id}),
                collection: AttemptCollection::Summary(request.run_id),
                cursor: request.cursor,
                limit: request.limit.into(),
            },
            budget,
        )
        .await
    }

    async fn attempt_history<TRecord: DeserializeOwned>(
        &self,
        selection: AttemptSelection,
        budget: ResultByteBudget,
    ) -> InspectionResult<AttemptHistoryPage<TRecord>> {
        let store = self.inspection_store()?;
        let service_id = &self.identity.service_id;
        let digest = cursor::filter_digest(&selection.filters)
            .map_err(|()| failure::invalid("request", "Cannot encode owner filters."))?;
        let position = selection
            .cursor
            .as_deref()
            .map(|value| decode_attempt_position(value, service_id, selection.scope, &digest))
            .transpose()
            .map_err(|()| {
                failure::invalid(
                    "cursor",
                    "Cursor must match this service, owner and pinned attempt page.",
                )
            })?;
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
                error.kind = AutomationInspectionFailureKind::HistoryExpired;
                error.stage = AutomationInspectionStage::Inspection;
                error.next_action = AutomationInspectionNextAction::RefreshCurrentState;
                return Err(error);
            }
            Err(StorageError::InvalidAttemptCursor) => {
                return Err(failure::invalid(
                    "cursor",
                    "Attempt cursor time or identity is inconsistent with retained evidence.",
                ));
            }
            Err(error) => return Err(failure::storage(error)),
        };
        let invalid_record = || failure::storage(StorageError::InvalidRecord);
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
            _ => return Err(invalid_record()),
        };
        let mut records = Vec::new();
        let mut next_cursor = None;
        let total = page.records.len();
        for (index, record) in page.records.into_iter().enumerate() {
            let projected = if selection.scope == "delivery/attempts" {
                project_delivery_attempt(
                    &mut store,
                    &selection.filters,
                    record.body,
                    record.receipt,
                )
                .await
            } else {
                project_summary_attempt(
                    &selection.filters,
                    record.body,
                    page.current_attempt_id.as_ref() == Some(&record.attempt_id)
                        && matches!(
                            page.owner_phase,
                            Some(
                                agent_automation::RunPhase::SummaryRequired
                                    | agent_automation::RunPhase::SummaryBlocked
                            )
                        ),
                )
            }
            .map_err(failure::storage)?;
            records.push(projected);
            let previous_included = coverage.latest_attempt_included;
            coverage.latest_attempt_included |=
                page.latest_attempt_id.as_ref() == Some(&record.attempt_id);
            let candidate_cursor = if page.has_more || index + 1 < total {
                let latest = page.latest_attempt_id.as_ref().ok_or_else(invalid_record)?;
                Some(
                    cursor::encode(&cursor::CollectionCursor {
                        version: 1,
                        service_id: service_id.clone(),
                        collection: selection.scope.into(),
                        upper_key: (page.as_of_ms, latest.as_str().into()),
                        last_key: (record.started_at_ms, record.attempt_id.as_str().into()),
                        filter_digest: digest.clone(),
                    })
                    .map_err(|()| invalid_record())?,
                )
            } else {
                None
            };
            let candidate = AttemptHistoryPage {
                records,
                next_cursor: candidate_cursor,
                coverage: coverage.clone(),
            };
            if !budget.admits(&candidate) {
                records = candidate.records;
                records.pop();
                coverage.latest_attempt_included = previous_included;
                if records.is_empty() {
                    return Err(failure::invalid(
                        "limit",
                        "One complete attempt record exceeds the Control frame limit; it was not silently truncated.",
                    ));
                }
                break;
            }
            records = candidate.records;
            next_cursor = candidate.next_cursor;
        }
        Ok(AttemptHistoryPage {
            records: typed_records(records)?,
            next_cursor,
            coverage,
        })
    }

    /// Reads retained automation events after `after`, disclosing any retention gap.
    pub async fn automation_events(
        &self,
        request: AutomationEventsRequest,
        budget: ResultByteBudget,
    ) -> InspectionResult<AutomationEventsPage> {
        let store = self.inspection_store()?;
        let service_id = &self.identity.service_id;
        let after = request
            .after
            .as_deref()
            .map(|value| crate::automation_event_cursor::decode(service_id, value))
            .transpose()
            .map_err(|()| {
                failure::invalid(
                    "after",
                    "Event cursor must belong to this service and contain a valid sequence/time pair.",
                )
            })?;
        let mut store = store.lock().await;
        let history = store
            .read_event_history(&EventHistoryQuery {
                after,
                now_ms: chrono::Utc::now().timestamp_millis(),
                limit: request.limit.into(),
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
                error.earliest_retained_cursor =
                    crate::automation_event_cursor::encode(service_id, &earliest).ok();
                return Err(error);
            }
            Err(StorageError::InvalidEventCursor) => {
                return Err(failure::invalid(
                    "after",
                    "Cursor sequence/time pair is inconsistent with retained events or current observation.",
                ));
            }
            Err(error) => return Err(failure::storage(error)),
        };
        let invalid_record = || failure::storage(StorageError::InvalidRecord);
        let earliest = crate::automation_event_cursor::encode(service_id, &page.earliest)
            .map_err(|()| invalid_record())?;
        let complete_cursor = crate::automation_event_cursor::encode(service_id, &page.next)
            .map_err(|()| invalid_record())?;
        let total = page.records.len();
        let mut records = Vec::new();
        let mut next_cursor = complete_cursor.clone();
        for (index, event) in page.records.into_iter().enumerate() {
            let event = crate::automation_event_projection::project(&mut store, event, service_id)
                .await
                .map_err(failure::storage)?;
            let candidate_cursor = if index + 1 == total {
                complete_cursor.clone()
            } else {
                event.cursor.clone()
            };
            records.push(event);
            let candidate = AutomationEventsPage {
                records,
                next_cursor: candidate_cursor,
                earliest_retained_cursor: earliest.clone(),
            };
            if !budget.admits(&candidate) {
                records = candidate.records;
                records.pop();
                if records.is_empty() {
                    return Err(failure::invalid(
                        "limit",
                        "A single event exceeds the Control frame budget; its content was not silently truncated.",
                    ));
                }
                break;
            }
            records = candidate.records;
            next_cursor = candidate.next_cursor;
        }
        Ok(AutomationEventsPage {
            records,
            next_cursor,
            earliest_retained_cursor: earliest,
        })
    }
}

struct AttemptSelection {
    /// The history name a cursor is bound to.
    scope: &'static str,
    collection: AttemptCollection,
    cursor: Option<String>,
    limit: u32,
    filters: Value,
}

fn decode_attempt_position(
    value: &str,
    service: &UuidIdentity,
    scope: &str,
    digest: &str,
) -> Result<AttemptHistoryPosition, ()> {
    let decoded = cursor::decode(value, service, scope, digest)?;
    Ok(AttemptHistoryPosition {
        as_of_ms: decoded.upper_key.0,
        latest_attempt_id: decoded.upper_key.1.try_into().map_err(|_| ())?,
        last_started_at_ms: decoded.last_key.0,
        last_attempt_id: decoded.last_key.1.try_into().map_err(|_| ())?,
    })
}

async fn project_delivery_attempt(
    store: &mut AutomationStore,
    filters: &Value,
    body: Value,
    receipt: Option<Value>,
) -> Result<Value, StorageError> {
    let id = filters
        .get("deliveryId")
        .cloned()
        .ok_or(StorageError::InvalidRecord)?;
    let id = serde_json::from_value(id).map_err(|_| StorageError::InvalidRecord)?;
    let attempt = serde_json::from_value(body).map_err(|_| StorageError::InvalidRecord)?;
    let receipt = receipt
        .map(serde_json::from_value)
        .transpose()
        .map_err(|_| StorageError::InvalidRecord)?;
    let projected =
        crate::attempt_history_projection::delivery(store, &id, attempt, receipt).await?;
    serde_json::to_value(projected).map_err(|_| StorageError::InvalidRecord)
}

fn project_summary_attempt(
    filters: &Value,
    body: Value,
    retry_allowed: bool,
) -> Result<Value, StorageError> {
    let id = serde_json::from_value(
        filters
            .get("runId")
            .cloned()
            .ok_or(StorageError::InvalidRecord)?,
    )
    .map_err(|_| StorageError::InvalidRecord)?;
    let attempt = serde_json::from_value(body).map_err(|_| StorageError::InvalidRecord)?;
    serde_json::to_value(crate::attempt_history_projection::summary(
        &id,
        attempt,
        retry_allowed,
    )?)
    .map_err(|_| StorageError::InvalidRecord)
}

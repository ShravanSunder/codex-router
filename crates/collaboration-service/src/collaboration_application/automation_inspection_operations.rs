//! Automation inspection: collection pages, read-only reconciliation and durable operation
//! receipts. Attempt and event history live in `automation_history_operations`. Nothing here
//! repeats a native mutation.
use super::{AutomationOperations, ResultByteBudget};
use crate::{automation_collection_cursor as cursor, automation_inspection_failure as failure};
use automation_storage::{
    AutomationCollection, AutomationListKey, AutomationListPosition, AutomationStore, StorageError,
};
use collaboration_protocol::{
    AutomationInspectionFailure, AutomationPage, AutomationPageRequest, CodexGeneration,
    DeliveryInspection, DeliveryListRequest, DeliveryShowRequest, EndpointRef, InstructionSnapshot,
    OperationShowRequest, OperationSnapshot, RevisionListRequest, RevisionRecord, RunListRequest,
    RunShowRequest, RunSnapshot, ScheduleSnapshot, SessionRef, UuidIdentity,
};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::Mutex;

pub(super) type InspectionResult<TResult> = Result<TResult, AutomationInspectionFailure>;

impl<'service> AutomationOperations<'service> {
    pub(super) fn inspection_store(
        &self,
    ) -> InspectionResult<&'service Arc<Mutex<AutomationStore>>> {
        self.store().ok_or_else(failure::unavailable)
    }

    /// Pages instruction documents, newest first.
    pub async fn instruction_list(
        &self,
        request: AutomationPageRequest,
        budget: ResultByteBudget,
    ) -> InspectionResult<AutomationPage<InstructionSnapshot>> {
        self.list_collection(
            CollectionSelection {
                scope: "instruction/list",
                collection: AutomationCollection::Instructions,
                cursor: request.cursor,
                limit: request.limit.into(),
                filters: json!({}),
            },
            budget,
        )
        .await
    }

    /// Pages schedules, newest first.
    pub async fn schedule_list(
        &self,
        request: AutomationPageRequest,
        budget: ResultByteBudget,
    ) -> InspectionResult<AutomationPage<ScheduleSnapshot>> {
        self.list_collection(
            CollectionSelection {
                scope: "schedule/list",
                collection: AutomationCollection::Schedules,
                cursor: request.cursor,
                limit: request.limit.into(),
                filters: json!({}),
            },
            budget,
        )
        .await
    }

    /// Pages one schedule's Runs, newest first.
    pub async fn run_list(
        &self,
        request: RunListRequest,
        budget: ResultByteBudget,
    ) -> InspectionResult<AutomationPage<RunSnapshot>> {
        self.list_collection(
            CollectionSelection {
                scope: "run/list",
                filters: json!({"scheduleId":request.schedule_id}),
                collection: AutomationCollection::Runs(request.schedule_id),
                cursor: request.cursor,
                limit: request.limit.into(),
            },
            budget,
        )
        .await
    }

    /// Pages one instruction's revisions, newest first.
    pub async fn revision_list(
        &self,
        request: RevisionListRequest,
        budget: ResultByteBudget,
    ) -> InspectionResult<AutomationPage<RevisionRecord>> {
        self.list_collection(
            CollectionSelection {
                scope: "revision/list",
                filters: json!({"instructionId":request.instruction_id}),
                collection: AutomationCollection::Revisions(request.instruction_id),
                cursor: request.cursor,
                limit: request.limit.into(),
            },
            budget,
        )
        .await
    }

    /// Pages deliveries, optionally of one wake, newest first.
    pub async fn delivery_list(
        &self,
        request: DeliveryListRequest,
        budget: ResultByteBudget,
    ) -> InspectionResult<AutomationPage<DeliveryInspection>> {
        self.list_collection(
            CollectionSelection {
                scope: "delivery/list",
                filters: json!({"wakeupId":request.wakeup_id}),
                collection: AutomationCollection::Deliveries(request.wakeup_id),
                cursor: request.cursor,
                limit: request.limit.into(),
            },
            budget,
        )
        .await
    }

    async fn list_collection<TRecord: DeserializeOwned>(
        &self,
        selection: CollectionSelection,
        budget: ResultByteBudget,
    ) -> InspectionResult<AutomationPage<TRecord>> {
        let store = self.inspection_store()?;
        let service_id = &self.identity.service_id;
        let digest = cursor::filter_digest(&selection.filters)
            .map_err(|()| failure::invalid("filters", "Cannot encode collection filters."))?;
        let position = match &selection.cursor {
            None => None,
            Some(value) => match cursor::decode(value, service_id, selection.scope, &digest) {
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
                    return Err(failure::invalid(
                        "cursor",
                        "Cursor must match this service, collection, filter and stable UUIDv7 key range; start a fresh listing.",
                    ));
                }
            },
        };
        if position
            .as_ref()
            .is_some_and(|position| position.validate_for(&selection.collection).is_err())
        {
            return Err(failure::invalid(
                "cursor",
                "Cursor creation times do not match its ordered resource identities.",
            ));
        }
        let mut store = store.lock().await;
        let page = store
            .list_collection_keys(&selection.collection, position, selection.limit)
            .await
            .map_err(failure::storage)?;
        let mut records = Vec::new();
        let mut next_cursor = None;
        for (index, key) in page.records.iter().enumerate() {
            let record = crate::automation_collection_projection::project_record(
                &mut store,
                &selection.collection,
                &key.resource_id,
            )
            .await
            .map_err(failure::storage)?;
            records.push(record);
            let has_more = page.has_more || index + 1 < page.records.len();
            let candidate_cursor = if has_more {
                Some(
                    next_token(
                        service_id,
                        selection.scope,
                        &digest,
                        page.upper.as_ref(),
                        key,
                    )
                    .map_err(|()| {
                        failure::invalid("cursor", "Cannot encode the next page boundary.")
                    })?,
                )
            } else {
                None
            };
            let candidate = AutomationPage {
                records,
                next_cursor: candidate_cursor,
            };
            if !budget.admits(&candidate) {
                records = candidate.records;
                records.pop();
                if records.is_empty() {
                    let mut error = failure::invalid(
                        "limit",
                        "One record exceeds the 1048576-byte response limit; no record was truncated.",
                    );
                    error.resource_id = Some(key.resource_id.clone());
                    return Err(error);
                }
                // The preceding candidate already retained a cursor because this row was still pending.
                break;
            }
            records = candidate.records;
            next_cursor = candidate.next_cursor;
        }
        Ok(AutomationPage {
            records: typed_records(records)?,
            next_cursor,
        })
    }

    /// Reconciles a delivery from exact native queue evidence; input is never resent.
    pub async fn delivery_reconcile(
        &self,
        request: DeliveryShowRequest,
        budget: ResultByteBudget,
    ) -> InspectionResult<DeliveryInspection> {
        let store = self.inspection_store()?;
        let record = read_delivery(store, &request).await?;
        let session_delivery = self
            .identity
            .session_delivery
            .as_ref()
            .ok_or_else(failure::unavailable)?;
        crate::delivery_reconciliation::reconcile(store, session_delivery.as_ref(), record)
            .await
            .map_err(failure::storage)?;
        let record = read_delivery(store, &request).await?;
        let mut snapshot = crate::delivery_projection::snapshot(record)
            .map_err(|()| failure::storage(StorageError::InvalidRecord))?;
        if let collaboration_protocol::DeliveryEvidence::OutcomeUnknown { explanation, .. } =
            &mut snapshot.evidence
        {
            *explanation = "Read-only reconciliation did not establish a complete native receipt. Missing, ambiguous or unavailable queue evidence does not prove non-submission. Inspect the exact target and attempt; no input was resent.".into();
        }
        bounded_reconciliation(snapshot, budget)
    }

    /// Reconciles a Run from exact native evidence; work is never started or interrupted.
    pub async fn run_reconcile(
        &self,
        request: RunShowRequest,
        budget: ResultByteBudget,
    ) -> InspectionResult<RunSnapshot> {
        let store = self.inspection_store()?;
        let record = read_run(store, &request).await?;
        crate::run_reconciliation::reconcile(
            store,
            self.identity.scheduled_run_execution.as_ref(),
            self.identity.native_backend.as_ref(),
            record,
        )
        .await
        .map_err(failure::storage)?;
        let record = read_run(store, &request).await?;
        let snapshot = crate::run_projection::snapshot(record)
            .map_err(|()| failure::storage(StorageError::InvalidRecord))?;
        bounded_reconciliation(snapshot, budget)
    }

    /// Shows an operation's original receipt; nothing is resubmitted.
    pub async fn operation_show(
        &self,
        request: OperationShowRequest,
        budget: ResultByteBudget,
    ) -> InspectionResult<OperationSnapshot> {
        self.inspect_operation(request, false, budget).await
    }

    /// Reconciles an unsettled configuration operation through the Host, then shows its receipt.
    pub async fn operation_reconcile(
        &self,
        request: OperationShowRequest,
        budget: ResultByteBudget,
    ) -> InspectionResult<OperationSnapshot> {
        self.inspect_operation(request, true, budget).await
    }

    async fn inspect_operation(
        &self,
        request: OperationShowRequest,
        reconcile: bool,
        budget: ResultByteBudget,
    ) -> InspectionResult<OperationSnapshot> {
        let store = self.inspection_store()?;
        let mut record = match store
            .lock()
            .await
            .read_operation(&request.operation_id)
            .await
        {
            Ok(record) => record,
            Err(error) => {
                let mut error = failure::storage(error);
                error.resource_id = Some(request.operation_id.as_str().into());
                return Err(error);
            }
        };
        let mut explanation = None;
        if reconcile
            && record.method == "automation/configure"
            && !matches!(
                record.state,
                automation_storage::StoredOperationState::Succeeded { .. }
                    | automation_storage::StoredOperationState::Failed { .. }
            )
        {
            if let Some(backend) = self.identity.configuration_backend.as_ref() {
                if let Err(error) = backend.reconcile(request.operation_id.clone()).await {
                    explanation = Some(error.message);
                }
                record = store
                    .lock()
                    .await
                    .read_operation(&request.operation_id)
                    .await
                    .map_err(failure::storage)?;
            } else {
                explanation = Some("The Host configuration backend is unavailable. Reconnect to the owning Host and reconcile this operation ID; no native work was resubmitted.".into());
            }
        }
        let mut snapshot =
            crate::operation_receipt_projection::snapshot(record, &self.identity.service_id)
                .map_err(failure::storage)?;
        if let (
            Some(message),
            collaboration_protocol::OperationState::Uncertain { explanation, .. },
        ) = (explanation, &mut snapshot.state)
        {
            *explanation = message;
        }
        match budget.response_bytes(&snapshot) {
            Some(bytes) if bytes <= budget.response_limit_bytes() => Ok(snapshot),
            Some(_) => {
                let mut error = failure::invalid(
                    "operationId",
                    "Original receipt exceeds the response limit; inspect the affected resource. No request was resubmitted.",
                );
                error.resource_id = Some(snapshot.resource_id);
                Err(error)
            }
            None => Err(failure::storage(StorageError::InvalidRecord)),
        }
    }
}

struct CollectionSelection {
    /// The collection name a cursor is bound to.
    scope: &'static str,
    collection: AutomationCollection,
    cursor: Option<String>,
    limit: u32,
    filters: Value,
}

/// Records are projected as JSON so pages can be measured; the page returns them typed.
pub(super) fn typed_records<TRecord: DeserializeOwned>(
    records: Vec<Value>,
) -> InspectionResult<Vec<TRecord>> {
    records
        .into_iter()
        .map(|record| {
            serde_json::from_value(record)
                .map_err(|_| failure::storage(StorageError::InvalidRecord))
        })
        .collect()
}

fn next_token(
    service_id: &UuidIdentity,
    scope: &str,
    digest: &str,
    upper: Option<&AutomationListKey>,
    last: &AutomationListKey,
) -> Result<String, ()> {
    let upper = upper.ok_or(())?;
    cursor::encode(&cursor::CollectionCursor {
        version: 1,
        service_id: service_id.clone(),
        collection: scope.into(),
        upper_key: (upper.created_at_ms, upper.resource_id.clone()),
        last_key: (last.created_at_ms, last.resource_id.clone()),
        filter_digest: digest.into(),
    })
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

async fn read_delivery(
    store: &Arc<Mutex<AutomationStore>>,
    request: &DeliveryShowRequest,
) -> InspectionResult<
    automation_storage::DeliveryRecord<
        SessionRef,
        CodexGeneration,
        crate::stored_delivery_receipt::StoredDeliveryReceipt,
    >,
> {
    store
        .lock()
        .await
        .read_delivery::<SessionRef, CodexGeneration, crate::stored_delivery_receipt::StoredDeliveryReceipt>(&request.delivery_id)
        .await
        .map_err(failure::storage)
}

async fn read_run(
    store: &Arc<Mutex<AutomationStore>>,
    request: &RunShowRequest,
) -> InspectionResult<
    agent_automation::RunRecord<
        SessionRef,
        EndpointRef,
        CodexGeneration,
        crate::stored_run_receipt::StoredRunReceipt,
    >,
> {
    store
        .lock()
        .await
        .read_run::<SessionRef, EndpointRef, CodexGeneration, crate::stored_run_receipt::StoredRunReceipt>(&request.run_id)
        .await
        .map_err(failure::storage)
}

fn bounded_reconciliation<TResult: serde::Serialize>(
    result: TResult,
    budget: ResultByteBudget,
) -> InspectionResult<TResult> {
    if budget.admits(&result) {
        Ok(result)
    } else {
        Err(failure::invalid(
            "response",
            "Reconciled evidence exceeds the response limit. Inspect the affected resource; no input was resent.",
        ))
    }
}

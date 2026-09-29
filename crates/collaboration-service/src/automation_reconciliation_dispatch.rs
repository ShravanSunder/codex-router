//! Reconciliation reads exact native evidence and never repeats a native mutation.
use crate::{ServiceIdentity, automation_inspection_failure as failure};
use collaboration_protocol::{CodexGeneration, DeliveryShowRequest, SessionRef};
use serde_json::{Value, json};

pub(crate) async fn delivery(id: Value, params: Value, identity: &ServiceIdentity) -> Value {
    let Some(store) = identity.automation.as_ref() else {
        return failure::response(id, failure::unavailable());
    };
    let params: DeliveryShowRequest = match serde_json::from_value(params) {
        Ok(params) => params,
        Err(_) => {
            return failure::response(
                id,
                failure::invalid(
                    "deliveryId",
                    "Provide the exact UUIDv7 delivery identity; reconciliation never resends input.",
                ),
            );
        }
    };
    let record = match store
        .lock()
        .await
        .read_delivery::<SessionRef, CodexGeneration, crate::stored_delivery_receipt::StoredDeliveryReceipt>(&params.delivery_id)
        .await
    {
        Ok(record) => record,
        Err(error) => return failure::response(id, failure::storage(error)),
    };
    let Some(session_delivery) = identity.session_delivery.as_ref() else {
        return failure::response(id, failure::unavailable());
    };
    if let Err(error) =
        crate::delivery_reconciliation::reconcile(store, session_delivery.as_ref(), record).await
    {
        return failure::response(id, failure::storage(error));
    }
    let record = match store
        .lock()
        .await
        .read_delivery::<SessionRef, CodexGeneration, crate::stored_delivery_receipt::StoredDeliveryReceipt>(&params.delivery_id)
        .await
    {
        Ok(record) => record,
        Err(error) => return failure::response(id, failure::storage(error)),
    };
    match crate::delivery_projection::snapshot(record) {
        Ok(mut snapshot) => {
            if let collaboration_protocol::DeliveryEvidence::OutcomeUnknown {
                explanation, ..
            } = &mut snapshot.evidence
            {
                *explanation = "Read-only reconciliation did not establish a complete native receipt. Missing, ambiguous or unavailable queue evidence does not prove non-submission. Inspect the exact target and attempt; no input was resent.".into();
            }
            bounded_response(id, snapshot)
        }
        Err(()) => failure::response(
            id,
            failure::storage(automation_storage::StorageError::InvalidRecord),
        ),
    }
}
pub(crate) async fn run(id: Value, params: Value, identity: &ServiceIdentity) -> Value {
    let Some(store) = identity.automation.as_ref() else {
        return failure::response(id, failure::unavailable());
    };
    let params: collaboration_protocol::RunShowRequest = match serde_json::from_value(params) {
        Ok(params) => params,
        Err(_) => {
            return failure::response(
                id,
                failure::invalid(
                    "runId",
                    "Provide the exact UUIDv7 Run identity; reconciliation never starts or interrupts work.",
                ),
            );
        }
    };
    let record = match store.lock().await.read_run::<SessionRef, collaboration_protocol::EndpointRef, CodexGeneration, crate::stored_run_receipt::StoredRunReceipt>(&params.run_id).await {
        Ok(record) => record,
        Err(error) => return failure::response(id, failure::storage(error)),
    };
    if let Err(error) = crate::run_reconciliation::reconcile(
        store,
        identity.scheduled_run_execution.as_ref(),
        identity.native_backend.as_ref(),
        record,
    )
    .await
    {
        return failure::response(id, failure::storage(error));
    }
    let record = match store.lock().await.read_run::<SessionRef, collaboration_protocol::EndpointRef, CodexGeneration, crate::stored_run_receipt::StoredRunReceipt>(&params.run_id).await {
        Ok(record) => record,
        Err(error) => return failure::response(id, failure::storage(error)),
    };
    match crate::run_projection::snapshot(record) {
        Ok(snapshot) => bounded_response(id, snapshot),
        Err(()) => failure::response(
            id,
            failure::storage(automation_storage::StorageError::InvalidRecord),
        ),
    }
}
fn bounded_response(id: Value, result: impl serde::Serialize) -> Value {
    let response = json!({"jsonrpc":"2.0","id":id,"result":result});
    match serde_json::to_vec(&response) {
        Ok(bytes) if bytes.len() <= collaboration_protocol::MAX_CONTROL_FRAME_BYTES => response,
        _ => failure::response(
            id,
            failure::invalid(
                "response",
                "Reconciled evidence exceeds the Control frame budget. Inspect the affected resource; no input was resent.",
            ),
        ),
    }
}

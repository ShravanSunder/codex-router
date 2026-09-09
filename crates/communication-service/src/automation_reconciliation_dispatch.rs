//! Reconciliation reads exact native evidence and never repeats a native mutation.
use crate::{ServiceIdentity, automation_inspection_failure as failure};
use communication_protocol::{CodexGeneration, DeliveryShowRequest, NativeSendReceipt, SessionRef};
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
        .read_delivery::<SessionRef, CodexGeneration, NativeSendReceipt>(&params.delivery_id)
        .await
    {
        Ok(record) => record,
        Err(error) => return failure::response(id, failure::storage(error)),
    };
    if let Err(error) =
        crate::delivery_reconciliation::reconcile(store, identity.native_backend.as_ref(), record)
            .await
    {
        return failure::response(id, failure::storage(error));
    }
    let record = match store
        .lock()
        .await
        .read_delivery::<SessionRef, CodexGeneration, NativeSendReceipt>(&params.delivery_id)
        .await
    {
        Ok(record) => record,
        Err(error) => return failure::response(id, failure::storage(error)),
    };
    match crate::delivery_projection::snapshot(record) {
        Ok(mut snapshot) => {
            if let communication_protocol::DeliveryEvidence::OutcomeUnknown {
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
    let params: communication_protocol::RunShowRequest = match serde_json::from_value(params) {
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
    let record = match store.lock().await.read_run::<SessionRef, communication_protocol::EndpointRef, CodexGeneration, NativeSendReceipt>(&params.run_id).await {
        Ok(record) => record,
        Err(error) => return failure::response(id, failure::storage(error)),
    };
    if let Err(error) =
        crate::run_reconciliation::reconcile(store, identity.native_backend.as_ref(), record).await
    {
        return failure::response(id, failure::storage(error));
    }
    let record = match store.lock().await.read_run::<SessionRef, communication_protocol::EndpointRef, CodexGeneration, NativeSendReceipt>(&params.run_id).await {
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
        Ok(bytes) if bytes.len() <= communication_protocol::MAX_CONTROL_FRAME_BYTES => response,
        _ => failure::response(
            id,
            failure::invalid(
                "response",
                "Reconciled evidence exceeds the Control frame budget. Inspect the affected resource; no input was resent.",
            ),
        ),
    }
}

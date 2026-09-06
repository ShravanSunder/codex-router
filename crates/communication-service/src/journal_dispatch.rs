//! Public lifecycle reads; blocking waits are owned by independent admitted request tasks.
use communication_protocol::{EndpointDescription, JournalStatus, UuidIdentity};
use lifecycle_observation::{JournalError, LifecycleStore};
use serde_json::{Value, json};
use std::sync::Arc;
fn failure(id: Value, kind: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32050,"message":"Lifecycle read unavailable","data":{"kind":kind,"stage":"discovery","message":"Lifecycle read unavailable"}}})
}
fn invalid(id: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32602,"message":"Invalid lifecycle parameters"}})
}
pub(crate) async fn dispatch_journal(
    method: &str,
    params: Value,
    id: Value,
    service_id: UuidIdentity,
    store: Option<Arc<LifecycleStore>>,
    endpoints: Vec<EndpointDescription>,
) -> Value {
    if method == "lifecycleJournal/status" {
        if params != json!({}) {
            return invalid(id);
        }
        let result = match store {
            Some(store) => match store.bounds().await {
                Ok(bounds) => json!(JournalStatus::Available { bounds }),
                Err(_) => json!(JournalStatus::Unavailable),
            },
            None => json!(JournalStatus::Unavailable),
        };
        return json!({"jsonrpc":"2.0","id":id,"result":result});
    }
    if method == "addressBook/list" {
        let Ok(params) =
            serde_json::from_value::<communication_protocol::AddressListParams>(params)
        else {
            return invalid(id);
        };
        if !(1..=100).contains(&params.page_size) {
            return invalid(id);
        }
        if params.endpoint.service_id != service_id {
            return failure(id, "wrongService");
        }
        if !endpoints
            .iter()
            .any(|entry| entry.endpoint == params.endpoint)
        {
            return failure(id, "endpointNotFound");
        }
        let Some(store) = store else {
            return failure(id, "unavailable");
        };
        let Ok(snapshot_id) = crate::new_service_uuid() else {
            return failure(id, "unavailable");
        };
        let Ok(at) = communication_protocol::ObservationTimestamp::try_from(
            chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        ) else {
            return failure(id, "unavailable");
        };
        let Ok(page_size) = usize::try_from(params.page_size) else {
            return invalid(id);
        };
        return match store
            .address_snapshot(
                &params.endpoint,
                page_size,
                params.cursor.as_deref(),
                snapshot_id,
                at,
            )
            .await
        {
            Ok(page) => json!({"jsonrpc":"2.0","id":id,"result":page}),
            Err(JournalError::SnapshotExpired) => {
                json!({"jsonrpc":"2.0","id":id,"error":{"code":-32050,"message":"Snapshot expired","data":{"kind":"snapshotExpired"}}})
            }
            Err(JournalError::InvalidRecord) => invalid(id),
            Err(JournalError::Capacity) => failure(id, "overloaded"),
            Err(_) => failure(id, "unavailable"),
        };
    }
    let Ok(params) = serde_json::from_value::<communication_protocol::JournalReadParams>(params)
    else {
        return invalid(id);
    };
    if !(1..=100).contains(&params.page_size)
        || params.wait_milliseconds > 30000
        || params.after.sequence > 9_007_199_254_740_991
    {
        return invalid(id);
    }
    if params.endpoint.service_id != service_id {
        return failure(id, "wrongService");
    }
    if !endpoints
        .iter()
        .any(|entry| entry.endpoint == params.endpoint)
    {
        return failure(id, "endpointNotFound");
    }
    let Some(store) = store else {
        return failure(id, "unavailable");
    };
    match store
        .read_wait(
            &params.endpoint,
            params.after,
            params.page_size,
            params.wait_milliseconds,
        )
        .await
    {
        Ok(page) => json!({"jsonrpc":"2.0","id":id,"result":page}),
        Err(JournalError::InvalidRecord) => invalid(id),
        Err(error @ (JournalError::HistoryExpired | JournalError::JournalChanged)) => {
            match store.bounds().await {
                Ok(bounds) => {
                    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32050,"message":"Journal cursor invalidated","data":{"kind":if matches!(error,JournalError::HistoryExpired){"historyExpired"}else{"journalChanged"},"current":bounds}}})
                }
                Err(_) => failure(id, "unavailable"),
            }
        }
        Err(_) => failure(id, "unavailable"),
    }
}

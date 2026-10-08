//! Lifecycle journal and address-book Control dispatch over the typed session operations.
use crate::ServiceIdentity;
use crate::collaboration_application::{JournalFailure, SessionOperations};
use serde_json::{Value, json};

fn invalid(id: Value) -> Value {
    crate::control_connection::rejection_response(id, &JournalFailure::InvalidRequest)
}

pub(crate) async fn dispatch_journal(
    method: &str,
    params: Value,
    id: Value,
    identity: &ServiceIdentity,
) -> Value {
    let sessions = SessionOperations::new(identity);
    let result = match method {
        "lifecycleJournal/status" => {
            if params != json!({}) {
                return invalid(id);
            }
            Ok(json!(sessions.journal_status().await))
        }
        "addressBook/list" => {
            let Ok(params) =
                serde_json::from_value::<collaboration_protocol::AddressListParams>(params)
            else {
                return invalid(id);
            };
            sessions.address_list(params).await.map(|page| json!(page))
        }
        _ => {
            let Ok(params) =
                serde_json::from_value::<collaboration_protocol::JournalReadParams>(params)
            else {
                return invalid(id);
            };
            sessions.journal_read(params).await.map(|page| json!(page))
        }
    };
    match result {
        Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
        Err(failure) => crate::control_connection::rejection_response(id, &failure),
    }
}

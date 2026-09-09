//! Configuration RPC delegates filesystem ownership to Host and reports unavailable admission honestly.
use crate::{AutomationConfigurationBackend, AutomationConfigurationHandle};
use communication_protocol::{
    AutomationConfigureRequest, AutomationStatus, ConfigurationFailure, ConfigurationFailureKind,
    ConfigurationFileState, ConfigurationNextAction,
};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::Mutex;
pub(crate) struct ConfigurationRequest<'a> {
    pub id: Value,
    pub method: &'a str,
    pub params: Value,
    pub handle: &'a AutomationConfigurationHandle,
    pub backend: Option<&'a Arc<dyn AutomationConfigurationBackend>>,
    pub store: Option<&'a Arc<Mutex<automation_storage::AutomationStore>>>,
    pub service_id: &'a communication_protocol::UuidIdentity,
}
pub(crate) async fn dispatch(request: ConfigurationRequest<'_>) -> Value {
    if request.method == "automation/status" {
        if request.params != json!({}) {
            return json!({"jsonrpc":"2.0","id":request.id,"error":{"code":-32602,"message":"automation/status requires an empty object"}});
        }
        let floor = if let Some(store) = request.store {
            store
                .lock()
                .await
                .automation_event_floor(chrono::Utc::now().timestamp_millis())
                .await
                .ok()
                .flatten()
        } else {
            None
        };
        let cursor = floor.and_then(|(sequence, observed)| {
            serde_json::to_string(&(
                1_u8,
                request.service_id,
                "automation-events",
                sequence.saturating_sub(1),
                observed,
            ))
            .ok()
        });
        return json!({"jsonrpc":"2.0","id":request.id,"result":AutomationStatus{storage_available:request.store.is_some(),configuration:request.handle.current().await,earliest_retained_event_cursor:cursor}});
    }
    let params=match serde_json::from_value::<AutomationConfigureRequest>(request.params){Ok(params)=>params,Err(_)=>return failure(request.id,ConfigurationFailure{kind:ConfigurationFailureKind::InvalidField,message:"Provide operationId and positive integer executionTimeoutSeconds/summaryTimeoutSeconds (1..31536000).".into(),operation_id:None,file_state:ConfigurationFileState::NotReplaced,next_action:ConfigurationNextAction::CorrectRequest})};
    let Some(backend) = request.backend else {
        return failure(
            request.id,
            ConfigurationFailure {
                kind: ConfigurationFailureKind::AutomationUnavailable,
                message: "Host configuration backend unavailable; no file replacement dispatched."
                    .into(),
                operation_id: Some(params.operation_id),
                file_state: ConfigurationFileState::NotReplaced,
                next_action: ConfigurationNextAction::RetryLater,
            },
        );
    };
    match backend.configure(params).await {
        Ok(result) => json!({"jsonrpc":"2.0","id":request.id,"result":result}),
        Err(error) => failure(request.id, error),
    }
}
fn failure(id: Value, data: ConfigurationFailure) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32050,"message":"Automation configuration failed","data":data}})
}

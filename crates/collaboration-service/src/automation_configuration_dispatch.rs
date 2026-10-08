//! Automation configuration Control dispatch over the typed automation operations.
use crate::ServiceIdentity;
use crate::collaboration_application::AutomationOperations;
use collaboration_protocol::{
    AutomationConfigureRequest, ConfigurationFailure, ConfigurationFailureKind,
    ConfigurationFileState, ConfigurationNextAction,
};
use serde_json::{Value, json};
pub(crate) async fn dispatch(
    id: Value,
    method: &str,
    params: Value,
    identity: &ServiceIdentity,
) -> Value {
    let automation = AutomationOperations::new(identity);
    if method == "automation/status" {
        if params != json!({}) {
            return json!({"jsonrpc":"2.0","id":id,"error":{"code":-32602,"message":"automation/status requires an empty object"}});
        }
        return json!({"jsonrpc":"2.0","id":id,"result":automation.automation_status().await});
    }
    let params=match serde_json::from_value::<AutomationConfigureRequest>(params){Ok(params)=>params,Err(_)=>return failure(id,ConfigurationFailure{kind:ConfigurationFailureKind::InvalidField,message:"Provide operationId and positive integer executionTimeoutSeconds/summaryTimeoutSeconds (1..31536000).".into(),operation_id:None,file_state:ConfigurationFileState::NotReplaced,next_action:ConfigurationNextAction::CorrectRequest})};
    match automation.automation_configure(params).await {
        Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
        Err(error) => failure(id, error),
    }
}
fn failure(id: Value, data: ConfigurationFailure) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32050,"message":"Automation configuration failed","data":data}})
}

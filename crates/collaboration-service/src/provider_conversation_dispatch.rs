//! Conversation-family Control dispatch: decodes each request and calls the typed
//! conversation operations.
use crate::ServiceIdentity;
use crate::collaboration_application::{ConversationFailure, ConversationOperations};
use collaboration_protocol::{
    ConversationOperationFailure, ConversationOperationFailureKind,
    ConversationOperationFailureStage, NonEmptyText, OperationId, ProviderInspectFailure,
    ProviderInspectFailureKind, ProviderOperationEffect, ProviderSettingsFailure,
    ProviderSettingsFailureKind, SessionRef,
};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

pub(crate) async fn dispatch(
    id: Value,
    method: &str,
    params: Value,
    identity: &ServiceIdentity,
) -> Value {
    let conversations = ConversationOperations::new(identity);
    macro_rules! call {
        ($respond:ident, $operation:ident) => {{
            let Ok(request) = parse(params) else {
                return invalid_params(id);
            };
            $respond(id, conversations.$operation(request).await)
        }};
    }
    match method {
        "conversation/create" => call!(conversation_response, conversation_create),
        "conversation/load" => call!(conversation_response, conversation_load),
        "conversation/resume" => call!(conversation_response, conversation_resume),
        "conversation/close" => call!(conversation_response, conversation_close),
        "conversation/prompt" => call!(conversation_response, conversation_prompt),
        "conversation/cancel" => call!(conversation_response, conversation_cancel),
        "conversation/settingsSet" => call!(settings_response, settings_set),
        "conversation/settingsAccept" => call!(settings_response, settings_accept),
        "provider/sessionInspect" => call!(inspect_response, provider_session_inspect),
        "conversation/operationShow" => call!(conversation_response, operation_show),
        "conversation/operationWait" => call!(conversation_response, operation_wait),
        "conversation/operationReconcile" => {
            call!(conversation_response, operation_reconcile)
        }
        _ => json_rpc_error(id, -32601, "Method not found"),
    }
}

pub(crate) fn overloaded(id: Value, method: &str, params: Value) -> Value {
    const MESSAGE: &str = "Request capacity exceeded; this request was not dispatched. Reconnect and retry the same operation identity when capacity is available.";
    let target = params
        .get("target")
        .cloned()
        .and_then(|value| serde_json::from_value::<SessionRef>(value).ok());
    if method == "provider/sessionInspect" {
        return inspect_failure_response(
            id,
            ProviderInspectFailure {
                kind: ProviderInspectFailureKind::Overloaded,
                stage: Some(ConversationOperationFailureStage::Discovery),
                target,
                message: MESSAGE.into(),
            },
        );
    }
    if matches!(
        method,
        "conversation/settingsSet" | "conversation/settingsAccept"
    ) {
        return settings_failure_response(
            id,
            ProviderSettingsFailure {
                kind: ProviderSettingsFailureKind::Overloaded,
                stage: Some(ConversationOperationFailureStage::Discovery),
                target,
                message: MESSAGE.into(),
                setting: None,
                value: None,
                advertised: Vec::new(),
            },
        );
    }
    if !method.starts_with("conversation/") {
        return json_rpc_error(id, -32601, "Method not found");
    }
    let Some(message) = NonEmptyText::try_from(MESSAGE.to_owned()).ok() else {
        return json_rpc_error(id, -32603, "Internal error");
    };
    failure_response(
        id,
        ConversationOperationFailure {
            kind: ConversationOperationFailureKind::Overloaded,
            stage: ConversationOperationFailureStage::Discovery,
            effect: ProviderOperationEffect::None,
            message,
            operation_id: params
                .get("operationId")
                .cloned()
                .and_then(|value| serde_json::from_value::<OperationId>(value).ok()),
            invalid_setting: None,
            provider_code: None,
            target,
            endpoint: None,
            availability: None,
        },
    )
}

fn inspect_response(
    id: Value,
    result: Result<collaboration_protocol::ProviderSessionInspectResult, ProviderInspectFailure>,
) -> Value {
    match result {
        Ok(result) => success(id, result),
        Err(failure) => inspect_failure_response(id, failure),
    }
}

fn inspect_failure_response(id: Value, failure: ProviderInspectFailure) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32050,"message":failure.message,"data":failure}})
}

fn settings_response(
    id: Value,
    result: Result<collaboration_protocol::ProviderSettingsResult, ProviderSettingsFailure>,
) -> Value {
    match result {
        Ok(result) => success(id, result),
        Err(failure) => settings_failure_response(id, failure),
    }
}

fn settings_failure_response(id: Value, failure: ProviderSettingsFailure) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32050,"message":failure.message,"data":failure}})
}

fn parse<T: DeserializeOwned>(params: Value) -> Result<T, ()> {
    serde_json::from_value(params).map_err(|_| ())
}

fn success(id: Value, result: impl serde::Serialize) -> Value {
    json!({"jsonrpc":"2.0","id":id,"result":result})
}

fn conversation_response(
    id: Value,
    result: Result<impl serde::Serialize, ConversationFailure>,
) -> Value {
    match result {
        Ok(result) => success(id, result),
        Err(ConversationFailure::Operation(failure)) => failure_response(id, failure),
        Err(
            failure @ (ConversationFailure::InvalidStoredOperation
            | ConversationFailure::UndescribableFailure),
        ) => json_rpc_error(id, -32603, &failure.to_string()),
    }
}

fn invalid_params(id: Value) -> Value {
    json_rpc_error(id, -32602, "Invalid params")
}
fn json_rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
}
fn failure_response(id: Value, failure: ConversationOperationFailure) -> Value {
    let message = String::from(failure.message.clone());
    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32050,"message":message,"data":failure}})
}

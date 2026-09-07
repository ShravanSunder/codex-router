//! Generation-scoped Control calls; no provider policy or native process ownership.
use crate::NativeGenerationGate;
use codex_native_integration::{NativeConnectionError, NativeOperation, NativeProtocolConnection};
use communication_protocol::{
    CodexGeneration, EndpointDescription, EndpointRef, NativeInspectParams, NativeInterruptParams,
    SessionRef, UuidIdentity,
};
use serde_json::{Value, json};

#[derive(Clone)]
pub struct NativeControlBackend {
    pub endpoint: EndpointRef,
    pub gate: NativeGenerationGate,
    pub codex_home: std::path::PathBuf,
}

pub(crate) struct NativeControlRequest<'a> {
    pub method: &'a str,
    pub params: Value,
    pub id: Value,
    pub service_id: &'a UuidIdentity,
    pub backend: Option<&'a NativeControlBackend>,
    pub endpoints: &'a [EndpointDescription],
    pub stored_observation:
        Option<crate::stored_inventory_observation::StoredInventoryObservation<'a>>,
}
pub(crate) async fn dispatch_native(request: NativeControlRequest<'_>) -> Value {
    if request.method == "codex/sessionList" {
        return crate::session_inventory_dispatch::dispatch_inventory(request).await;
    }
    if request.method == "codex/messageSend" {
        return crate::native_message_dispatch::dispatch_message(request).await;
    }
    let NativeControlRequest {
        method,
        params,
        id,
        service_id,
        backend,
        endpoints,
        ..
    } = request;
    let stage = if method == "codex/sessionInspect" {
        "inspect"
    } else {
        "interrupt"
    };
    let (target, generation, operation, native_params, turn_id) = match method {
        "codex/sessionInspect" => {
            let Ok(params) = serde_json::from_value::<NativeInspectParams>(params) else {
                return invalid(id);
            };
            let native = json!({"threadId":String::from(params.target.session_id.clone()),"includeTurns":false});
            (
                params.target,
                None,
                NativeOperation::ReadThread,
                native,
                None,
            )
        }
        "codex/turnInterrupt" => {
            let Ok(params) = serde_json::from_value::<NativeInterruptParams>(params) else {
                return invalid(id);
            };
            let native = json!({"threadId":String::from(params.target.session_id.clone()),"turnId":String::from(params.turn_id.clone())});
            (
                params.target,
                Some(params.generation),
                NativeOperation::InterruptTurn,
                native,
                Some(params.turn_id),
            )
        }
        _ => return invalid(id),
    };
    if &target.endpoint.service_id != service_id {
        return failure(id, "wrongService", stage);
    }
    let Some(endpoint) = endpoints
        .iter()
        .find(|endpoint| endpoint.endpoint == target.endpoint)
    else {
        return failure(id, "endpointNotFound", stage);
    };
    let Some((advertised_digest, advertised_generation)) =
        endpoint.channels.iter().find_map(|channel| match channel {
            communication_protocol::ChannelDescription::NativeCodex {
                schema_digest,
                generation,
                ..
            } => Some((schema_digest, generation)),
            _ => None,
        })
    else {
        return failure(id, "unsupportedCapability", stage);
    };
    let Some(backend) = backend.filter(|backend| backend.endpoint == target.endpoint) else {
        return failure(id, "unsupportedCapability", stage);
    };
    let Ok(admission) = backend.gate.acquire() else {
        return failure(id, "unavailable", stage);
    };
    if generation
        .as_ref()
        .is_some_and(|generation| generation != admission.generation())
    {
        return failure(id, "staleGeneration", stage);
    }
    let Some(schemas) = admission.schemas() else {
        return failure(id, "unsupportedCapability", stage);
    };
    if advertised_generation.as_ref() != Some(admission.generation()) {
        return failure(id, "unavailable", stage);
    }
    if advertised_digest
        .as_ref()
        .map(|digest| String::from(digest.clone()))
        .as_deref()
        != Some(schemas.schema_digest())
    {
        return failure(id, "unsupportedCapability", stage);
    }
    let retired = admission.retirement();
    let connection = tokio::select! {
        biased;
        _ = retired.cancelled() => return failure(id, "unavailable", stage),
        connection = NativeProtocolConnection::connect(admission.backend_path()) => connection,
    };
    let Ok(mut connection) = connection else {
        return failure(id, "unavailable", stage);
    };
    if retired.is_cancelled() {
        return failure(id, "unavailable", stage);
    }
    let result = tokio::select! {
        biased;
        result = connection.request_validated(&schemas, operation, native_params) => result,
        _ = retired.cancelled() => Err(NativeConnectionError::OutcomeUnknown),
    };
    match result {
        Ok(result) => success(NativeControlSuccess {
            id,
            stage,
            target,
            generation: admission.generation().clone(),
            turn_id,
            result,
        }),
        Err(NativeConnectionError::InvalidInput) => invalid(id),
        Err(NativeConnectionError::Rejected { .. }) => failure(id, "nativeRejected", stage),
        Err(NativeConnectionError::Unavailable) => failure(id, "unavailable", stage),
        Err(_) => failure(id, "outcomeUnknown", stage),
    }
}

struct NativeControlSuccess {
    id: Value,
    stage: &'static str,
    target: SessionRef,
    generation: CodexGeneration,
    turn_id: Option<communication_protocol::NonEmptyText>,
    result: Value,
}
fn success(success: NativeControlSuccess) -> Value {
    let NativeControlSuccess {
        id,
        stage,
        target,
        generation,
        turn_id,
        result,
    } = success;
    let result = if stage == "inspect" {
        let Some(thread) = result.get("thread") else {
            return failure(id, "outcomeUnknown", stage);
        };
        if thread.get("id").and_then(Value::as_str)
            != Some(String::from(target.session_id.clone()).as_str())
        {
            return failure(id, "outcomeUnknown", stage);
        }
        json!(communication_protocol::NativeInspectResult {
            target,
            generation,
            thread: thread.clone()
        })
    } else {
        let Some(turn_id) = turn_id else {
            return failure(id, "outcomeUnknown", stage);
        };
        if result != json!({}) {
            return failure(id, "outcomeUnknown", stage);
        }
        json!(communication_protocol::NativeInterruptResult {
            target,
            generation,
            turn_id,
            kind: communication_protocol::NativeInterruptKind::InterruptCompleted
        })
    };
    let response = json!({"jsonrpc":"2.0","id":id,"result":result});
    if serde_json::to_vec(&response)
        .is_ok_and(|bytes| bytes.len() <= communication_protocol::MAX_CONTROL_FRAME_BYTES)
    {
        response
    } else {
        failure(id, "overloaded", stage)
    }
}
fn invalid(id: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32602,"message":"Invalid native control parameters"}})
}
fn failure(id: Value, kind: &str, stage: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32050,"message":"Native control operation failed","data":{"kind":kind,"stage":stage,"message":"Native control operation failed"}}})
}

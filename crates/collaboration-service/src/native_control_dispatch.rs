//! Generation-scoped Control calls; no provider policy or native process ownership.
use crate::NativeGenerationGate;
use codex_native_integration::{NativeConnectionError, NativeOperation, NativeProtocolConnection};
use collaboration_protocol::{
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
    /// Recorded Router access routes. `session inspect` reports the access the
    /// broker holds for a thread; a thread without a route has none to report.
    pub access_routes: Option<&'a crate::ServiceApprovalBroker>,
}
pub(crate) async fn dispatch_native(request: NativeControlRequest<'_>) -> Value {
    if request.method == "codex/sessionList" {
        return crate::session_inventory_dispatch::dispatch_inventory(request).await;
    }
    if request.method == "codex/messageSend" {
        return crate::native_message_dispatch::dispatch_message(request).await;
    }
    if request.method == "codex/sessionRename" {
        return dispatch_rename(request).await;
    }
    let NativeControlRequest {
        method,
        params,
        id,
        service_id,
        backend,
        endpoints,
        access_routes,
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
            collaboration_protocol::ChannelDescription::NativeCodex {
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
    let effective_access = match access_routes {
        Some(routes) if stage == "inspect" => {
            recorded_access(routes, String::from(target.session_id.clone()).as_str()).await
        }
        _ => None,
    };
    match result {
        Ok(result) => success(NativeControlSuccess {
            id,
            stage,
            target,
            generation: admission.generation().clone(),
            turn_id,
            result,
            effective_access,
        }),
        Err(error) => {
            let native = connection.take_last_rejection();
            native_call_failure(id, stage, stage != "inspect", &error, native.as_ref())
        }
    }
}

async fn dispatch_rename(request: NativeControlRequest<'_>) -> Value {
    let Ok(params) =
        serde_json::from_value::<collaboration_protocol::NativeRenameParams>(request.params)
    else {
        return invalid(request.id);
    };
    let scalar_count = params.name.chars().count();
    if params.name.trim() != params.name
        || !(1..=120).contains(&scalar_count)
        || params.name.chars().any(char::is_control)
    {
        return invalid(request.id);
    }
    if &params.target.endpoint.service_id != request.service_id {
        return failure(request.id, "wrongService", "rename");
    }
    let Some(endpoint) = request
        .endpoints
        .iter()
        .find(|entry| entry.endpoint == params.target.endpoint)
    else {
        return failure(request.id, "endpointNotFound", "rename");
    };
    let Some((advertised_digest, advertised_generation)) =
        endpoint.channels.iter().find_map(|channel| match channel {
            collaboration_protocol::ChannelDescription::NativeCodex {
                schema_digest,
                generation,
                ..
            } => Some((schema_digest, generation)),
            _ => None,
        })
    else {
        return failure(request.id, "unsupportedCapability", "rename");
    };
    let Some(backend) = request
        .backend
        .filter(|backend| backend.endpoint == params.target.endpoint)
    else {
        return failure(request.id, "unsupportedCapability", "rename");
    };
    let Ok(admission) = backend.gate.acquire() else {
        return failure(request.id, "unavailable", "rename");
    };
    let Some(schemas) = admission.schemas() else {
        return failure(request.id, "unsupportedCapability", "rename");
    };
    if advertised_generation.as_ref() != Some(admission.generation())
        || advertised_digest
            .as_ref()
            .map(|value| String::from(value.clone()))
            .as_deref()
            != Some(schemas.schema_digest())
        || !schemas.supports_operation(NativeOperation::SetThreadName)
    {
        return failure(request.id, "unsupportedCapability", "rename");
    }
    let Ok(mut connection) = NativeProtocolConnection::connect(admission.backend_path()).await
    else {
        return failure(request.id, "unavailable", "rename");
    };
    let thread_id = String::from(params.target.session_id.clone());
    let before = connection
        .request_validated(
            &schemas,
            NativeOperation::ReadThread,
            json!({"threadId":thread_id,"includeTurns":false}),
        )
        .await;
    let before = match before {
        Ok(before) => before,
        Err(error) => {
            let native = connection.take_last_rejection();
            return native_call_failure(request.id, "rename", false, &error, native.as_ref());
        }
    };
    let previous_name = before
        .pointer("/thread/name")
        .and_then(Value::as_str)
        .map(str::to_owned);
    if let Err(error) = connection
        .request_validated(
            &schemas,
            NativeOperation::SetThreadName,
            json!({"threadId":thread_id,"name":params.name}),
        )
        .await
    {
        let native = connection.take_last_rejection();
        return native_call_failure(request.id, "rename", true, &error, native.as_ref());
    }
    let after = connection
        .request_validated(
            &schemas,
            NativeOperation::ReadThread,
            json!({"threadId":thread_id,"includeTurns":false}),
        )
        .await;
    let after = match after {
        Ok(after) => after,
        Err(error) => {
            let native = connection.take_last_rejection();
            return native_call_failure(request.id, "rename", true, &error, native.as_ref());
        }
    };
    let Some(name) = after.pointer("/thread/name").and_then(Value::as_str) else {
        return failure(request.id, "outcomeUnknown", "rename");
    };
    if name != params.name {
        return rename_echo_mismatch(request.id, &params.name, name);
    }
    let result = collaboration_protocol::NativeRenameResult {
        target: params.target,
        name: name.to_owned(),
        previous_name,
    };
    json!({"jsonrpc":"2.0","id":request.id,"result":result})
}

/// Returns the access Router recorded for this thread, if the broker holds one.
async fn recorded_access(
    routes: &crate::ServiceApprovalBroker,
    thread_id: &str,
) -> Option<collaboration_protocol::RouterAccess> {
    use codex_acp_adapter::ApprovalBroker;
    routes
        .route(thread_id)
        .await
        .ok()
        .flatten()
        .map(|route| route.access)
}

struct NativeControlSuccess {
    id: Value,
    stage: &'static str,
    target: SessionRef,
    generation: CodexGeneration,
    turn_id: Option<collaboration_protocol::NonEmptyText>,
    result: Value,
    effective_access: Option<collaboration_protocol::RouterAccess>,
}
fn success(success: NativeControlSuccess) -> Value {
    let NativeControlSuccess {
        id,
        stage,
        target,
        generation,
        turn_id,
        result,
        effective_access,
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
        json!(collaboration_protocol::NativeInspectResult {
            target,
            generation,
            effective_access,
            settings_observation: collaboration_protocol::SettingsObservation::Unavailable {
                reason: collaboration_protocol::SettingsUnavailableReason::ThreadReadOmitsSettings,
            },
            thread: thread.clone()
        })
    } else {
        let Some(turn_id) = turn_id else {
            return failure(id, "outcomeUnknown", stage);
        };
        if result != json!({}) {
            return failure(id, "outcomeUnknown", stage);
        }
        json!(collaboration_protocol::NativeInterruptResult {
            target,
            generation,
            turn_id,
            kind: collaboration_protocol::NativeInterruptKind::InterruptCompleted
        })
    };
    let response = json!({"jsonrpc":"2.0","id":id,"result":result});
    if serde_json::to_vec(&response)
        .is_ok_and(|bytes| bytes.len() <= collaboration_protocol::MAX_CONTROL_FRAME_BYTES)
    {
        response
    } else {
        failure(id, "overloaded", stage)
    }
}
/// The name the runtime echoed is not the name Router asked for: report both.
fn rename_echo_mismatch(id: Value, requested: &str, effective: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32050,"message":"Native session rename failed","data":{"kind":"nameMismatch","requested":requested,"effective":effective,"stage":"rename"}}})
}

fn invalid(id: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32602,"message":"Invalid native control parameters"}})
}
/// Projects one failed native control call.
///
/// A refusal keeps its reason and corrective action from the shared closed set.
/// After a dispatched mutation, a lost or unreadable outcome is unknown, never a
/// refusal; before one, it is plain unavailability.
fn native_call_failure(
    id: Value,
    stage: &'static str,
    mutation: bool,
    error: &NativeConnectionError,
    native: Option<&Value>,
) -> Value {
    match error {
        NativeConnectionError::InvalidInput => invalid(id),
        NativeConnectionError::Rejected { code } => {
            let (reason, next_action) =
                crate::message_effect_state::classify_native_rejection(*code, native);
            let mut data = json!({"kind":"nativeRejected","stage":stage,
                "message":"Native control operation failed",
                "reason":reason,"nextAction":next_action});
            if reason == "unknown"
                && let Some(fields) = data.as_object_mut()
            {
                fields.insert("nativeCode".into(), json!(code));
            }
            json!({"jsonrpc":"2.0","id":id,"error":{"code":-32050,"message":"Native control operation failed","data":data}})
        }
        NativeConnectionError::Unavailable if !mutation => failure(id, "unavailable", stage),
        _ => failure(id, "outcomeUnknown", stage),
    }
}

fn failure(id: Value, kind: &str, stage: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32050,"message":"Native control operation failed","data":{"kind":kind,"stage":stage,"message":"Native control operation failed"}}})
}

#[cfg(test)]
mod native_failure_tests {
    use super::{NativeConnectionError, native_call_failure, rename_echo_mismatch};
    use serde_json::json;

    #[test]
    fn every_native_error_class_keeps_its_own_projection_on_the_rename_path() {
        // Arrange: one refusal with native evidence, plus the transport classes.
        let rejection = json!({"error":{"message":"thread has an active turn"}});

        // Act & assert: a refusal carries its reason and corrective action.
        let refused = native_call_failure(
            json!("1"),
            "rename",
            true,
            &NativeConnectionError::Rejected { code: -32000 },
            Some(&rejection),
        );
        assert_eq!(refused["error"]["data"]["kind"], "nativeRejected");
        assert_eq!(refused["error"]["data"]["reason"], "busy");
        assert_eq!(refused["error"]["data"]["nextAction"], "useDeliverySteer");

        // Assert: an unclassified refusal names the native code instead of guessing.
        let unknown = native_call_failure(
            json!("1"),
            "rename",
            true,
            &NativeConnectionError::Rejected { code: -32099 },
            None,
        );
        assert_eq!(unknown["error"]["data"]["reason"], "unknown");
        assert_eq!(unknown["error"]["data"]["nativeCode"], -32099);

        // Assert: invalid input is a request defect, not a native refusal.
        assert_eq!(
            native_call_failure(
                json!("1"),
                "rename",
                true,
                &NativeConnectionError::InvalidInput,
                None
            )["error"]["code"],
            -32602
        );

        // Assert: lost transport before the write is unavailability; after it, unknown.
        assert_eq!(
            native_call_failure(
                json!("1"),
                "rename",
                false,
                &NativeConnectionError::Unavailable,
                None
            )["error"]["data"]["kind"],
            "unavailable"
        );
        for error in [
            NativeConnectionError::Unavailable,
            NativeConnectionError::OutcomeUnknown,
        ] {
            assert_eq!(
                native_call_failure(json!("1"), "rename", true, &error, None)["error"]["data"]["kind"],
                "outcomeUnknown",
                "a dispatched rename must never be reported as refused"
            );
        }
    }

    #[test]
    fn an_echoed_name_that_differs_reports_both_names() {
        // Arrange & act.
        let mismatch = rename_echo_mismatch(json!("1"), "Review", "Old name");

        // Assert.
        assert_eq!(mismatch["error"]["data"]["kind"], "nameMismatch");
        assert_eq!(mismatch["error"]["data"]["requested"], "Review");
        assert_eq!(mismatch["error"]["data"]["effective"], "Old name");
        assert_eq!(mismatch["error"]["data"]["stage"], "rename");
    }
}

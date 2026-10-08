//! Codex native session Control dispatch: decodes each request and calls the typed session
//! operations. No provider policy or native process ownership.
use crate::ServiceIdentity;
use crate::collaboration_application::{
    NativeSessionFailure, NativeSessionFailureKind, NativeSessionStage, SessionOperations,
};
use collaboration_protocol::EndpointRef;
use serde_json::{Value, json};
#[cfg(test)]
use {
    crate::collaboration_application::{classify_native_call_failure, valid_session_rename_name},
    codex_native_integration::NativeConnectionError,
};

#[derive(Clone)]
pub struct NativeControlBackend {
    pub endpoint: EndpointRef,
    pub gate: crate::NativeGenerationGate,
    pub codex_home: std::path::PathBuf,
}

pub(crate) async fn dispatch_native(
    id: Value,
    method: &str,
    params: Value,
    identity: &ServiceIdentity,
) -> Value {
    let sessions = SessionOperations::new(identity);
    let budget = crate::control_connection::control_result_budget(&id);
    macro_rules! decode {
        ($request:ty, $invalid_message:expr) => {
            match serde_json::from_value::<$request>(params) {
                Ok(request) => request,
                Err(_) => return invalid(id, $invalid_message),
            }
        };
    }
    let result = match method {
        "codex/sessionList" => {
            let request = decode!(
                collaboration_protocol::NativeSessionListParams,
                INVALID_INVENTORY_MESSAGE
            );
            return match sessions.codex_session_list(request, budget).await {
                Ok(page) => {
                    let page = published_session_page(&page);
                    if budget.admits(&page) {
                        json!({"jsonrpc":"2.0","id":id,"result":page})
                    } else {
                        let failure = NativeSessionFailure::refused(
                            NativeSessionFailureKind::ResponseTooLarge,
                            NativeSessionStage::Discovery,
                        );
                        crate::control_connection::rejection_response(id, &failure)
                    }
                }
                Err(failure) => crate::control_connection::rejection_response(id, &failure),
            };
        }
        "codex/sessionInspect" => {
            let request = decode!(collaboration_protocol::NativeInspectParams, INVALID_MESSAGE);
            sessions
                .codex_session_inspect(request, budget)
                .await
                .map(|result| json!(result))
        }
        "codex/turnInterrupt" => {
            let request = decode!(
                collaboration_protocol::NativeInterruptParams,
                INVALID_MESSAGE
            );
            sessions
                .codex_turn_interrupt(request, budget)
                .await
                .map(|result| json!(result))
        }
        "codex/sessionRename" => {
            let request = decode!(collaboration_protocol::NativeRenameParams, INVALID_MESSAGE);
            sessions
                .codex_session_rename(request)
                .await
                .map(|result| json!(result))
        }
        _ => return invalid(id, INVALID_MESSAGE),
    };
    match result {
        Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
        Err(failure) => crate::control_connection::rejection_response(id, &failure),
    }
}

const INVALID_MESSAGE: &str = crate::collaboration_application::INVALID_NATIVE_PARAMETERS;
/// The fields Control has always published for each listed session, in their written order.
const PUBLISHED_SESSION_FIELDS: [&str; 10] = [
    "target",
    "name",
    "title",
    "source",
    "gitBranch",
    "workingDirectory",
    "observation",
    "model",
    "reasoningEffort",
    "idleSeconds",
];
const INVALID_INVENTORY_MESSAGE: &str =
    crate::collaboration_application::INVALID_INVENTORY_PARAMETERS;

/// Control's published session page: every session carries every field, with an absent model
/// or reasoning effort as `null`. Its size is checked again, since the nulls add bytes.
fn published_session_page(page: &collaboration_protocol::NativeSessionListResult) -> Value {
    let mut published = json!(page);
    if let Some(sessions) = published.get_mut("sessions").and_then(Value::as_array_mut) {
        for session in sessions {
            let fields = PUBLISHED_SESSION_FIELDS
                .iter()
                .map(|field| {
                    (
                        (*field).to_owned(),
                        session.get(*field).cloned().unwrap_or(Value::Null),
                    )
                })
                .collect::<serde_json::Map<_, _>>();
            *session = Value::Object(fields);
        }
    }
    published
}

fn invalid(id: Value, message: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32602,"message":message}})
}

#[cfg(test)]
fn native_call_failure(
    id: Value,
    stage: &'static str,
    mutation: bool,
    error: &NativeConnectionError,
    native: Option<&Value>,
) -> Value {
    let stage = match stage {
        "inspect" => NativeSessionStage::Inspect,
        "interrupt" => NativeSessionStage::Interrupt,
        _ => NativeSessionStage::Rename,
    };
    let failure = classify_native_call_failure(stage, mutation, error, native);
    crate::control_connection::rejection_response(id, &failure)
}

#[cfg(test)]
fn rename_echo_mismatch(id: Value, requested: &str, effective: &str) -> Value {
    let failure = NativeSessionFailure::NameMismatch {
        requested: requested.to_owned(),
        effective: effective.to_owned(),
    };
    crate::control_connection::rejection_response(id, &failure)
}

#[cfg(test)]
fn rename_method_unsupported(id: Value) -> Value {
    let failure = crate::collaboration_application::rename_method_unsupported();
    crate::control_connection::rejection_response(id, &failure)
}

#[cfg(test)]
mod native_failure_tests {
    use super::{
        NativeConnectionError, native_call_failure, rename_echo_mismatch, valid_session_rename_name,
    };
    use codex_native_integration::NativeOperation;
    use serde_json::json;

    #[test]
    fn rename_names_cannot_inject_header_lines_or_identities() {
        for invalid_name in [
            "Name\n forged",
            "Name\u{0001} forged",
            "Name ← sender",
            "Name → recipient",
        ] {
            assert!(
                !valid_session_rename_name(invalid_name),
                "rename should reject {invalid_name:?}"
            );
        }
        assert!(valid_session_rename_name("Renamed Sidekick"));
    }

    #[test]
    fn every_native_error_class_keeps_its_own_projection_on_the_rename_path() {
        // Arrange: one refusal with native evidence, plus the transport classes.
        let rejection = json!({"message":"thread has an active turn"});

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
        assert_eq!(
            refused["error"]["data"]["message"],
            "thread has an active turn"
        );

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

    #[test]
    fn missing_thread_rename_schema_names_the_app_server_method_and_repair() {
        let mut definitions = serde_json::Map::new();
        for operation in [
            "ThreadRead",
            "ThreadResume",
            "ThreadStart",
            "ThreadLoadedList",
            "TurnStart",
            "TurnSteer",
            "TurnInterrupt",
        ] {
            definitions.insert(format!("{operation}Params"), json!({"type":"object"}));
            definitions.insert(format!("{operation}Response"), json!({"type":"object"}));
        }
        let bundle = codex_native_integration::NativeSchemaBundle::from_documents(
            std::collections::BTreeMap::from([(
                "codex_app_server_protocol.schemas.json".to_owned(),
                serde_json::to_vec(&json!({"definitions":{"v2":definitions}}))
                    .unwrap_or_else(|error| panic!("schema: {error}")),
            )]),
        )
        .unwrap_or_else(|error| panic!("bundle: {error}"));
        let schemas = codex_native_integration::NativePayloadSchemas::from_bundle(&bundle)
            .unwrap_or_else(|error| panic!("native operations: {error}"));
        assert!(!schemas.supports_operation(NativeOperation::SetThreadName));
        let response = super::rename_method_unsupported(json!("1"));

        assert_eq!(
            response["error"]["data"]["message"],
            "Codex app-server method `thread/name/set` is missing from its cached schema; update Codex so its app-server schema defines ThreadSetNameParams and ThreadSetNameResponse, then restart the Router Host"
        );
    }

    #[test]
    fn an_unknown_post_dispatch_result_includes_the_native_transport_class() {
        let response = native_call_failure(
            json!("1"),
            "interrupt",
            true,
            &NativeConnectionError::OutcomeUnknown,
            None,
        );

        assert_eq!(response["error"]["data"]["kind"], "outcomeUnknown");
        assert!(
            response["error"]["data"]["message"]
                .as_str()
                .is_some_and(|message| message.contains("native request outcome is unknown"))
        );
    }

    #[test]
    fn a_failed_inspection_does_not_claim_an_unknown_mutation_effect() {
        let response = native_call_failure(
            json!("1"),
            "inspect",
            false,
            &NativeConnectionError::OutcomeUnknown,
            None,
        );

        assert_eq!(response["error"]["data"]["kind"], "unavailable");
        assert!(
            response["error"]["data"]["message"]
                .as_str()
                .is_some_and(|message| message.contains("native request outcome is unknown"))
        );
    }
}

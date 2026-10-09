//! The Codex native control backend the session operations call: the endpoint it serves and
//! the generation gate that admits each call. No provider policy or native process ownership.
use collaboration_protocol::EndpointRef;
#[cfg(test)]
use {
    crate::collaboration_application::{
        NativeSessionFailure, NativeSessionStage, classify_native_call_failure,
        valid_session_rename_name,
    },
    codex_native_integration::NativeConnectionError,
    serde_json::{Value, json},
};

#[derive(Clone)]
pub struct NativeControlBackend {
    pub endpoint: EndpointRef,
    pub gate: crate::NativeGenerationGate,
    pub codex_home: std::path::PathBuf,
}

/// A native failure as callers receive it, in the envelope these tests read.
#[cfg(test)]
fn published(failure: &impl crate::collaboration_application::CollaborationRejection) -> Value {
    json!({"error": failure.published_rejection()})
}

#[cfg(test)]
fn native_call_failure(
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
    published(&classify_native_call_failure(
        stage, mutation, error, native,
    ))
}

#[cfg(test)]
fn rename_echo_mismatch(requested: &str, effective: &str) -> Value {
    published(&NativeSessionFailure::NameMismatch {
        requested: requested.to_owned(),
        effective: effective.to_owned(),
    })
}

#[cfg(test)]
fn rename_method_unsupported() -> Value {
    published(&crate::collaboration_application::rename_method_unsupported())
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
            "rename",
            true,
            &NativeConnectionError::Rejected { code: -32099 },
            None,
        );
        assert_eq!(unknown["error"]["data"]["reason"], "unknown");
        assert_eq!(unknown["error"]["data"]["nativeCode"], -32099);

        // Assert: invalid input is a request defect, not a native refusal.
        assert_eq!(
            native_call_failure("rename", true, &NativeConnectionError::InvalidInput, None)["error"]
                ["code"],
            -32602
        );

        // Assert: lost transport before the write is unavailability; after it, unknown.
        assert_eq!(
            native_call_failure("rename", false, &NativeConnectionError::Unavailable, None)["error"]
                ["data"]["kind"],
            "unavailable"
        );
        for error in [
            NativeConnectionError::Unavailable,
            NativeConnectionError::OutcomeUnknown,
        ] {
            assert_eq!(
                native_call_failure("rename", true, &error, None)["error"]["data"]["kind"],
                "outcomeUnknown",
                "a dispatched rename must never be reported as refused"
            );
        }
    }

    #[test]
    fn an_echoed_name_that_differs_reports_both_names() {
        // Arrange & act.
        let mismatch = rename_echo_mismatch("Review", "Old name");

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
        let response = super::rename_method_unsupported();

        assert_eq!(
            response["error"]["data"]["message"],
            "Codex app-server method `thread/name/set` is missing from its cached schema; update Codex so its app-server schema defines ThreadSetNameParams and ThreadSetNameResponse, then restart the Router Host"
        );
    }

    #[test]
    fn an_unknown_post_dispatch_result_includes_the_native_transport_class() {
        let response = native_call_failure(
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

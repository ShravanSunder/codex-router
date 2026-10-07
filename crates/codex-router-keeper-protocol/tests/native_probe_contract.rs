use std::path::PathBuf;
use std::time::Duration;

use codex_native_integration::{
    AppServerObservation, AppServerObservationValidationError, AppServerProbeAction,
    CodexProtocolError, NativeObservationStage, RemoteControlObservation,
};
use codex_router_descriptor_boundary::BoundaryError;
use codex_router_keeper_protocol::{
    ComponentFingerprint, ComponentKind, GenerationAliasPath, JsonMessage, MAX_FRAME_BYTES,
    NativeProbeFailure, NativeProbeJob, NativeProbeJobConversionError, NativeProbeJobWire,
    NativeProbeResult, NativeProbeResultWire, NativeProbeStage, ReceiverHelloWire,
    RemoteControlObservationWire,
};

const MAX_NATIVE_EVIDENCE_BYTES: usize = 64 * 1024;

macro_rules! test_success {
    ($result:expr, $message:literal) => {{
        let result = $result;
        assert!(result.is_ok(), $message);
        match result {
            Ok(value) => value,
            Err(_) => return,
        }
    }};
}

macro_rules! test_error {
    ($result:expr, $message:literal) => {{
        let result = $result;
        assert!(result.is_err(), $message);
        match result {
            Err(error) => error,
            Ok(_) => return,
        }
    }};
}

fn generation_alias() -> Result<GenerationAliasPath, Box<dyn std::error::Error>> {
    Ok(GenerationAliasPath::try_from(PathBuf::from(
        "/run/codex/gen-0123abcd-7.sock",
    ))?)
}

#[test]
fn receiver_hello_uses_the_declared_literal_wire_shape() {
    let wire = ReceiverHelloWire::Hello {
        parent_role: ComponentKind::Keeper,
        fingerprint: test_success!(ComponentFingerprint::from_bytes(&[0; 32]), "fingerprint"),
    };
    let expected = br#"{"type":"hello","parent_role":"keeper","fingerprint":"0000000000000000000000000000000000000000000000000000000000000000"}"#;
    assert_eq!(
        test_success!(serde_json::to_vec(&wire), "hello serializes"),
        expected
    );
    assert_eq!(
        test_success!(
            serde_json::from_slice::<ReceiverHelloWire>(expected),
            "literal hello decodes"
        ),
        wire
    );
    assert!(serde_json::from_str::<ReceiverHelloWire>(
        r#"{"type":"other","parent_role":"keeper","fingerprint":"0000000000000000000000000000000000000000000000000000000000000000"}"#
    )
    .is_err());
}

#[test]
fn all_three_native_probe_jobs_have_literal_wire_and_typed_projections() {
    let alias = test_success!(generation_alias(), "canonical generation alias");
    let cases = [
        (
            NativeProbeJobWire::Observe {
                alias: alias.clone(),
                native_wait_ms: 0,
                remote_wait_ms: 250,
            },
            AppServerProbeAction::Observe,
            Duration::ZERO,
            Duration::from_millis(250),
            r#"{"type":"observe","alias":"/run/codex/gen-0123abcd-7.sock","native_wait_ms":0,"remote_wait_ms":250}"#,
        ),
        (
            NativeProbeJobWire::WaitForReady {
                alias: alias.clone(),
                native_wait_ms: 5000,
                remote_wait_ms: 0,
            },
            AppServerProbeAction::WaitForReady,
            Duration::from_secs(5),
            Duration::ZERO,
            r#"{"type":"waitForReady","alias":"/run/codex/gen-0123abcd-7.sock","native_wait_ms":5000,"remote_wait_ms":0}"#,
        ),
        (
            NativeProbeJobWire::EnableAndObserve {
                alias: alias.clone(),
                native_wait_ms: 2000,
                remote_wait_ms: 7000,
            },
            AppServerProbeAction::EnableAndObserve,
            Duration::from_secs(2),
            Duration::from_secs(7),
            r#"{"type":"enableAndObserve","alias":"/run/codex/gen-0123abcd-7.sock","native_wait_ms":2000,"remote_wait_ms":7000}"#,
        ),
    ];

    for (wire, expected_action, native_wait, remote_wait, literal) in cases {
        assert_eq!(
            test_success!(serde_json::to_string(&wire), "job serializes"),
            literal
        );
        assert_eq!(
            test_success!(
                serde_json::from_str::<NativeProbeJobWire>(literal),
                "literal job decodes"
            ),
            wire
        );

        let job = NativeProbeJob::from(wire.clone());
        assert_eq!(job.action(), expected_action);
        assert_eq!(job.alias(), &alias);
        assert_eq!(job.native_readiness_wait(), native_wait);
        assert_eq!(job.remote_control_wait(), remote_wait);
        assert_eq!(
            test_success!(
                NativeProbeJobWire::try_from(job),
                "whole milliseconds encode"
            ),
            wire
        );
    }

    assert!(serde_json::from_str::<NativeProbeJobWire>(
        r#"{"type":"observe","alias":"relative/gen-0123abcd-7.sock","native_wait_ms":0,"remote_wait_ms":0}"#
    )
    .is_err());
    assert!(serde_json::from_str::<NativeProbeJobWire>(
        r#"{"type":"observe","alias":"/run/codex/gen-0123abcd-07.sock","native_wait_ms":0,"remote_wait_ms":0}"#
    )
    .is_err());
    assert!(serde_json::from_str::<NativeProbeJobWire>(
        r#"{"type":"unknown","alias":"/run/codex/gen-0123abcd-7.sock","native_wait_ms":0,"remote_wait_ms":0}"#
    )
    .is_err());
}

#[test]
fn duration_projection_is_exact_and_rejects_fractional_or_overflowed_milliseconds() {
    let alias = test_success!(generation_alias(), "canonical generation alias");
    let maximum_wire = NativeProbeJobWire::Observe {
        alias: alias.clone(),
        native_wait_ms: u64::MAX,
        remote_wait_ms: u64::MAX,
    };
    let maximum_job = NativeProbeJob::from(maximum_wire.clone());
    assert_eq!(
        maximum_job.native_readiness_wait(),
        Duration::from_millis(u64::MAX)
    );
    assert_eq!(
        test_success!(
            NativeProbeJobWire::try_from(maximum_job),
            "maximum milliseconds round-trip"
        ),
        maximum_wire
    );

    let fractional_native = NativeProbeJob::new(
        AppServerProbeAction::Observe,
        alias.clone(),
        Duration::from_nanos(1),
        Duration::ZERO,
    );
    assert_eq!(
        NativeProbeJobWire::try_from(fractional_native).err(),
        Some(NativeProbeJobConversionError::NativeReadinessWaitNotRepresentable)
    );

    let fractional_remote = NativeProbeJob::new(
        AppServerProbeAction::Observe,
        alias.clone(),
        Duration::ZERO,
        Duration::from_nanos(1),
    );
    assert_eq!(
        NativeProbeJobWire::try_from(fractional_remote).err(),
        Some(NativeProbeJobConversionError::RemoteControlWaitNotRepresentable)
    );

    let overflow_native = NativeProbeJob::new(
        AppServerProbeAction::Observe,
        alias.clone(),
        Duration::MAX,
        Duration::ZERO,
    );
    assert_eq!(
        NativeProbeJobWire::try_from(overflow_native).err(),
        Some(NativeProbeJobConversionError::NativeReadinessWaitNotRepresentable)
    );

    let overflow_remote = NativeProbeJob::new(
        AppServerProbeAction::Observe,
        alias,
        Duration::ZERO,
        Duration::MAX,
    );
    assert_eq!(
        NativeProbeJobWire::try_from(overflow_remote).err(),
        Some(NativeProbeJobConversionError::RemoteControlWaitNotRepresentable)
    );
}

#[test]
fn native_probe_stages_and_failures_round_trip_as_closed_literal_values() {
    let stages = [
        (
            NativeObservationStage::Connect,
            NativeProbeStage::Connect,
            "connect",
        ),
        (
            NativeObservationStage::WebSocketUpgrade,
            NativeProbeStage::WebSocketUpgrade,
            "webSocketUpgrade",
        ),
        (
            NativeObservationStage::NativeReadiness,
            NativeProbeStage::NativeReadiness,
            "nativeReadiness",
        ),
        (
            NativeObservationStage::Initialize,
            NativeProbeStage::Initialize,
            "initialize",
        ),
        (
            NativeObservationStage::RemoteControlStatus,
            NativeProbeStage::RemoteControlStatus,
            "remoteControlStatus",
        ),
        (
            NativeObservationStage::RemoteControlStatusChange,
            NativeProbeStage::RemoteControlStatusChange,
            "remoteControlStatusChange",
        ),
        (
            NativeObservationStage::RemoteControlEnable,
            NativeProbeStage::RemoteControlEnable,
            "remoteControlEnable",
        ),
    ];
    for (native_stage, wire_stage, literal) in stages {
        assert_eq!(NativeProbeStage::from(native_stage), wire_stage);
        assert_eq!(NativeObservationStage::from(wire_stage), native_stage);
        assert_eq!(
            test_success!(serde_json::to_string(&wire_stage), "stage serializes"),
            format!("\"{literal}\"")
        );
        assert_eq!(
            test_success!(
                serde_json::from_str::<NativeProbeStage>(&format!("\"{literal}\"")),
                "literal stage decodes"
            ),
            wire_stage
        );
    }
    assert!(serde_json::from_str::<NativeProbeStage>("\"callerText\"").is_err());

    let failures = [
        (NativeProbeFailure::Connect, r#"{"type":"connect"}"#),
        (NativeProbeFailure::WebSocket, r#"{"type":"webSocket"}"#),
        (NativeProbeFailure::Json, r#"{"type":"json"}"#),
        (
            NativeProbeFailure::Timeout {
                stage: NativeProbeStage::Connect,
            },
            r#"{"type":"timeout","stage":"connect"}"#,
        ),
        (
            NativeProbeFailure::Closed {
                stage: NativeProbeStage::WebSocketUpgrade,
            },
            r#"{"type":"closed","stage":"webSocketUpgrade"}"#,
        ),
        (
            NativeProbeFailure::InvalidResponse {
                stage: NativeProbeStage::RemoteControlStatusChange,
            },
            r#"{"type":"invalidResponse","stage":"remoteControlStatusChange"}"#,
        ),
        (
            NativeProbeFailure::InvalidUserAgent,
            r#"{"type":"invalidUserAgent"}"#,
        ),
    ];
    for (failure, literal) in failures {
        assert_eq!(
            test_success!(serde_json::to_string(&failure), "failure serializes"),
            literal
        );
        assert_eq!(
            test_success!(
                serde_json::from_str::<NativeProbeFailure>(literal),
                "literal failure decodes"
            ),
            failure
        );
    }
    assert!(serde_json::from_str::<NativeProbeFailure>(r#"{"type":"callerText"}"#).is_err());
    assert!(
        serde_json::from_str::<NativeProbeFailure>(r#"{"type":"timeout","stage":"callerText"}"#)
            .is_err()
    );
}

#[test]
fn native_protocol_errors_project_to_bounded_failures_without_source_text() {
    let websocket_cause = std::io::Error::other("private websocket cause");
    let json_cause = test_error!(
        serde_json::from_str::<serde_json::Value>("private json cause"),
        "invalid JSON fixture"
    );
    let cases = [
        (
            CodexProtocolError::Connect(std::io::Error::other("private connect cause")),
            NativeProbeFailure::Connect,
        ),
        (
            CodexProtocolError::WebSocket(websocket_cause.into()),
            NativeProbeFailure::WebSocket,
        ),
        (
            CodexProtocolError::Json(json_cause),
            NativeProbeFailure::Json,
        ),
        (
            CodexProtocolError::Timeout {
                stage: NativeObservationStage::Initialize,
            },
            NativeProbeFailure::Timeout {
                stage: NativeProbeStage::Initialize,
            },
        ),
        (
            CodexProtocolError::Closed {
                stage: NativeObservationStage::RemoteControlEnable,
            },
            NativeProbeFailure::Closed {
                stage: NativeProbeStage::RemoteControlEnable,
            },
        ),
        (
            CodexProtocolError::InvalidResponse {
                stage: NativeObservationStage::RemoteControlStatus,
            },
            NativeProbeFailure::InvalidResponse {
                stage: NativeProbeStage::RemoteControlStatus,
            },
        ),
        (
            CodexProtocolError::InvalidUserAgent,
            NativeProbeFailure::InvalidUserAgent,
        ),
    ];

    for (native_error, expected_failure) in cases {
        let failure = NativeProbeFailure::from(native_error);
        assert_eq!(failure, expected_failure);
        let wire = test_success!(serde_json::to_string(&failure), "failure serializes");
        assert!(!wire.contains("private"));
    }
}

#[test]
fn remote_control_and_observed_result_preserve_raw_recorded_parts() {
    let cases = [
        (
            RemoteControlObservation::Connected {
                server_name: String::new(),
                environment_id: None,
            },
            r#"{"state":"connected","server_name":"","environment_id":null}"#,
            r#"{"type":"observed","running_version":"not-semver","remote_control":{"state":"connected","server_name":"","environment_id":null}}"#,
        ),
        (
            RemoteControlObservation::Connecting {
                server_name: "line\n\"name\"".to_owned(),
                environment_id: Some(String::new()),
            },
            r#"{"state":"connecting","server_name":"line\n\"name\"","environment_id":""}"#,
            r#"{"type":"observed","running_version":"not-semver","remote_control":{"state":"connecting","server_name":"line\n\"name\"","environment_id":""}}"#,
        ),
        (
            RemoteControlObservation::Errored {
                server_name: "server".to_owned(),
                environment_id: Some("env-123".to_owned()),
            },
            r#"{"state":"errored","server_name":"server","environment_id":"env-123"}"#,
            r#"{"type":"observed","running_version":"not-semver","remote_control":{"state":"errored","server_name":"server","environment_id":"env-123"}}"#,
        ),
        (
            RemoteControlObservation::Disabled {
                server_name: "snowman ☃".to_owned(),
                environment_id: None,
            },
            r#"{"state":"disabled","server_name":"snowman ☃","environment_id":null}"#,
            r#"{"type":"observed","running_version":"not-semver","remote_control":{"state":"disabled","server_name":"snowman ☃","environment_id":null}}"#,
        ),
    ];

    for (native_remote_control, literal_remote_control, literal_result) in cases {
        let wire_remote_control = RemoteControlObservationWire::from(native_remote_control.clone());
        assert_eq!(
            test_success!(
                serde_json::to_string(&wire_remote_control),
                "remote state serializes"
            ),
            literal_remote_control
        );
        assert_eq!(
            test_success!(
                serde_json::from_str::<RemoteControlObservationWire>(literal_remote_control),
                "literal remote state decodes"
            ),
            wire_remote_control
        );
        assert_eq!(
            RemoteControlObservation::from(wire_remote_control),
            native_remote_control
        );

        let observation = test_success!(
            AppServerObservation::from_recorded_parts(
                "not-semver".to_owned(),
                native_remote_control.clone(),
            ),
            "valid recorded native observation"
        );
        let typed_result = NativeProbeResult::Observed(observation.clone());
        let wire_result: NativeProbeResultWire = typed_result.into();
        let result_json = test_success!(
            serde_json::to_string(&wire_result),
            "observed result serializes"
        );
        assert_eq!(result_json, literal_result);
        let decoded_wire = test_success!(
            serde_json::from_str::<NativeProbeResultWire>(literal_result),
            "literal observed result decodes"
        );
        assert_eq!(
            NativeProbeResult::try_from(decoded_wire),
            Ok(NativeProbeResult::Observed(observation))
        );
    }
}

#[test]
fn recorded_result_preserves_the_native_field_boundary_without_a_new_aggregate_cap() {
    let maximum_field = "x".repeat(MAX_NATIVE_EVIDENCE_BYTES);
    let observation = test_success!(
        AppServerObservation::from_recorded_parts(
            maximum_field.clone(),
            RemoteControlObservation::Connected {
                server_name: maximum_field.clone(),
                environment_id: Some(maximum_field),
            },
        ),
        "maximum bounded native fields are valid"
    );
    let wire: NativeProbeResultWire = NativeProbeResult::Observed(observation).into();
    let framed = test_success!(
        JsonMessage::encode(&wire),
        "bounded result fits current frame"
    );
    assert!(framed.bytes().len() < MAX_FRAME_BYTES);

    for invalid_version in ["", " ", "not-semver other"] {
        let wire = NativeProbeResultWire::Observed {
            running_version: invalid_version.to_owned(),
            remote_control: RemoteControlObservationWire::Disabled {
                server_name: String::new(),
                environment_id: None,
            },
        };
        assert_eq!(
            NativeProbeResult::try_from(wire).err(),
            Some(AppServerObservationValidationError::InvalidRunningVersion)
        );
    }

    let oversized_value = "x".repeat(MAX_NATIVE_EVIDENCE_BYTES + 1);
    for (running_version, remote_control, expected_field) in [
        (
            oversized_value.clone(),
            RemoteControlObservationWire::Disabled {
                server_name: String::new(),
                environment_id: None,
            },
            codex_native_integration::AppServerObservationField::RunningVersion,
        ),
        (
            "one-token".to_owned(),
            RemoteControlObservationWire::Errored {
                server_name: oversized_value.clone(),
                environment_id: None,
            },
            codex_native_integration::AppServerObservationField::RemoteControlServerName,
        ),
        (
            "one-token".to_owned(),
            RemoteControlObservationWire::Connected {
                server_name: String::new(),
                environment_id: Some(oversized_value),
            },
            codex_native_integration::AppServerObservationField::RemoteControlEnvironmentId,
        ),
    ] {
        assert_eq!(
            NativeProbeResult::try_from(NativeProbeResultWire::Observed {
                running_version,
                remote_control,
            })
            .err(),
            Some(AppServerObservationValidationError::FieldTooLarge {
                field: expected_field,
            })
        );
    }
}

#[test]
fn json_message_keeps_literal_escaping_and_full_encoded_frame_bound() {
    let escaped = test_success!(JsonMessage::encode(&"line\n\"quote\""), "small string fits");
    assert_eq!(escaped.bytes(), b"\"line\\n\\\"quote\\\"\"");

    let mut at_limit = Vec::with_capacity(MAX_FRAME_BYTES);
    at_limit.push(b'\"');
    at_limit.resize(MAX_FRAME_BYTES - 1, b'a');
    at_limit.push(b'\"');
    assert_eq!(at_limit.len(), MAX_FRAME_BYTES);
    assert!(JsonMessage::from_bytes(at_limit).is_ok());

    let mut over_limit = Vec::with_capacity(MAX_FRAME_BYTES + 1);
    over_limit.push(b'\"');
    over_limit.resize(MAX_FRAME_BYTES, b'a');
    over_limit.push(b'\"');
    assert_eq!(over_limit.len(), MAX_FRAME_BYTES + 1);
    assert!(matches!(
        JsonMessage::from_bytes(over_limit),
        Err(BoundaryError::TooLarge)
    ));
}

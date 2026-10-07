use codex_native_integration::AppServerObservation;
use codex_native_integration::AppServerObservationField;
use codex_native_integration::AppServerObservationValidationError;
use codex_native_integration::CodexProtocolError;
use codex_native_integration::NativeObservationStage;
use codex_native_integration::RemoteControlObservation;
use codex_native_integration::RemoteControlServerName;

const MAX_NATIVE_EVIDENCE_BYTES: usize = 64 * 1024;

#[test]
fn native_observation_stages_have_closed_wire_values_and_legacy_labels() {
    let stages = [
        (NativeObservationStage::Connect, "connect", "connect"),
        (
            NativeObservationStage::WebSocketUpgrade,
            "webSocketUpgrade",
            "websocket upgrade",
        ),
        (
            NativeObservationStage::NativeReadiness,
            "nativeReadiness",
            "native readiness",
        ),
        (
            NativeObservationStage::Initialize,
            "initialize",
            "initialize",
        ),
        (
            NativeObservationStage::RemoteControlStatus,
            "remoteControlStatus",
            "Remote Control status",
        ),
        (
            NativeObservationStage::RemoteControlStatusChange,
            "remoteControlStatusChange",
            "Remote Control status change",
        ),
        (
            NativeObservationStage::RemoteControlEnable,
            "remoteControlEnable",
            "Remote Control enable",
        ),
    ];

    for (stage, wire_value, legacy_label) in stages {
        assert_eq!(
            serde_json::to_string(&stage).expect("stage serializes"),
            format!("\"{wire_value}\"")
        );
        assert_eq!(
            serde_json::from_str::<NativeObservationStage>(&format!("\"{wire_value}\""))
                .expect("known stage deserializes"),
            stage
        );
        assert_eq!(stage.to_string(), legacy_label);
    }

    assert!(serde_json::from_str::<NativeObservationStage>("\"callerText\"").is_err());
    assert_eq!(
        CodexProtocolError::Timeout {
            stage: NativeObservationStage::RemoteControlStatus,
        }
        .to_string(),
        "native app-server Remote Control status timed out",
    );
    assert_eq!(
        CodexProtocolError::Closed {
            stage: NativeObservationStage::RemoteControlStatusChange,
        }
        .to_string(),
        "native app-server closed during Remote Control status change",
    );
    assert_eq!(
        CodexProtocolError::InvalidResponse {
            stage: NativeObservationStage::Initialize,
        }
        .to_string(),
        "native app-server returned an invalid initialize response",
    );
}

#[test]
fn recorded_observation_preserves_every_raw_remote_control_variant() {
    let remote_control_values = [
        RemoteControlObservation::Connected {
            server_name: String::new(),
            environment_id: None,
        },
        RemoteControlObservation::Connecting {
            server_name: "  \n".to_owned(),
            environment_id: Some(String::new()),
        },
        RemoteControlObservation::Errored {
            server_name: "Remote\n\"Name\"".to_owned(),
            environment_id: Some("env_123".to_owned()),
        },
        RemoteControlObservation::Disabled {
            server_name: "owner/☃".to_owned(),
            environment_id: None,
        },
    ];

    for remote_control in remote_control_values {
        let observation = AppServerObservation::from_recorded_parts(
            "not-semver".to_owned(),
            remote_control.clone(),
        )
        .expect("a single raw version token and bounded claims are valid");

        assert_eq!(observation.running_version(), "not-semver");
        assert_eq!(observation.remote_control(), &remote_control);
    }
}

#[test]
fn recorded_observation_requires_one_nonempty_version_token() {
    for invalid_version in ["", " \t", " 1.2.3", "1.2.3 ", "1.2.3 other"] {
        assert_eq!(
            AppServerObservation::from_recorded_parts(
                invalid_version.to_owned(),
                RemoteControlObservation::Disabled {
                    server_name: String::new(),
                    environment_id: None,
                },
            )
            .expect_err("invalid version claims must be rejected"),
            AppServerObservationValidationError::InvalidRunningVersion,
        );
    }
}

#[test]
fn recorded_observation_accepts_the_evidence_limit_and_rejects_each_oversized_field() {
    let maximum_value = "x".repeat(MAX_NATIVE_EVIDENCE_BYTES);
    let maximum_remote_control = RemoteControlObservation::Connected {
        server_name: maximum_value.clone(),
        environment_id: Some(maximum_value.clone()),
    };
    let maximum_observation =
        AppServerObservation::from_recorded_parts(maximum_value, maximum_remote_control.clone())
            .expect("each value at the native evidence limit is valid");
    assert_eq!(
        maximum_observation.running_version().len(),
        MAX_NATIVE_EVIDENCE_BYTES
    );
    assert_eq!(
        maximum_observation.remote_control(),
        &maximum_remote_control,
    );

    let oversized_value = "x".repeat(MAX_NATIVE_EVIDENCE_BYTES + 1);
    let valid_remote_control = RemoteControlObservation::Connected {
        server_name: "machine".to_owned(),
        environment_id: None,
    };

    assert_eq!(
        AppServerObservation::from_recorded_parts(oversized_value.clone(), valid_remote_control,)
            .expect_err("oversized version claims must be rejected"),
        AppServerObservationValidationError::FieldTooLarge {
            field: AppServerObservationField::RunningVersion,
        },
    );

    assert_eq!(
        AppServerObservation::from_recorded_parts(
            "1.2.3".to_owned(),
            RemoteControlObservation::Errored {
                server_name: oversized_value.clone(),
                environment_id: None,
            },
        )
        .expect_err("oversized server-name claims must be rejected"),
        AppServerObservationValidationError::FieldTooLarge {
            field: AppServerObservationField::RemoteControlServerName,
        },
    );

    assert_eq!(
        AppServerObservation::from_recorded_parts(
            "1.2.3".to_owned(),
            RemoteControlObservation::Disabled {
                server_name: String::new(),
                environment_id: Some(oversized_value),
            },
        )
        .expect_err("oversized environment claims must be rejected"),
        AppServerObservationValidationError::FieldTooLarge {
            field: AppServerObservationField::RemoteControlEnvironmentId,
        },
    );
}

#[test]
fn validated_remote_control_server_name_preserves_host_rules_and_serde_validation() {
    let unusual_name = "Remote\n\"Name\"".to_owned();
    let validated_name = RemoteControlServerName::try_from(unusual_name.clone())
        .expect("newline and quote display characters remain valid");
    assert_eq!(validated_name.as_str(), unusual_name);
    assert_eq!(
        serde_json::to_string(&validated_name).expect("validated name serializes"),
        serde_json::to_string(&unusual_name).expect("source string serializes"),
    );
    assert_eq!(
        serde_json::from_str::<RemoteControlServerName>(
            &serde_json::to_string(&unusual_name).expect("source string serializes"),
        )
        .expect("valid name deserializes through validation"),
        validated_name,
    );

    for invalid_name in [
        String::new(),
        " \t".to_owned(),
        "name\0with-nul".to_owned(),
        "x".repeat(4097),
    ] {
        assert_eq!(
            RemoteControlServerName::try_from(invalid_name.clone()).expect_err("invalid name"),
            "invalid Remote Control server name",
        );
        assert!(
            serde_json::from_str::<RemoteControlServerName>(
                &serde_json::to_string(&invalid_name).expect("source string serializes"),
            )
            .is_err(),
            "Serde must validate the source string"
        );
    }

    let maximum_name = RemoteControlServerName::try_from("x".repeat(4096))
        .expect("the existing 4096-byte boundary remains valid");
    assert_eq!(maximum_name.as_str().len(), 4096);
}

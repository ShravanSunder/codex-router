//! Argument rejection is machine-readable before service discovery or native dispatch.
#[test]
fn json_argument_errors_report_validation_and_no_effects() {
    for group in [
        "wake",
        "schedule",
        "instruction",
        "run",
        "automation",
        "delivery",
        "revision",
        "operation",
    ] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_agent-sessions"))
            .args([group, "invalid-action", "--json"])
            .output()
            .expect("CLI executes");
        assert_eq!(output.status.code(), Some(2), "{group}");
        let value: serde_json::Value =
            serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
                panic!(
                    "{group} must return JSON: {error}; stderr={}",
                    String::from_utf8_lossy(&output.stderr)
                )
            });
        assert_eq!(
            value
                .pointer("/error/kind")
                .and_then(serde_json::Value::as_str),
            Some("invalidField")
        );
        assert_eq!(
            value
                .pointer("/error/stage")
                .and_then(serde_json::Value::as_str),
            Some("validation")
        );
        assert_eq!(
            value
                .pointer("/error/effects/mutation")
                .and_then(serde_json::Value::as_str),
            Some("none")
        );
        assert_eq!(
            value
                .pointer("/error/nextAction")
                .and_then(serde_json::Value::as_str),
            Some("correctRequest")
        );
    }
}

#[test]
fn help_remains_successful_without_connecting_to_a_service() {
    for group in [
        "wake",
        "schedule",
        "instruction",
        "run",
        "automation",
        "delivery",
        "revision",
        "operation",
    ] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_agent-sessions"))
            .args([group, "--help"])
            .output()
            .expect("CLI executes");
        assert!(output.status.success(), "{group}");
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("Usage:"),
            "{group}"
        );
    }
}

#[test]
fn invalid_run_identity_reports_field_and_no_dispatch() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_agent-sessions"))
        .args(["run", "show", "--run-id", "invalid", "--json"])
        .output()
        .expect("CLI executes");
    assert_eq!(output.status.code(), Some(2));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).expect("JSON error");
    assert_eq!(
        value
            .pointer("/error/stage")
            .and_then(serde_json::Value::as_str),
        Some("validation")
    );
    assert_eq!(
        value
            .pointer("/error/field")
            .and_then(serde_json::Value::as_str),
        Some("--run-id")
    );
    assert_eq!(
        value
            .pointer("/error/effects/mutation")
            .and_then(serde_json::Value::as_str),
        Some("none")
    );
    assert_eq!(
        value
            .pointer("/error/nextAction")
            .and_then(serde_json::Value::as_str),
        Some("correctRequest")
    );
}

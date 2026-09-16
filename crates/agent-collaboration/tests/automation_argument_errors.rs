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
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"))
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
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"))
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
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"))
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

#[test]
fn finite_control_groups_keep_json_for_missing_required_arguments() {
    let cases: &[&[&str]] = &[
        &["endpoints", "invalid-action", "--json"],
        &["addresses", "list", "--json"],
        &["journal", "read", "--json"],
        &["sessions", "list", "--json"],
        &["session", "inspect", "--json"],
        &["conversation", "prompt", "--json"],
        &["board", "project", "show", "--json"],
        &["message", "send", "--json"],
    ];

    for arguments in cases {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"))
            .args(*arguments)
            .output()
            .unwrap_or_else(|error| panic!("command {arguments:?}: {error}"));
        assert_eq!(output.status.code(), Some(2), "command {arguments:?}");
        let record: serde_json::Value = serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|error| panic!("JSON for {arguments:?}: {error}"));
        assert_eq!(record["error"]["kind"], "invalidField");
        assert_eq!(record["error"]["stage"], "validation");
        assert_eq!(record["error"]["nextAction"], "correctRequest");
    }
}

#[test]
fn conversation_model_choice_is_validated_before_service_discovery() {
    let cases = [
        (
            vec![
                "conversation",
                "prompt",
                "--endpoint",
                "codex-local",
                "--cwd",
                "/tmp",
                "--new",
                "--model",
                "gpt-5.6-sol",
                "--text",
                "hello",
                "--json",
            ],
            "--effort",
        ),
        (
            vec![
                "conversation",
                "prompt",
                "--endpoint",
                "codex-local",
                "--cwd",
                "/tmp",
                "--new",
                "--effort",
                "medium",
                "--text",
                "hello",
                "--json",
            ],
            "--model",
        ),
        (
            vec![
                "conversation",
                "prompt",
                "--endpoint",
                "codex-local",
                "--cwd",
                "/tmp",
                "--session",
                "01a0a9aa-0393-7a30-aeca-c7c77d679774",
                "--model",
                "gpt-5.6-sol",
                "--effort",
                "medium",
                "--text",
                "hello",
                "--json",
            ],
            "model is fixed for a thread; fork to change it",
        ),
    ];
    for (arguments, expected) in cases {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"))
            .args(arguments)
            .output()
            .expect("CLI executes");
        assert_eq!(output.status.code(), Some(2));
        let record: serde_json::Value = serde_json::from_slice(&output.stdout).expect("JSON error");
        assert!(
            record["error"]["message"]
                .as_str()
                .is_some_and(|message| message.contains(expected)),
            "{record}"
        );
    }
}

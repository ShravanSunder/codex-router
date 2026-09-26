fn session_ref() -> String {
    serde_json::json!({
        "endpoint": {
            "serviceId": "019f0000-0000-7000-8000-000000000010",
            "endpointId": "codex-local"
        },
        "sessionId": "message-feedback-fixture"
    })
    .to_string()
}

#[test]
fn missing_from_is_machine_readable_when_json_is_requested() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"))
        .args([
            "message",
            "send",
            "--to",
            &session_ref(),
            "--text",
            "fixture",
            "--json",
        ])
        .output()
        .unwrap_or_else(|error| panic!("message command: {error}"));

    assert_eq!(output.status.code(), Some(2));
    let record: serde_json::Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("JSON argument error: {error}"));
    assert_eq!(record["error"]["kind"], "invalidField");
    assert_eq!(record["error"]["stage"], "validation");
    assert_eq!(record["error"]["nextAction"], "correctRequest");
    assert!(
        record["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("agent-collaboration whoami --json"))
    );
}

#[test]
fn lifecycle_address_gets_fixed_session_ref_guidance_without_payload_echo() {
    let sensitive_input = r#"{"nativeThreadId":"do-not-echo-this-value"}"#;
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"))
        .args([
            "message",
            "send",
            "--human-user",
            "--to",
            sensitive_input,
            "--text",
            "fixture",
            "--json",
        ])
        .output()
        .unwrap_or_else(|error| panic!("message command: {error}"));

    assert_eq!(output.status.code(), Some(2));
    let record: serde_json::Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("JSON guidance: {error}"));
    let message = record["error"]["message"]
        .as_str()
        .unwrap_or_else(|| panic!("guidance message"));
    for expected in [
        "--to",
        "endpoint.serviceId",
        "endpoint.endpointId",
        "sessionId",
        "nativeThreadId",
        "sessions list",
        ".target",
    ] {
        assert!(message.contains(expected), "missing {expected}: {message}");
    }
    assert!(!message.contains("do-not-echo-this-value"));
}

#[test]
fn valid_session_ref_shape_reaches_discovery() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"))
        .args([
            "message",
            "send",
            "--human-user",
            "--to",
            &session_ref(),
            "--text",
            "fixture",
            "--json",
            "--service-directory",
            "/tmp/collaboration-message-feedback-absent",
        ])
        .output()
        .unwrap_or_else(|error| panic!("message command: {error}"));

    assert_eq!(output.status.code(), Some(3));
    let record: serde_json::Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("JSON discovery error: {error}"));
    assert_eq!(record["error"]["kind"], "unavailable");
}

#[test]
fn human_user_and_from_remain_mutually_exclusive() {
    let address = session_ref();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"))
        .args([
            "message",
            "send",
            "--human-user",
            "--to",
            &address,
            "--from",
            &address,
            "--text",
            "fixture",
            "--json",
        ])
        .output()
        .unwrap_or_else(|error| panic!("message command: {error}"));

    assert_eq!(output.status.code(), Some(2));
    let record: serde_json::Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("JSON conflict error: {error}"));
    assert_eq!(record["error"]["kind"], "invalidField");
}

#[test]
fn help_distinguishes_message_targets_from_lifecycle_addresses() {
    let message = std::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"))
        .args(["message", "send", "--help"])
        .output()
        .unwrap_or_else(|error| panic!("message help: {error}"));
    let message_help = String::from_utf8(message.stdout)
        .unwrap_or_else(|error| panic!("message help text: {error}"));
    assert!(message_help.contains("sessions list"));
    assert!(message_help.contains(".target"));
    assert!(message_help.contains("supplied self address"));

    let addresses = std::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"))
        .args(["addresses", "list", "--help"])
        .output()
        .unwrap_or_else(|error| panic!("addresses help: {error}"));
    let address_help = String::from_utf8(addresses.stdout)
        .unwrap_or_else(|error| panic!("addresses help text: {error}"));
    assert!(address_help.contains("lifecycle coverage"));
    assert!(address_help.contains("not message target discovery"));
}

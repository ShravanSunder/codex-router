use std::process::Command;

const HUMAN_ACTOR: &str = r#"{"kind":"human","humanId":"cli-test-human"}"#;

#[test]
fn board_help_lists_every_descriptive_command_family() {
    let output = Command::new(env!("CARGO_BIN_EXE_agent-sessions"))
        .args(["board", "--help"])
        .output()
        .unwrap_or_else(|error| panic!("board help should execute: {error}"));

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let help = String::from_utf8_lossy(&output.stdout);
    for command in [
        "project",
        "repository",
        "create",
        "update",
        "show",
        "list",
        "archive",
        "topic",
        "message",
        "thread",
        "inbox",
    ] {
        assert!(
            help.contains(command),
            "board help omitted {command}: {help}"
        );
    }
    assert!(help.contains("durable project message boards"));
}

#[test]
fn message_list_help_exposes_scope_selection_and_paging_as_distinct_inputs() {
    let output = Command::new(env!("CARGO_BIN_EXE_agent-sessions"))
        .args(["board", "message", "list", "--help"])
        .output()
        .unwrap_or_else(|error| panic!("message list help should execute: {error}"));

    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    for flag in [
        "--scope",
        "--selection",
        "--after-activity-sequence",
        "--from-activity-sequence",
        "--to-activity-sequence",
        "--limit",
        "--cursor",
    ] {
        assert!(
            help.contains(flag),
            "message list help omitted {flag}: {help}"
        );
    }
    assert!(help.contains("all-projects"));
    assert!(help.contains("after-position"));
}

#[test]
fn invalid_actor_is_rejected_before_service_discovery_without_echoing_identity_payload() {
    let output = Command::new(env!("CARGO_BIN_EXE_agent-sessions"))
        .args([
            "board",
            "project",
            "create",
            "--name",
            "Coordination",
            "--actor",
            r#"{"kind":"session","session":{"secret":"must-not-leak"}}"#,
            "--service-directory",
            "/tmp/absent-board-cli-service",
            "--json",
        ])
        .output()
        .unwrap_or_else(|error| panic!("invalid actor command should execute: {error}"));

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stderr.is_empty());
    let result = String::from_utf8_lossy(&output.stdout);
    assert!(result.contains("--actor must be valid typed Identity JSON"));
    assert!(!result.contains("must-not-leak"));
    assert!(!result.contains("boardUnavailable"));
}

#[test]
fn placement_validation_requires_only_the_matching_resource_id() {
    let id = "018f6f67-64d2-7a21-bf9a-8f193f987001";
    let output = Command::new(env!("CARGO_BIN_EXE_agent-sessions"))
        .args([
            "board",
            "message",
            "post",
            "--placement",
            "topic",
            "--root-message-id",
            id,
            "--text",
            "hello",
            "--actor",
            HUMAN_ACTOR,
            "--json",
        ])
        .output()
        .unwrap_or_else(|error| panic!("invalid placement command should execute: {error}"));

    assert_eq!(output.status.code(), Some(2));
    let result = String::from_utf8_lossy(&output.stdout);
    assert!(result.contains("--topic-id is required"));
    assert!(result.contains("--root-message-id must be omitted"));
}

#[test]
fn range_validation_rejects_reversed_activity_bounds_before_connecting() {
    let output = Command::new(env!("CARGO_BIN_EXE_agent-sessions"))
        .args([
            "board",
            "message",
            "list",
            "--scope",
            "all-projects",
            "--selection",
            "range",
            "--from-activity-sequence",
            "9",
            "--to-activity-sequence",
            "4",
            "--json",
        ])
        .output()
        .unwrap_or_else(|error| panic!("invalid range command should execute: {error}"));

    assert_eq!(output.status.code(), Some(2));
    let result = String::from_utf8_lossy(&output.stdout);
    assert!(result.contains("range start must not exceed range end"));
}

#[test]
fn message_list_omits_selection_to_use_latest_default() {
    let output = Command::new(env!("CARGO_BIN_EXE_agent-sessions"))
        .args([
            "board",
            "message",
            "list",
            "--scope",
            "all-projects",
            "--service-directory",
            "/tmp/absent-board-cli-service",
            "--json",
        ])
        .output()
        .unwrap_or_else(|error| panic!("default latest command should parse: {error}"));

    assert_eq!(output.status.code(), Some(3));
    let result = String::from_utf8_lossy(&output.stdout);
    assert!(result.contains("boardUnavailable"));
    assert!(!result.contains("--selection"));
}

#[test]
fn creation_and_post_help_expose_exact_utf8_and_reference_bounds() {
    let project_help = Command::new(env!("CARGO_BIN_EXE_agent-sessions"))
        .args(["board", "project", "create", "--help"])
        .output()
        .unwrap();
    assert!(project_help.status.success());
    let project_help = String::from_utf8_lossy(&project_help.stdout);
    assert!(project_help.contains("1 to 256 UTF-8 bytes"));
    assert!(project_help.contains("0 to 16384 UTF-8 bytes"));

    let message_help = Command::new(env!("CARGO_BIN_EXE_agent-sessions"))
        .args(["board", "message", "post", "--help"])
        .output()
        .unwrap();
    assert!(message_help.status.success());
    let message_help = String::from_utf8_lossy(&message_help.stdout);
    assert!(message_help.contains("1 to 65536 UTF-8 bytes"));
    assert!(message_help.contains("at most 64 distinct references total"));
}

#[test]
fn bounded_text_errors_name_exact_limits_without_echoing_input() {
    let secret_value = format!("secret-value-{}", "x".repeat(256));
    let output = Command::new(env!("CARGO_BIN_EXE_agent-sessions"))
        .args([
            "board",
            "project",
            "create",
            "--name",
            &secret_value,
            "--actor",
            HUMAN_ACTOR,
            "--json",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stderr.is_empty());
    let result = String::from_utf8_lossy(&output.stdout);
    assert!(result.contains("--name must contain 1 to 256 UTF-8 bytes after trimming"));
    assert!(!result.contains(&secret_value));
}

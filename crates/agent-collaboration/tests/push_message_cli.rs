//! Focused CLI coverage for fetching and replying to stored Router push records.
use serde_json::{Value, json};
use std::{error::Error, ffi::OsString, time::Duration};

mod fake_api_support;
use fake_api_support::{FakeCollaborationApi, FakeReply};

const SERVICE_ID: &str = "00000000-0000-4000-8000-000000000001";
const SERVICE_EPOCH: &str = "00000000-0000-4000-8000-000000000002";
const CALLER_SESSION_ID: &str = "push-cli-caller";
const SENDER_SESSION_ID: &str = "push-cli-sender";
const TARGET_SESSION_ID: &str = "push-cli-target";
const PUSH_ID: &str = "019f0000-0000-7000-8000-000000000101";
const PUSH_LINK: &str =
    "router://00000000-0000-4000-8000-000000000001/push/019f0000-0000-7000-8000-000000000101";
const NOTICE_LINE: &str =
    "💬 Agent → You: hello router://host/push/019f0000-0000-7000-8000-000000000101";

type TestResult<TValue> = Result<TValue, Box<dyn Error + Send + Sync>>;

#[tokio::test]
async fn root_show_fetches_a_push_by_link_using_the_harness_identity() {
    let (output, request) = invoke_with_reply(
        vec!["show".into(), PUSH_LINK.into(), "--json".into()],
        MockReply::Result(show_result()),
    )
    .await
    .expect("show command completes against the stand-in API");

    assert_eq!(
        request.get("tool").and_then(Value::as_str),
        Some("router_show")
    );
    assert_eq!(
        request.pointer("/arguments/reference"),
        Some(&json!(PUSH_LINK))
    );
    assert_eq!(
        request.pointer("/arguments/caller"),
        Some(&session_ref(CALLER_SESSION_ID, "codex-local"))
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "stdout: {}; stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout).expect("valid show JSON output");
    assert_eq!(
        response.pointer("/result/record/body"),
        Some(&json!("stored body"))
    );
    assert_eq!(
        response.pointer("/result/record/headerFacts/senderDisplayName"),
        Some(&serde_json::Value::Null)
    );
    assert_eq!(response.pointer("/result/link"), Some(&json!(PUSH_LINK)));
}

#[tokio::test]
async fn show_preserves_a_not_permitted_service_error() {
    let error = json!({
        "code": -32050,
        "message": "Push show rejected",
        "data": {
            "kind": "notPermitted",
            "stage": "inspect",
            "message": "Not permitted to read this push"
        }
    });
    let (output, request) = invoke_with_reply(
        vec!["show".into(), PUSH_ID.into(), "--json".into()],
        MockReply::Error(error),
    )
    .await
    .expect("show command completes against the stand-in API");

    assert_eq!(
        request.get("tool").and_then(Value::as_str),
        Some("router_show")
    );
    assert_eq!(
        request.pointer("/arguments/caller/sessionId"),
        Some(&json!(CALLER_SESSION_ID))
    );
    assert_eq!(
        output.status.code(),
        Some(4),
        "stdout: {}; stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout).expect("valid show error JSON");
    assert_eq!(
        response.pointer("/error/serviceKind"),
        Some(&json!("notPermitted"))
    );
    assert_eq!(
        response.pointer("/error/data/kind"),
        Some(&json!("notPermitted"))
    );
}

#[tokio::test]
async fn message_send_reports_the_stored_push_and_uses_the_harness_sender() {
    let target = session_ref(TARGET_SESSION_ID, "claude-local");
    let (output, request) = invoke_with_reply(
        vec![
            "message".into(),
            "send".into(),
            "--to".into(),
            serde_json::to_string(&target)
                .expect("serialize target SessionRef")
                .into(),
            "--text".into(),
            "hello recipient".into(),
        ],
        MockReply::Result(send_result(target)),
    )
    .await
    .expect("message send completes against the stand-in API");

    assert_eq!(
        request.get("tool").and_then(Value::as_str),
        Some("message_send")
    );
    assert_eq!(
        request.pointer("/arguments/message/sender"),
        Some(&session_ref(CALLER_SESSION_ID, "codex-local"))
    );
    assert_eq!(
        request.pointer("/arguments/message/text"),
        Some(&json!("hello recipient"))
    );
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).expect("CLI stdout is UTF-8");
    for expected in [PUSH_ID, PUSH_LINK, "peer message written", "Claude target"] {
        assert!(stdout.contains(expected), "missing {expected}: {stdout}");
    }
}

#[tokio::test]
async fn held_message_send_prints_the_delivery_condition_and_exits_zero() {
    let target = session_ref(TARGET_SESSION_ID, "claude-local");
    let mut result = send_result(target.clone());
    result["deliveryState"] = json!("held");
    result["receipt"]["outcome"] = json!({"kind":"unknown"});
    result["receipt"]["client"] = Value::Null;
    let (output, _) = invoke_with_reply(
        vec![
            "message".into(),
            "send".into(),
            "--to".into(),
            serde_json::to_string(&target)
                .expect("serialize target SessionRef")
                .into(),
            "--text".into(),
            "hello recipient".into(),
        ],
        MockReply::Result(result),
    )
    .await
    .expect("message send completes against the stand-in API");

    assert_eq!(output.status.code(), Some(0));
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr).expect("CLI stderr is UTF-8"),
        format!("held: {PUSH_LINK} — delivered when Claude target is next running\n")
    );
}

#[tokio::test]
async fn rejected_message_send_prints_one_actionable_line_and_exit_four() {
    let target = session_ref(TARGET_SESSION_ID, "claude-local");
    let mut result = send_result(target.clone());
    result["deliveryState"] = json!("rejected");
    result["receipt"]["outcome"] = json!({
        "kind":"rejected",
        "reason":"busy",
        "nextAction":"retryLater",
        "clientCode":null,
        "detail":"target is busy"
    });
    result["receipt"]["client"] = Value::Null;
    let (output, _) = invoke_with_reply(
        vec![
            "message".into(),
            "send".into(),
            "--to".into(),
            serde_json::to_string(&target)
                .expect("serialize target SessionRef")
                .into(),
            "--text".into(),
            "hello recipient".into(),
        ],
        MockReply::Result(result),
    )
    .await
    .expect("message send completes against the stand-in API");

    assert_eq!(output.status.code(), Some(4));
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr).expect("CLI stderr is UTF-8"),
        "error: Delivery to Claude target was rejected: target is busy — retry later\n"
    );
}

#[tokio::test]
async fn claude_queue_rejection_recommends_auto_and_exits_four() {
    let target = session_ref(TARGET_SESSION_ID, "claude-local");
    let mut result = send_result(target.clone());
    result["deliveryState"] = json!("rejected");
    result["receipt"]["outcome"] = json!({
        "kind":"rejected",
        "reason":"queueUnsupported",
        "nextAction":"correctRequest",
        "clientCode":null,
        "detail":"Queue delivery isn't supported for Claude Code terminals"
    });
    result["receipt"]["client"] = Value::Null;
    let (output, _) = invoke_with_reply(
        vec![
            "message".into(),
            "send".into(),
            "--to".into(),
            serde_json::to_string(&target)
                .expect("serialize target SessionRef")
                .into(),
            "--delivery".into(),
            "queue".into(),
            "--text".into(),
            "hello recipient".into(),
        ],
        MockReply::Result(result),
    )
    .await
    .expect("message send completes against the stand-in API");

    assert_eq!(output.status.code(), Some(4));
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr).expect("CLI stderr is UTF-8"),
        "error: Delivery to Claude target was rejected: Queue delivery isn't supported for Claude Code terminals — resend with --delivery auto\n"
    );
}

#[tokio::test]
async fn ambiguous_peer_rejection_lists_claims_and_the_branch_next_step() {
    let target = session_ref(TARGET_SESSION_ID, "claude-local");
    let mut result = send_result(target.clone());
    result["deliveryState"] = json!("rejected");
    result["receipt"]["outcome"] = json!({
        "kind":"rejected",
        "reason":"liveElsewhere",
        "nextAction":"inspectTarget",
        "clientCode":null,
        "detail":"this Claude session is claimed by 2 live terminals",
        "claims":[
            {"pid":52304,"name":"terminal-one","cwd":"/workspace/one"},
            {"pid":68833,"name":null,"cwd":"/Users/example/fallback-terminal"}
        ]
    });
    result["receipt"]["client"] = Value::Null;
    let (output, _) = invoke_with_reply(
        vec![
            "message".into(),
            "send".into(),
            "--to".into(),
            serde_json::to_string(&target)
                .expect("serialize target SessionRef")
                .into(),
            "--text".into(),
            "hello recipient".into(),
        ],
        MockReply::Result(result),
    )
    .await
    .expect("message send completes against the stand-in API");

    assert_eq!(output.status.code(), Some(4));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).expect("CLI stderr is UTF-8");
    assert_eq!(
        stderr,
        "error: Claude target is open in 2 terminals (pid 52304 terminal-one, pid 68833 fallback-terminal) — close one of these terminals, or run `/branch` in one of them\n"
    );
    assert!(!stderr.contains("/Users/example"));
}

#[tokio::test]
async fn unknown_message_send_points_to_show_before_retrying_and_exits_five() {
    let target = session_ref(TARGET_SESSION_ID, "claude-local");
    let mut result = send_result(target.clone());
    result["deliveryState"] = json!("outcome-unknown");
    result["receipt"]["outcome"] = json!({"kind":"unknown"});
    result["receipt"]["client"] = Value::Null;
    let (output, _) = invoke_with_reply(
        vec![
            "message".into(),
            "send".into(),
            "--to".into(),
            serde_json::to_string(&target)
                .expect("serialize target SessionRef")
                .into(),
            "--text".into(),
            "hello recipient".into(),
        ],
        MockReply::Result(result),
    )
    .await
    .expect("message send completes against the stand-in API");

    assert_eq!(output.status.code(), Some(5));
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr).expect("CLI stderr is UTF-8"),
        format!(
            "error: Delivery outcome for Claude target is unknown — run agent-collaboration show {PUSH_LINK} before retrying\n"
        )
    );
}

#[tokio::test]
async fn stored_message_send_outcome_unknown_keeps_the_push_link_and_exits_five() {
    let target = session_ref(TARGET_SESSION_ID, "claude-local");
    let message = format!(
        "Push was stored; delivery outcome is unknown. Inspect {PUSH_LINK} before retrying."
    );
    let error = json!({
        "code": -32050,
        "message": message,
        "data": {
            "kind": "outcomeUnknown",
            "stage": "inspect",
            "message": message
        }
    });
    let (output, _) = invoke_with_reply(
        vec![
            "message".into(),
            "send".into(),
            "--to".into(),
            serde_json::to_string(&target)
                .expect("serialize target SessionRef")
                .into(),
            "--text".into(),
            "hello recipient".into(),
        ],
        MockReply::Error(error),
    )
    .await
    .expect("message send completes against the stand-in API");

    assert_eq!(output.status.code(), Some(5));
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr).expect("CLI stderr is UTF-8"),
        format!(
            "error: Delivery to {TARGET_SESSION_ID}: {message} — inspect the target before retrying\n"
        )
    );
}

#[test]
fn invalid_message_target_prints_one_line_with_a_session_ref_example() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"))
        .args(["message", "send", "--to", "not-json", "--text", "hello"])
        .output()
        .expect("run invalid message target validation");

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let message = String::from_utf8(output.stderr).expect("CLI stderr is UTF-8");
    assert_eq!(message.lines().count(), 1);
    assert!(message.starts_with("error: --to must be compact SessionRef JSON"));
    assert!(message.contains("serviceId"));
    assert!(message.contains("sessionId"));
    assert!(message.ends_with(" — correct the named argument and try again\n"));
}

#[test]
fn invalid_show_reference_names_the_uuidv7_or_router_link_form() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"))
        .args(["show", "not-a-push-reference"])
        .output()
        .expect("run invalid push reference validation");

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let message = String::from_utf8(output.stderr).expect("CLI stderr is UTF-8");
    assert_eq!(message.lines().count(), 1);
    assert!(message.starts_with("error: PUSH_ID_OR_LINK must be a UUIDv7 push id"));
    assert!(message.contains("router://<machine-id>/push/<push-id>"));
    assert!(message.contains("019f0000-0000-7000-8000-000000000101"));
    assert!(message.ends_with(" — correct the named argument and try again\n"));
}

#[tokio::test]
async fn message_inbox_prints_only_the_notice_line() {
    let (output, request) = invoke_with_reply(
        vec!["message".into(), "inbox".into()],
        MockReply::Result(notice_list()),
    )
    .await
    .expect("message inbox completes against the stand-in API");

    assert_eq!(
        request.get("tool").and_then(Value::as_str),
        Some("message_inbox")
    );
    assert_eq!(
        request.pointer("/arguments/caller"),
        Some(&session_ref(CALLER_SESSION_ID, "codex-local"))
    );
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(output.stdout).expect("CLI stdout is UTF-8"),
        format!("{NOTICE_LINE}\n")
    );
}

#[tokio::test]
async fn message_history_uses_the_explicit_session_and_returns_notice_records() {
    let other_session = session_ref(SENDER_SESSION_ID, "claude-local");
    let (output, request) = invoke_with_reply(
        vec![
            "message".into(),
            "history".into(),
            "--with".into(),
            serde_json::to_string(&other_session)
                .expect("serialize requested SessionRef")
                .into(),
            "--json".into(),
        ],
        MockReply::Result(notice_list()),
    )
    .await
    .expect("message history completes against the stand-in API");

    assert_eq!(
        request.get("tool").and_then(Value::as_str),
        Some("message_history")
    );
    assert_eq!(request.pointer("/arguments/with"), Some(&other_session));
    assert_eq!(
        request.pointer("/arguments/caller/sessionId"),
        Some(&json!(CALLER_SESSION_ID))
    );
    assert_eq!(output.status.code(), Some(0));
    let response: Value =
        serde_json::from_slice(&output.stdout).expect("valid history JSON output");
    assert_eq!(
        response.pointer("/result/page/records/0/line"),
        Some(&json!(NOTICE_LINE))
    );
    assert!(response.pointer("/result/page/records/0/body").is_none());
}

#[tokio::test]
async fn message_reply_uses_an_explicit_push_id_and_reports_the_recipient() {
    let (output, request) = invoke_with_reply(
        vec![
            "message".into(),
            "reply".into(),
            PUSH_ID.into(),
            "answer text".into(),
            "--json".into(),
        ],
        MockReply::Result(reply_result()),
    )
    .await
    .expect("message reply completes against the stand-in API");

    assert_eq!(
        request.get("tool").and_then(Value::as_str),
        Some("message_reply")
    );
    assert_eq!(
        request.pointer("/arguments/reference"),
        Some(&json!(PUSH_ID))
    );
    assert_eq!(
        request.pointer("/arguments/text"),
        Some(&json!("answer text"))
    );
    assert_eq!(
        request.pointer("/arguments/caller"),
        Some(&session_ref(CALLER_SESSION_ID, "codex-local"))
    );
    assert_eq!(output.status.code(), Some(0));
    let response: Value = serde_json::from_slice(&output.stdout).expect("valid reply JSON output");
    assert_eq!(
        response.pointer("/result/record/pushId"),
        Some(&json!(PUSH_ID))
    );
    assert_eq!(
        response.pointer("/result/record/link"),
        Some(&json!(PUSH_LINK))
    );
    assert_eq!(
        response.pointer("/result/record/targetIdentity"),
        Some(&json!("Claude sender"))
    );
}

#[tokio::test]
async fn message_reply_accepts_a_router_link_reference() {
    let (output, request) = invoke_with_reply(
        vec![
            "message".into(),
            "reply".into(),
            PUSH_LINK.into(),
            "answer text".into(),
        ],
        MockReply::Result(reply_result()),
    )
    .await
    .expect("message reply completes against the stand-in API");

    assert_eq!(
        request.pointer("/arguments/reference"),
        Some(&json!(PUSH_LINK))
    );
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).expect("CLI stdout is UTF-8");
    assert!(stdout.contains("Claude sender"));
    assert!(stdout.contains(PUSH_ID));
    assert!(stdout.contains(PUSH_LINK));
}

#[test]
fn message_reply_without_a_reference_returns_syntax_guidance() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"))
        .args(["message", "reply", "--json"])
        .output()
        .expect("run message reply syntax check");

    assert_eq!(output.status.code(), Some(2));
    let response: Value = serde_json::from_slice(&output.stdout).expect("valid syntax error JSON");
    assert_eq!(
        response.pointer("/error/kind"),
        Some(&json!("invalidField"))
    );
    assert_eq!(response.pointer("/error/field"), Some(&json!("arguments")));
    assert_eq!(
        response.pointer("/error/nextAction"),
        Some(&json!("correctRequest"))
    );
    assert_eq!(
        response.pointer("/error/constraint"),
        Some(&json!("Use the command help to correct the arguments."))
    );
    let message = response
        .pointer("/error/message")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        message.contains("PUSH_ID_OR_LINK"),
        "missing required syntax: {message}"
    );
}

#[test]
fn message_history_rejects_an_invalid_session_reference_with_field_guidance() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"))
        .args(["message", "history", "--with", "not-json", "--json"])
        .output()
        .expect("run message history argument validation");

    assert_eq!(output.status.code(), Some(2));
    let response: Value = serde_json::from_slice(&output.stdout).expect("valid history error JSON");
    assert_eq!(
        response.pointer("/error/kind"),
        Some(&json!("invalidField"))
    );
    assert_eq!(response.pointer("/error/field"), Some(&json!("--with")));
    assert_eq!(
        response.pointer("/error/constraint"),
        Some(&json!(
            "--with must be compact SessionRef JSON with endpoint.serviceId, endpoint.endpointId, and sessionId; for example --with '{\"endpoint\":{\"serviceId\":\"00000000-0000-4000-8000-000000000001\",\"endpointId\":\"codex-local\"},\"sessionId\":\"target-session\"}'"
        ))
    );
    assert_eq!(
        response.pointer("/error/nextAction"),
        Some(&json!("correctRequest"))
    );
}

#[test]
fn root_show_help_exposes_the_push_reference_argument() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"))
        .args(["show", "--help"])
        .output()
        .expect("run show help");

    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).expect("show help is UTF-8");
    assert!(
        help.contains("PUSH_ID_OR_LINK"),
        "missing push syntax: {help}"
    );
}

#[test]
fn root_show_without_a_reference_returns_required_syntax() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"))
        .args(["show", "--json"])
        .output()
        .expect("run show argument validation");

    assert_eq!(output.status.code(), Some(2));
    let response: Value = serde_json::from_slice(&output.stdout).expect("valid show error JSON");
    assert_eq!(
        response.pointer("/error/kind"),
        Some(&json!("invalidField"))
    );
    assert_eq!(response.pointer("/error/field"), Some(&json!("arguments")));
    let message = response
        .pointer("/error/message")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        message.contains("PUSH_ID_OR_LINK"),
        "missing required syntax: {message}"
    );
}

#[test]
fn show_result_fixture_matches_the_typed_show_result() {
    serde_json::from_value::<collaboration_client::protocol::PushRecordShowResult>(show_result())
        .expect("show result fixture matches its typed result");
}

enum MockReply {
    Result(Value),
    Error(Value),
}

/// Runs the CLI against a stand-in API that answers its one tool call with `reply`, and
/// returns the output with the call as `{"tool", "arguments"}`.
async fn invoke_with_reply(
    arguments: Vec<OsString>,
    reply: MockReply,
) -> TestResult<(std::process::Output, Value)> {
    let mut fixture = FakeCollaborationApi::new(SERVICE_ID, SERVICE_EPOCH)?;
    // The message tools present a stored push's receipt; other results pass through as is.
    let reply = match reply {
        MockReply::Result(result) => FakeReply::Receipt(result),
        MockReply::Error(error) => FakeReply::Error(error),
    };
    let response_task = fixture.serve(vec![reply]);
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"));
    command
        .args(arguments)
        .arg("--service-directory")
        .arg(fixture.directory())
        .env("CODEX_THREAD_ID", CALLER_SESSION_ID)
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .env_remove("CURSOR_CONVERSATION_ID");
    let output = tokio::time::timeout(Duration::from_secs(5), command.output()).await??;
    let mut calls = response_task.await??;
    let request = calls.pop().ok_or("the CLI made no tool call")?;
    Ok((output, request))
}

fn session_ref(session_id: &str, endpoint_id: &str) -> Value {
    json!({
        "endpoint": {"serviceId": SERVICE_ID, "endpointId": endpoint_id},
        "sessionId": session_id
    })
}

fn show_result() -> Value {
    json!({
        "record": {
            "pushId": PUSH_ID,
            "kind": "direct-message",
            "origin": {"session": session_ref(SENDER_SESSION_ID, "claude-local")},
            "originRouterRef": null,
            "target": session_ref(CALLER_SESSION_ID, "codex-local"),
            "replyToPushId": null,
            "headerFacts": {"kind":"directMessage", "senderDisplayName":null},
            "body": "stored body",
            "activity": null,
            "deliveryState": "delivered",
            "lastOutcome": {
                "outcome": {"kind":"peerMessageWritten"},
                "reachability": "claudeCodePeer",
                "client": {"kind":"claudeCodePeer"}
            },
            "createdAt": "2026-09-30T12:00:00Z",
            "settledAt": "2026-09-30T12:00:01Z",
            "readAt": null
        },
        "link": PUSH_LINK,
        "activityRanges": []
    })
}

fn notice_list() -> Value {
    json!({
        "records": [{
            "pushId": PUSH_ID,
            "link": PUSH_LINK,
            "line": NOTICE_LINE,
            "origin": {"session": session_ref(SENDER_SESSION_ID, "claude-local")},
            "target": session_ref(CALLER_SESSION_ID, "codex-local"),
            "replyToPushId": null,
            "deliveryState": "delivered",
            "createdAt": "2026-09-30T12:00:00Z",
            "readAt": null
        }]
    })
}

fn send_result(target: Value) -> Value {
    json!({
        "pushId": PUSH_ID,
        "link": PUSH_LINK,
        "target": target,
        "targetIdentity": "Claude target",
        "deliveryState":"delivered",
        "receipt": {
            "outcome": {"kind":"peerMessageWritten"},
            "reachability": "claudeCodePeer",
            "client": {"kind":"claudeCodePeer"}
        }
    })
}

fn reply_result() -> Value {
    json!({
        "target": session_ref(SENDER_SESSION_ID, "claude-local"),
        "targetIdentity": "Claude sender",
        "deliveryState":"delivered",
        "pushId": PUSH_ID,
        "link": PUSH_LINK,
        "receipt": {
            "outcome": {"kind":"peerMessageWritten"},
            "reachability": "claudeCodePeer",
            "client": {"kind":"claudeCodePeer"}
        }
    })
}

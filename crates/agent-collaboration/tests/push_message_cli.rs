//! Focused CLI coverage for fetching and replying to stored Router push records.
use serde_json::{Value, json};
use std::{error::Error, ffi::OsString, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::UnixListener,
};

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
    .expect("show command completes against the Control fixture");

    assert_eq!(
        request.get("method").and_then(Value::as_str),
        Some("router/show")
    );
    assert_eq!(
        request.pointer("/params/reference"),
        Some(&json!(PUSH_LINK))
    );
    assert_eq!(
        request.pointer("/params/caller"),
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
    .expect("show command completes against the Control fixture");

    assert_eq!(
        request.get("method").and_then(Value::as_str),
        Some("router/show")
    );
    assert_eq!(
        request.pointer("/params/caller/sessionId"),
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
    .expect("message send completes against the Control fixture");

    assert_eq!(
        request.get("method").and_then(Value::as_str),
        Some("message/send")
    );
    assert_eq!(
        request.pointer("/params/message/sender"),
        Some(&session_ref(CALLER_SESSION_ID, "codex-local"))
    );
    assert_eq!(
        request.pointer("/params/message/text"),
        Some(&json!("hello recipient"))
    );
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).expect("CLI stdout is UTF-8");
    for expected in [PUSH_ID, PUSH_LINK, "peer message written", "Claude target"] {
        assert!(stdout.contains(expected), "missing {expected}: {stdout}");
    }
}

#[tokio::test]
async fn message_inbox_prints_only_the_notice_line() {
    let (output, request) = invoke_with_reply(
        vec!["message".into(), "inbox".into()],
        MockReply::Result(notice_list()),
    )
    .await
    .expect("message inbox completes against the Control fixture");

    assert_eq!(
        request.get("method").and_then(Value::as_str),
        Some("message/inbox")
    );
    assert_eq!(
        request.pointer("/params/caller"),
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
    .expect("message history completes against the Control fixture");

    assert_eq!(
        request.get("method").and_then(Value::as_str),
        Some("message/history")
    );
    assert_eq!(request.pointer("/params/with"), Some(&other_session));
    assert_eq!(
        request.pointer("/params/caller/sessionId"),
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
    .expect("message reply completes against the Control fixture");

    assert_eq!(
        request.get("method").and_then(Value::as_str),
        Some("message/reply")
    );
    assert_eq!(request.pointer("/params/reference"), Some(&json!(PUSH_ID)));
    assert_eq!(request.pointer("/params/text"), Some(&json!("answer text")));
    assert_eq!(
        request.pointer("/params/caller"),
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
    .expect("message reply completes against the Control fixture");

    assert_eq!(
        request.pointer("/params/reference"),
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
            "--with must be compact SessionRef JSON with endpoint.serviceId, endpoint.endpointId, and sessionId"
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
fn show_result_fixture_matches_the_typed_control_result() {
    serde_json::from_value::<collaboration_client::protocol::PushRecordShowResult>(show_result())
        .expect("show result fixture matches its Control type");
}

enum MockReply {
    Result(Value),
    Error(Value),
}

struct ServiceFixture {
    directory: tempfile::TempDir,
    _publication: collaboration_service::ManifestPublication,
    listener: Option<UnixListener>,
}

impl ServiceFixture {
    fn new() -> TestResult<Self> {
        let directory = tempfile::tempdir()?;
        let listener = UnixListener::bind(directory.path().join("control.sock"))?;
        let digest = format!("sha256:{}", "a".repeat(64));
        let manifest: collaboration_client::protocol::ServiceManifest =
            serde_json::from_value(json!({
                "version": 2,
                "serviceId": SERVICE_ID,
                "serviceEpoch": SERVICE_EPOCH,
                "machineLabel": "fixture-host",
                "control": {"transport": "unixJsonLines", "path": "control.sock"},
                "controlSchemaDigest": digest,
                "mcp": {"transport": "streamableHttp", "url": "http://127.0.0.1:0/mcp"}
            }))?;
        let publication =
            collaboration_service::ManifestPublication::publish(directory.path(), &manifest)?;
        Ok(Self {
            directory,
            _publication: publication,
            listener: Some(listener),
        })
    }
}

async fn invoke_with_reply(
    arguments: Vec<OsString>,
    reply: MockReply,
) -> TestResult<(std::process::Output, Value)> {
    let mut fixture = ServiceFixture::new()?;
    let listener = fixture.listener.take().ok_or("fixture listener missing")?;
    let response_task = tokio::spawn(serve_control_request(listener, reply));
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"));
    command
        .args(arguments)
        .arg("--service-directory")
        .arg(fixture.directory.path())
        .env("CODEX_THREAD_ID", CALLER_SESSION_ID)
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .env_remove("CURSOR_CONVERSATION_ID");
    let output = tokio::time::timeout(Duration::from_secs(5), command.output()).await??;
    let request = response_task.await??;
    Ok((output, request))
}

async fn serve_control_request(listener: UnixListener, reply: MockReply) -> TestResult<Value> {
    let (stream, _) = listener.accept().await?;
    let mut stream = BufReader::new(stream);
    let initialize = read_request(&mut stream).await?;
    assert_eq!(
        initialize.get("method").and_then(Value::as_str),
        Some("control/initialize")
    );
    let initialize_id = initialize
        .get("id")
        .cloned()
        .ok_or("initialize ID missing")?;
    let initialized = json!({
        "jsonrpc": "2.0",
        "id": initialize_id,
        "result": {
            "version": {"major": 1, "minor": 0},
            "serviceId": SERVICE_ID,
            "serviceEpoch": SERVICE_EPOCH,
            "controlSchemaDigest": format!("sha256:{}", "a".repeat(64))
        }
    });
    stream
        .get_mut()
        .write_all(format!("{initialized}\n").as_bytes())
        .await?;

    let request = read_request(&mut stream).await?;
    let request_id = request.get("id").cloned().ok_or("request ID missing")?;
    let response = match reply {
        MockReply::Result(result) => json!({"jsonrpc":"2.0","id":request_id,"result":result}),
        MockReply::Error(error) => json!({"jsonrpc":"2.0","id":request_id,"error":error}),
    };
    stream
        .get_mut()
        .write_all(format!("{response}\n").as_bytes())
        .await?;
    Ok(request)
}

async fn read_request(stream: &mut BufReader<tokio::net::UnixStream>) -> TestResult<Value> {
    let mut line = String::new();
    stream.read_line(&mut line).await?;
    Ok(serde_json::from_str(&line)?)
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
        "pushId": PUSH_ID,
        "link": PUSH_LINK,
        "receipt": {
            "outcome": {"kind":"peerMessageWritten"},
            "reachability": "claudeCodePeer",
            "client": {"kind":"claudeCodePeer"}
        }
    })
}

#![allow(clippy::expect_used)]
//! Fail-fast fixtures for live Claude Code registry admission.

use claude_code_peer_messaging::{ClaudeCodeSessionRegistry, PeerSessionLookup, PeerSessionStatus};
use collaboration_protocol::SessionId;
use serde_json::{Value, json};

fn live_record_fixture(process_id: u32, session_id: &str) -> Value {
    let mut record: Value =
        serde_json::from_str(include_str!("fixtures/claude_code_session_record.json"))
            .expect("Claude Code registry fixture");
    let object = record
        .as_object_mut()
        .expect("registry fixture is an object");
    object.insert("pid".to_owned(), json!(process_id));
    object.insert("sessionId".to_owned(), json!(session_id));
    record
}

#[test]
fn live_registry_entry_is_writable_only_for_supported_protocol() {
    let root = tempfile::tempdir().expect("registry directory");
    let pid = std::process::id();
    let path = root.path().join(format!("{pid}.json"));
    let session_id = SessionId::try_from("fixture-session".to_owned()).expect("session ID");
    std::fs::write(
        &path,
        json!({
            "pid": pid,
            "sessionId": "fixture-session",
            "status": "busy",
            "peerProtocol": 1,
            "messagingSocketPath": root.path().join("peer.sock"),
            "otherClaudeField": "ignored"
        })
        .to_string(),
    )
    .expect("registry record");
    let registry = ClaudeCodeSessionRegistry::new(root.path().to_owned());

    let lookup = registry.lookup(&session_id).expect("live record");

    let PeerSessionLookup::Writable(record) = lookup else {
        panic!("supported live peer should be writable")
    };
    assert_eq!(record.status, PeerSessionStatus::Busy);
    assert_eq!(record.session_id, session_id);

    std::fs::write(
        &path,
        json!({
            "pid": pid,
            "sessionId": "fixture-session",
            "status": "idle",
            "peerProtocol": 1,
            "messagingSocketPath": root.path().join("peer.sock")
        })
        .to_string(),
    )
    .expect("idle record");
    let PeerSessionLookup::Writable(idle) = registry.lookup(&session_id).expect("idle lookup")
    else {
        panic!("idle supported peer should be writable")
    };
    assert_eq!(idle.status, PeerSessionStatus::Idle);

    std::fs::write(
        &path,
        json!({
            "pid": pid,
            "sessionId": "fixture-session",
            "status": "idle",
            "peerProtocol": 2,
            "messagingSocketPath": root.path().join("peer.sock")
        })
        .to_string(),
    )
    .expect("unsupported protocol record");
    assert!(matches!(
        registry.lookup(&session_id).expect("unsupported lookup"),
        PeerSessionLookup::LiveUnsupported { .. }
    ));

    std::fs::write(
        &path,
        json!({
            "pid": pid,
            "sessionId": "fixture-session",
            "status": "shell",
            "peerProtocol": 1,
            "messagingSocketPath": root.path().join("peer.sock")
        })
        .to_string(),
    )
    .expect("shell status record");
    assert!(matches!(
        registry.lookup(&session_id).expect("shell lookup"),
        PeerSessionLookup::Writable(_)
    ));
}

#[test]
fn live_registry_status_is_advisory_and_preserves_unknown_values() {
    let root = tempfile::tempdir().expect("registry directory");
    let process_id = std::process::id();
    let session_id = SessionId::try_from("fixture-session".to_owned()).expect("session ID");
    let path = root.path().join(format!("{process_id}.json"));
    let registry = ClaudeCodeSessionRegistry::new(root.path().to_owned());

    for (status, expected) in [
        (Some("busy"), PeerSessionStatus::Busy),
        (Some("idle"), PeerSessionStatus::Idle),
        (Some("waiting"), PeerSessionStatus::Waiting),
        (Some("shell"), PeerSessionStatus::Shell),
        (
            Some("compacting"),
            PeerSessionStatus::Other("compacting".to_owned()),
        ),
        (None, PeerSessionStatus::Unreported),
    ] {
        let mut record = json!({
            "pid": process_id,
            "sessionId": "fixture-session",
            "peerProtocol": 1,
            "messagingSocketPath": root.path().join("peer.sock"),
        });
        if let Some(status) = status {
            record["status"] = json!(status);
        }
        std::fs::write(&path, record.to_string()).expect("registry record");

        let lookup = registry.lookup(&session_id).expect("live registry lookup");

        let PeerSessionLookup::Writable(record) = lookup else {
            panic!("live status {status:?} must not make the peer unsupported")
        };
        assert_eq!(record.status, expected);
    }
}

#[test]
fn dead_or_missing_registry_entry_is_absent() {
    let root = tempfile::tempdir().expect("registry directory");
    let registry = ClaudeCodeSessionRegistry::new(root.path().to_owned());
    let session_id = SessionId::try_from("fixture-session".to_owned()).expect("session ID");
    assert!(matches!(
        registry.lookup(&session_id).expect("missing lookup"),
        PeerSessionLookup::Absent
    ));

    let dead_pid = 999_999_u32;
    std::fs::write(
        root.path().join(format!("{dead_pid}.json")),
        json!({
            "pid": dead_pid,
            "sessionId": "fixture-session",
            "status": "idle",
            "peerProtocol": 1,
            "messagingSocketPath": root.path().join("dead.sock")
        })
        .to_string(),
    )
    .expect("dead record");
    assert!(matches!(
        registry.lookup(&session_id).expect("dead lookup"),
        PeerSessionLookup::Absent
    ));
}

#[test]
fn malformed_live_registry_record_fails_closed_when_no_target_is_readable() {
    let root = tempfile::tempdir().expect("registry directory");
    let pid = std::process::id();
    std::fs::write(root.path().join(format!("{pid}.json")), b"{malformed")
        .expect("malformed live fixture");
    let registry = ClaudeCodeSessionRegistry::new(root.path().to_owned());
    let session_id = SessionId::try_from("fixture-session".to_owned()).expect("session ID");

    assert!(registry.lookup(&session_id).is_err());
}

#[test]
fn sanitized_claude_code_record_fixture_is_writable() {
    let root = tempfile::tempdir().expect("registry directory");
    let process_id = std::process::id();
    let session_id = SessionId::try_from("fixture-session".to_owned()).expect("session ID");
    let record = live_record_fixture(process_id, "fixture-session");
    std::fs::write(
        root.path().join(format!("{process_id}.json")),
        record.to_string(),
    )
    .expect("registry record");

    let lookup = ClaudeCodeSessionRegistry::new(root.path().to_owned())
        .lookup(&session_id)
        .expect("lookup");

    assert!(matches!(lookup, PeerSessionLookup::Writable(_)));
}

#[test]
fn unrelated_corrupt_live_record_does_not_hide_valid_target() {
    let root = tempfile::tempdir().expect("registry directory");
    let mut unrelated_process = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("unrelated live process");
    let unrelated_pid = unrelated_process.id();
    let target_pid = std::process::id();
    let target = SessionId::try_from("fixture-target".to_owned()).expect("target session ID");
    std::fs::write(
        root.path().join(format!("{unrelated_pid}.json")),
        b"{corrupt unrelated record",
    )
    .expect("unrelated corrupt registry record");
    std::fs::write(
        root.path().join(format!("{target_pid}.json")),
        live_record_fixture(target_pid, "fixture-target").to_string(),
    )
    .expect("target registry record");
    let registry = ClaudeCodeSessionRegistry::new(root.path().to_owned());

    let lookup = registry
        .lookup(&target)
        .expect("valid target survives corrupt record");
    unrelated_process.kill().expect("stop fixture process");
    unrelated_process.wait().expect("reap fixture process");

    assert!(matches!(lookup, PeerSessionLookup::Writable(_)));
}

#[test]
fn unsupported_target_protocol_reports_its_version() {
    let root = tempfile::tempdir().expect("registry directory");
    let process_id = std::process::id();
    let session_id = SessionId::try_from("fixture-session".to_owned()).expect("session ID");
    let mut record = live_record_fixture(process_id, "fixture-session");
    record
        .as_object_mut()
        .expect("registry fixture is an object")
        .insert("peerProtocol".to_owned(), json!(2));
    std::fs::write(
        root.path().join(format!("{process_id}.json")),
        record.to_string(),
    )
    .expect("unsupported target registry record");

    let lookup = ClaudeCodeSessionRegistry::new(root.path().to_owned())
        .lookup(&session_id)
        .expect("unsupported records remain readable");

    assert!(matches!(
        lookup,
        PeerSessionLookup::LiveUnsupported { reason }
            if reason.contains("protocol 2")
    ));
}

#![allow(clippy::expect_used)]
//! Fail-fast fixtures for live Claude Code registry admission.

use claude_code_peer_messaging::{ClaudeCodeSessionRegistry, PeerSessionLookup, PeerSessionStatus};
use collaboration_protocol::SessionId;
use serde_json::json;

#[test]
fn live_registry_entry_is_writable_only_for_supported_protocol_and_status() {
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
    .expect("unsupported status record");
    assert!(matches!(
        registry.lookup(&session_id).expect("status lookup"),
        PeerSessionLookup::LiveUnsupported { .. }
    ));
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
fn malformed_live_registry_record_fails_closed() {
    let root = tempfile::tempdir().expect("registry directory");
    let pid = std::process::id();
    std::fs::write(root.path().join(format!("{pid}.json")), b"{malformed")
        .expect("malformed live fixture");
    let registry = ClaudeCodeSessionRegistry::new(root.path().to_owned());
    let session_id = SessionId::try_from("fixture-session".to_owned()).expect("session ID");

    assert!(registry.lookup(&session_id).is_err());
}

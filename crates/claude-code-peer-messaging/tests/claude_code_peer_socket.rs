#![allow(clippy::expect_used, clippy::indexing_slicing)]
//! Fail-fast fake socket assertions at the peer wire boundary.

use agent_automation::PeerProcessId;
use claude_code_peer_messaging::{
    ClaudeCodePeerSocket, PeerSessionRecord, PeerSessionStatus, PeerSocketWriteOutcome,
};
use collaboration_protocol::SessionId;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use std::{os::unix::fs::PermissionsExt as _, path::Path};
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, BufReader};

fn fixture_record(root: &Path) -> PeerSessionRecord {
    PeerSessionRecord {
        session_id: SessionId::try_from("fixture-session".to_owned()).expect("session ID"),
        process_id: PeerProcessId::try_from(std::process::id()).expect("process ID"),
        status: PeerSessionStatus::Busy,
        socket_path: root.join("peer.sock"),
    }
}

fn publish_fixture_key(root: &Path, record: &PeerSessionRecord) {
    let digest = Sha256::digest(record.socket_path.to_str().expect("socket path").as_bytes());
    let pid = u32::from(record.process_id);
    let path = root.join(format!("{pid}.{digest:x}.key"));
    std::fs::write(
        &path,
        json!({"peerToken":"0123456789abcdef0123456789abcdef"}).to_string(),
    )
    .expect("key fixture");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
        .expect("owner-only key fixture");
}

#[tokio::test]
async fn full_auth_and_user_frames_report_written_without_acceptance_claim() {
    let root = tempfile::tempdir().expect("peer root");
    let record = fixture_record(root.path());
    publish_fixture_key(root.path(), &record);
    let listener = tokio::net::UnixListener::bind(&record.socket_path).expect("peer listener");
    let receiver = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accepted peer");
        let mut lines = BufReader::new(stream).lines();
        let auth: Value =
            serde_json::from_str(&lines.next_line().await.expect("auth line").expect("auth"))
                .expect("auth JSON");
        let user: Value =
            serde_json::from_str(&lines.next_line().await.expect("user line").expect("user"))
                .expect("user JSON");
        (auth, user)
    });

    let outcome = ClaudeCodePeerSocket::new(root.path().to_owned())
        .write_user_message(&record, "hello peer")
        .await;

    assert_eq!(outcome, PeerSocketWriteOutcome::Written);
    let (auth, user) = receiver.await.expect("receiver");
    assert_eq!(auth["type"], "auth");
    assert_eq!(auth["token"], "0123456789abcdef0123456789abcdef");
    assert_eq!(
        user,
        json!({"type":"user","message":{"role":"user","content":"hello peer"}})
    );
}

#[tokio::test]
async fn refused_peer_socket_is_known_not_submitted() {
    let root = tempfile::tempdir().expect("peer root");
    let record = fixture_record(root.path());
    publish_fixture_key(root.path(), &record);

    let outcome = ClaudeCodePeerSocket::new(root.path().to_owned())
        .write_user_message(&record, "hello peer")
        .await;

    assert!(matches!(
        outcome,
        PeerSocketWriteOutcome::NotSubmitted { .. }
    ));
}

#[tokio::test]
async fn missing_published_key_is_known_not_submitted() {
    let root = tempfile::tempdir().expect("peer root");
    let record = fixture_record(root.path());

    let outcome = ClaudeCodePeerSocket::new(root.path().to_owned())
        .write_user_message(&record, "hello peer")
        .await;

    assert!(
        matches!(outcome, PeerSocketWriteOutcome::NotSubmitted { reason } if reason.contains("authentication key"))
    );
}

#[tokio::test]
async fn interrupted_write_after_user_bytes_is_unknown() {
    let root = tempfile::tempdir().expect("peer root");
    let record = fixture_record(root.path());
    publish_fixture_key(root.path(), &record);
    let listener = tokio::net::UnixListener::bind(&record.socket_path).expect("peer listener");
    let receiver = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accepted peer");
        let mut reader = BufReader::new(stream);
        let mut auth = String::new();
        reader.read_line(&mut auth).await.expect("auth frame");
        let mut first_user_bytes = [0_u8; 1024];
        reader
            .read_exact(&mut first_user_bytes)
            .await
            .expect("partial user frame");
    });

    let outcome = ClaudeCodePeerSocket::new(root.path().to_owned())
        .write_user_message(&record, &"x".repeat(900_000))
        .await;

    receiver.await.expect("receiver");
    assert!(matches!(outcome, PeerSocketWriteOutcome::Unknown { .. }));
}

#![allow(clippy::expect_used, clippy::indexing_slicing)]
//! Real registry/socket fixtures at the peer route boundary.

use agent_automation::{
    ClaudeCodePeerEffectEvidence, PeerProcessId, PeerSessionReference, PeerWriteEffect,
    RouteEffectEvidence,
};
use claude_code_peer_messaging::{ClaudeCodePeerSocket, ClaudeCodeSessionRegistry};
use codex_router_host::ClaudeCodePeerDeliveryRoute;
use collaboration_protocol::{
    AttemptId, CodexGeneration, DeliveryClientReceipt, DeliveryCorrelationId, DeliveryOutcome,
    DeliveryRejectionReason, EndpointId, EndpointRef, MessageContent, MessageDelivery, MessageText,
    SessionId, SessionRef, UuidIdentity,
};
use collaboration_service::{
    AttemptEvidenceSink, AttemptReconciliationContext, DeliveryFuture, DeliveryPrecondition,
    DeliveryRequest, RouteClaim, SessionDeliveryRoute,
};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use std::{os::unix::fs::PermissionsExt as _, path::Path, sync::Arc};
use tokio::io::{AsyncBufReadExt as _, BufReader};

struct RecordedPeerEvidence(
    tokio::sync::Mutex<Vec<RouteEffectEvidence<SessionRef, CodexGeneration>>>,
);

impl AttemptEvidenceSink for RecordedPeerEvidence {
    fn record(
        &self,
        evidence: RouteEffectEvidence<SessionRef, CodexGeneration>,
    ) -> DeliveryFuture<'_, ()> {
        Box::pin(async move {
            self.0.lock().await.push(evidence);
            Ok(())
        })
    }
}

fn target() -> SessionRef {
    SessionRef {
        endpoint: EndpointRef {
            service_id: UuidIdentity::try_from("0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89".to_owned())
                .expect("service ID"),
            endpoint_id: EndpointId::try_from("claude-local".to_owned()).expect("endpoint ID"),
        },
        session_id: SessionId::try_from("fixture-session".to_owned()).expect("session ID"),
    }
}

fn publish_peer(root: &Path, status: &str) -> std::path::PathBuf {
    let pid = std::process::id();
    let socket_path = root.join("peer.sock");
    std::fs::write(
        root.join(format!("{pid}.json")),
        json!({
            "pid": pid,
            "sessionId": "fixture-session",
            "status": status,
            "peerProtocol": 1,
            "messagingSocketPath": socket_path,
        })
        .to_string(),
    )
    .expect("registry fixture");
    let digest = Sha256::digest(socket_path.to_str().expect("socket path").as_bytes());
    let key_path = root.join(format!("{pid}.{digest:x}.key"));
    std::fs::write(
        &key_path,
        json!({"peerToken":"0123456789abcdef0123456789abcdef"}).to_string(),
    )
    .expect("auth fixture");
    std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600))
        .expect("private key");
    socket_path
}

fn delivery(mode: MessageDelivery) -> DeliveryRequest {
    DeliveryRequest {
        target: target(),
        message: MessageContent::HumanUser {
            text: MessageText::try_from("hello Claude".to_owned()).expect("message text"),
        },
        mode,
        precondition: DeliveryPrecondition::Unpinned,
        correlation: DeliveryCorrelationId::try_from("peer-test-correlation".to_owned())
            .expect("correlation"),
        attempt: AttemptId::generate(),
    }
}

#[tokio::test]
async fn peer_route_writes_origin_and_reply_line_without_claiming_acceptance() {
    let root = tempfile::tempdir().expect("registry root");
    let socket_path = publish_peer(root.path(), "busy");
    let listener = tokio::net::UnixListener::bind(&socket_path).expect("peer listener");
    let receiver = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accepted peer");
        let mut lines = BufReader::new(stream).lines();
        let _auth: Value =
            serde_json::from_str(&lines.next_line().await.expect("auth line").expect("auth"))
                .expect("auth JSON");
        let user: Value =
            serde_json::from_str(&lines.next_line().await.expect("user line").expect("user"))
                .expect("user JSON");
        user
    });
    let route = ClaudeCodePeerDeliveryRoute::new(
        target().endpoint.service_id,
        Arc::new(ClaudeCodeSessionRegistry::new(root.path().to_owned())),
        Arc::new(ClaudeCodePeerSocket::new(root.path().to_owned())),
    );
    let evidence = RecordedPeerEvidence(tokio::sync::Mutex::new(Vec::new()));
    assert!(matches!(
        route.claim(&target()).await.expect("peer claim"),
        RouteClaim::Holds
    ));
    let mut request = delivery(MessageDelivery::Auto);
    let mut sender = target();
    sender.endpoint.endpoint_id =
        EndpointId::try_from("codex-local".to_owned()).expect("sender endpoint");
    sender.session_id = SessionId::try_from("sender-session".to_owned()).expect("sender session");
    request.message = MessageContent::Agent {
        sender: sender.clone(),
        text: MessageText::try_from("hello Claude".to_owned()).expect("message"),
    };

    let outcome = route.deliver(request, &evidence).await.expect("delivery");

    assert_eq!(outcome.outcome, DeliveryOutcome::PeerMessageWritten);
    assert!(matches!(
        outcome.client,
        Some(DeliveryClientReceipt::ClaudeCodePeer)
    ));
    let user = receiver.await.expect("receiver");
    let content = user["message"]["content"]
        .as_str()
        .expect("peer message content");
    assert!(content.contains(&serde_json::to_string(&sender).expect("sender JSON")));
    assert!(content.contains("message_send"));
    let records = evidence.0.lock().await;
    assert_eq!(records.len(), 2);
    assert!(
        matches!(&records[0], RouteEffectEvidence::ClaudeCodePeer(peer) if peer.write == PeerWriteEffect::Dispatching)
    );
    assert!(
        matches!(&records[1], RouteEffectEvidence::ClaudeCodePeer(peer) if peer.write == PeerWriteEffect::Written)
    );
}

#[tokio::test]
async fn peer_queue_rejects_and_idle_steer_does_not_write() {
    let root = tempfile::tempdir().expect("registry root");
    publish_peer(root.path(), "idle");
    let route = ClaudeCodePeerDeliveryRoute::new(
        target().endpoint.service_id,
        Arc::new(ClaudeCodeSessionRegistry::new(root.path().to_owned())),
        Arc::new(ClaudeCodePeerSocket::new(root.path().to_owned())),
    );
    let evidence = RecordedPeerEvidence(tokio::sync::Mutex::new(Vec::new()));

    let queued = route
        .deliver(delivery(MessageDelivery::Queue), &evidence)
        .await
        .expect("queue response");
    let steered = route
        .deliver(delivery(MessageDelivery::Steer), &evidence)
        .await
        .expect("steer response");

    assert!(
        matches!(queued.outcome, DeliveryOutcome::Rejected(rejection)
            if rejection.reason == DeliveryRejectionReason::QueueUnsupported)
    );
    assert!(
        matches!(steered.outcome, DeliveryOutcome::NotSubmitted { reason, .. } if reason == "no running turn")
    );
    assert!(evidence.0.lock().await.is_empty());
}

#[tokio::test]
async fn live_unknown_peer_protocol_reports_live_elsewhere() {
    let root = tempfile::tempdir().expect("registry root");
    let socket_path = publish_peer(root.path(), "idle");
    std::fs::write(
        root.path().join(format!("{}.json", std::process::id())),
        json!({
            "pid": std::process::id(),
            "sessionId": "fixture-session",
            "status": "idle",
            "peerProtocol": 99,
            "messagingSocketPath": socket_path,
        })
        .to_string(),
    )
    .expect("unsupported registry entry");
    let route = ClaudeCodePeerDeliveryRoute::new(
        target().endpoint.service_id,
        Arc::new(ClaudeCodeSessionRegistry::new(root.path().to_owned())),
        Arc::new(ClaudeCodePeerSocket::new(root.path().to_owned())),
    );
    let evidence = RecordedPeerEvidence(tokio::sync::Mutex::new(Vec::new()));

    let claim = route.claim(&target()).await.expect("claim");
    let receipt = route
        .deliver(delivery(MessageDelivery::Auto), &evidence)
        .await
        .expect("delivery refusal");

    assert!(matches!(
        claim,
        RouteClaim::LiveElsewhere { writable: false }
    ));
    assert!(
        matches!(receipt.outcome, DeliveryOutcome::Rejected(rejection)
        if rejection.reason == DeliveryRejectionReason::LiveElsewhere)
    );
    assert!(evidence.0.lock().await.is_empty());
}

#[tokio::test]
async fn peer_reconciliation_rejects_evidence_for_another_session() {
    let root = tempfile::tempdir().expect("registry root");
    let route = ClaudeCodePeerDeliveryRoute::new(
        target().endpoint.service_id,
        Arc::new(ClaudeCodeSessionRegistry::new(root.path().to_owned())),
        Arc::new(ClaudeCodePeerSocket::new(root.path().to_owned())),
    );
    let recorded = RouteEffectEvidence::ClaudeCodePeer(ClaudeCodePeerEffectEvidence {
        session_id: PeerSessionReference::try_from("another-session".to_owned())
            .expect("recorded session"),
        process_id: PeerProcessId::try_from(std::process::id()).expect("process"),
        write: PeerWriteEffect::Written,
    });

    let outcome = route
        .reconcile_attempt(AttemptReconciliationContext {
            target: target(),
            message: delivery(MessageDelivery::Auto).message,
            mode: MessageDelivery::Auto,
            recorded,
        })
        .await;

    assert!(outcome.is_err());
}

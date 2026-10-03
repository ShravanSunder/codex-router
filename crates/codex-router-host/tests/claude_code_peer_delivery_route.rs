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
    DeliveryRejectionReason, EndpointId, EndpointRef, MessageDelivery, MessageText, PushId,
    SessionId, SessionReachability, SessionRef, UuidIdentity,
};
use collaboration_service::{
    AttemptEvidenceSink, AttemptReconciliationContext, DeliveryFuture, DeliveryPrecondition,
    RouteClaim, RoutePresence, SessionDeliveryRoute, SessionDeliveryRouter, SessionMessageDelivery,
    layer_zero::{DeliveryRequest as PreparedDeliveryRequest, PreparedPush},
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

fn publish_peer(root: &Path, status: Option<&str>) -> std::path::PathBuf {
    let pid = std::process::id();
    let socket_path = root.join("peer.sock");
    let mut record = json!({
        "pid": pid,
        "sessionId": "fixture-session",
        "peerProtocol": 1,
        "messagingSocketPath": socket_path,
    });
    if let Some(status) = status {
        record["status"] = json!(status);
    }
    std::fs::write(root.join(format!("{pid}.json")), record.to_string()).expect("registry fixture");
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

fn publish_peer_claim(root: &Path, process_id: u32, name: Option<&str>, cwd: Option<&str>) {
    let mut record = json!({
        "pid": process_id,
        "sessionId": "fixture-session",
        "peerProtocol": 99,
    });
    if let Some(name) = name {
        record["name"] = json!(name);
    }
    if let Some(cwd) = cwd {
        record["cwd"] = json!(cwd);
    }
    std::fs::write(root.join(format!("{process_id}.json")), record.to_string())
        .expect("registry claim fixture");
}

fn delivery(mode: MessageDelivery) -> PreparedDeliveryRequest {
    let target = target();
    let push_id =
        PushId::try_from(AttemptId::generate().as_str().to_owned()).expect("UUIDv7 push id");
    let correlation =
        DeliveryCorrelationId::try_from(push_id.as_str().to_owned()).expect("push id correlation");
    let line = MessageText::try_from(format!(
        "✉️ sender · \"hello Claude\" · router://{}/push/{}",
        String::from(target.endpoint.service_id.clone()),
        push_id.as_str()
    ))
    .expect("prepared push line");
    PreparedDeliveryRequest {
        payload: PreparedPush {
            push_id,
            line,
            load_policy: collaboration_service::LoadPolicy::MayLoad,
        },
        target,
        mode,
        precondition: DeliveryPrecondition::Unpinned,
        correlation,
        attempt: AttemptId::generate(),
    }
}

#[tokio::test]
async fn prepared_push_reaches_peer_route_as_the_exact_router_line() {
    let root = tempfile::tempdir().expect("registry root");
    let socket_path = publish_peer(root.path(), Some("busy"));
    let listener = tokio::net::UnixListener::bind(&socket_path).expect("peer listener");
    let receiver = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accepted peer");
        let mut lines = BufReader::new(stream).lines();
        let _auth: Value =
            serde_json::from_str(&lines.next_line().await.expect("auth line").expect("auth"))
                .expect("auth JSON");
        serde_json::from_str::<Value>(&lines.next_line().await.expect("user line").expect("user"))
            .expect("user JSON")
    });
    let route = Arc::new(ClaudeCodePeerDeliveryRoute::new(
        target().endpoint,
        Arc::new(ClaudeCodeSessionRegistry::new(root.path().to_owned())),
        Arc::new(ClaudeCodePeerSocket::new(root.path().to_owned())),
    ));
    let router = SessionDeliveryRouter::new(vec![route]);
    let evidence = RecordedPeerEvidence(tokio::sync::Mutex::new(Vec::new()));
    let push_id = PushId::try_from("018f47d2-24d5-7a68-b9ec-6f759c39458f".to_owned())
        .expect("UUIDv7 push id");
    let line = MessageText::try_from(format!(
        "✉️ Main · \"S1 is green\" · router://00000000-0000-4000-8000-000000000001/push/{}",
        push_id.as_str()
    ))
    .expect("push line");
    let correlation =
        DeliveryCorrelationId::try_from(push_id.as_str().to_owned()).expect("push id correlation");
    assert_eq!(correlation.as_str(), push_id.as_str());
    let request = PreparedDeliveryRequest {
        payload: PreparedPush {
            push_id,
            line: line.clone(),
            load_policy: collaboration_service::LoadPolicy::LoadedOnly,
        },
        target: target(),
        mode: MessageDelivery::Auto,
        precondition: DeliveryPrecondition::Unpinned,
        correlation,
        attempt: AttemptId::generate(),
    };

    let receipt = router
        .deliver(request, &evidence)
        .await
        .expect("prepared push delivery");

    assert_eq!(receipt.outcome, DeliveryOutcome::PeerMessageWritten);
    assert_eq!(
        receipt.reachability,
        Some(SessionReachability::ClaudeCodePeer)
    );
    assert!(matches!(
        receipt.client,
        Some(DeliveryClientReceipt::ClaudeCodePeer)
    ));
    let user = receiver.await.expect("receiver");
    assert_eq!(user["message"]["content"], line.as_str());
    assert!(
        !user["message"]["content"]
            .as_str()
            .expect("peer message content")
            .contains("For follow-up messages")
    );
}

#[tokio::test]
async fn peer_queue_rejects_but_idle_steer_writes_to_the_live_socket() {
    let root = tempfile::tempdir().expect("registry root");
    let socket_path = publish_peer(root.path(), Some("idle"));
    let listener = tokio::net::UnixListener::bind(&socket_path).expect("peer listener");
    let receiver = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accepted peer");
        let mut lines = BufReader::new(stream).lines();
        let _auth: Value =
            serde_json::from_str(&lines.next_line().await.expect("auth line").expect("auth"))
                .expect("auth JSON");
        serde_json::from_str::<Value>(&lines.next_line().await.expect("user line").expect("user"))
            .expect("user JSON")
    });
    let route = ClaudeCodePeerDeliveryRoute::new(
        target().endpoint,
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
    assert_eq!(steered.outcome, DeliveryOutcome::PeerMessageWritten);
    assert_eq!(receiver.await.expect("receiver")["type"], "user");
    assert_eq!(evidence.0.lock().await.len(), 2);
}

#[tokio::test]
async fn live_unknown_peer_protocol_reports_live_elsewhere() {
    let root = tempfile::tempdir().expect("registry root");
    let socket_path = publish_peer(root.path(), Some("idle"));
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
        target().endpoint,
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
        RouteClaim::LiveElsewhere {
            writable: false,
            detail: Some(reason),
        } if reason.contains("protocol 99")
    ));
    assert!(matches!(
        route.presence(&target()).await.expect("unsupported peer presence"),
        RoutePresence::LiveElsewhere { detail: Some(reason) } if reason.contains("protocol 99")
    ));
    assert!(
        matches!(receipt.outcome, DeliveryOutcome::Rejected(rejection)
        if rejection.reason == DeliveryRejectionReason::LiveElsewhere)
    );
    assert!(evidence.0.lock().await.is_empty());
}

#[tokio::test]
async fn ambiguous_live_peer_claims_are_preserved_in_the_rejection() {
    let root = tempfile::tempdir().expect("registry root");
    let first_process_id = std::process::id();
    let second_process_id = std::os::unix::process::parent_id();
    assert_ne!(first_process_id, second_process_id);
    publish_peer_claim(
        root.path(),
        first_process_id,
        Some("first-live-terminal"),
        Some("/workspace/first"),
    );
    publish_peer_claim(root.path(), second_process_id, None, None);

    let route = Arc::new(ClaudeCodePeerDeliveryRoute::new(
        target().endpoint,
        Arc::new(ClaudeCodeSessionRegistry::new(root.path().to_owned())),
        Arc::new(ClaudeCodePeerSocket::new(root.path().to_owned())),
    ));
    let router = SessionDeliveryRouter::new(vec![route]);
    let evidence = RecordedPeerEvidence(tokio::sync::Mutex::new(Vec::new()));
    let receipt = router
        .deliver(delivery(MessageDelivery::Auto), &evidence)
        .await
        .expect("ambiguous route rejection");

    let DeliveryOutcome::Rejected(rejection) = receipt.outcome else {
        panic!(
            "ambiguous live records must be rejected: {:?}",
            receipt.outcome
        );
    };
    assert_eq!(
        receipt.reachability,
        Some(SessionReachability::ClaudeCodePeer)
    );
    assert_eq!(rejection.reason, DeliveryRejectionReason::LiveElsewhere);
    let claims = serde_json::to_value(&rejection)
        .expect("rejection encodes")
        .get("claims")
        .cloned()
        .expect("structured peer claims");
    let claims = claims.as_array().expect("claims array");
    assert_eq!(claims.len(), 2);
    assert!(claims.contains(&json!({
        "pid": first_process_id,
        "name": "first-live-terminal",
        "cwd": "/workspace/first"
    })));
    assert!(claims.contains(&json!({
        "pid": second_process_id,
        "name": null,
        "cwd": null
    })));
    assert!(evidence.0.lock().await.is_empty());
}

#[tokio::test]
async fn every_live_registry_status_allows_auto_and_steer_peer_writes() {
    for status in [
        Some("busy"),
        Some("idle"),
        Some("waiting"),
        Some("shell"),
        Some("compacting"),
        None,
    ] {
        for mode in [MessageDelivery::Auto, MessageDelivery::Steer] {
            let root = tempfile::tempdir().expect("registry root");
            let socket_path = publish_peer(root.path(), status);
            let listener = tokio::net::UnixListener::bind(&socket_path).expect("peer listener");
            let receiver = tokio::spawn(async move {
                let (stream, _) = listener.accept().await.expect("accepted peer");
                let mut lines = BufReader::new(stream).lines();
                let _auth: Value = serde_json::from_str(
                    &lines.next_line().await.expect("auth line").expect("auth"),
                )
                .expect("auth JSON");
                serde_json::from_str::<Value>(
                    &lines.next_line().await.expect("user line").expect("user"),
                )
                .expect("user JSON")
            });
            let route = ClaudeCodePeerDeliveryRoute::new(
                target().endpoint,
                Arc::new(ClaudeCodeSessionRegistry::new(root.path().to_owned())),
                Arc::new(ClaudeCodePeerSocket::new(root.path().to_owned())),
            );
            let evidence = RecordedPeerEvidence(tokio::sync::Mutex::new(Vec::new()));

            assert_eq!(
                route.presence(&target()).await.expect("peer presence"),
                RoutePresence::Running,
                "status={status:?}"
            );

            let outcome = route
                .deliver(delivery(mode), &evidence)
                .await
                .expect("peer delivery");

            assert_eq!(
                outcome.outcome,
                DeliveryOutcome::PeerMessageWritten,
                "status={status:?}, mode={mode:?}"
            );
            let message = receiver.await.expect("peer receives write");
            assert_eq!(message["type"], "user");
            assert_eq!(
                evidence.0.lock().await.len(),
                2,
                "status={status:?}, mode={mode:?}"
            );
        }
    }
}

#[tokio::test]
async fn peer_reconciliation_rejects_evidence_for_another_session() {
    let root = tempfile::tempdir().expect("registry root");
    let route = ClaudeCodePeerDeliveryRoute::new(
        target().endpoint,
        Arc::new(ClaudeCodeSessionRegistry::new(root.path().to_owned())),
        Arc::new(ClaudeCodePeerSocket::new(root.path().to_owned())),
    );
    assert_eq!(
        route
            .presence(&target())
            .await
            .expect("absent peer presence"),
        RoutePresence::NotMine
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
            prepared_push_id: PushId::try_from("018f47d2-24d5-7a68-b9ec-6f759c39458f".to_owned())
                .expect("UUIDv7 push id"),
            mode: MessageDelivery::Auto,
            recorded,
        })
        .await;

    assert!(outcome.is_err());
}

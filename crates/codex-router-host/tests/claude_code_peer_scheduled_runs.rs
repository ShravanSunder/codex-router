#![allow(clippy::expect_used, clippy::panic)]
//! A peer scheduled run is a write-only acceptance through the real socket client.
use agent_automation::{CapturedRunInputs, RouteEffectEvidence, RunId, RunPhase};
use claude_code_peer_messaging::{ClaudeCodePeerSocket, ClaudeCodeSessionRegistry};
use codex_router_host::ClaudeCodePeerDeliveryRoute;
use collaboration_protocol::{
    CodexGeneration, DeliveryOutcome, EndpointId, EndpointRef, MessageText, RunExecution,
    SessionId, SessionRef, UuidIdentity,
};
use collaboration_service::{
    DeliveryFuture, DeliveryPrecondition, RunEvidenceDisposition, RunEvidenceSink,
    RunObservationContext, RunReconciliation, RunSettlement, RunSubmission, ScheduleCapability,
    ScheduleDestination, ScheduleSupport, ScheduledRunExecution, ScheduledRunSubmission,
    SessionDeliveryRoute, SessionDeliveryRouter, SettlementEvidence, StopRequestOutcome,
};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use std::{os::unix::fs::PermissionsExt as _, sync::Arc};
use tokio::io::{AsyncBufReadExt as _, BufReader};

struct RecordedRunEvidence(
    tokio::sync::Mutex<Vec<RouteEffectEvidence<SessionRef, CodexGeneration>>>,
);

impl RunEvidenceSink for RecordedRunEvidence {
    fn record(
        &self,
        evidence: RouteEffectEvidence<SessionRef, CodexGeneration>,
    ) -> DeliveryFuture<'_, RunEvidenceDisposition> {
        Box::pin(async move {
            self.0.lock().await.push(evidence);
            Ok(RunEvidenceDisposition::Recorded { timing: None })
        })
    }

    fn record_stop_intent(&self) -> DeliveryFuture<'_, RunEvidenceDisposition> {
        Box::pin(async { Ok(RunEvidenceDisposition::AdmissionRefused) })
    }
}

#[tokio::test]
async fn existing_live_peer_run_finishes_as_written_without_summary() {
    let root = tempfile::tempdir().expect("registry root");
    let session_id = SessionId::try_from("peer-run".to_owned()).expect("session ID");
    let socket_path = root.path().join("peer.sock");
    let process_id = std::process::id();
    std::fs::write(
        root.path().join(format!("{process_id}.json")),
        json!({
            "pid": process_id,
            "sessionId": String::from(session_id.clone()),
            "status": "idle",
            "peerProtocol": 1,
            "messagingSocketPath": socket_path,
        })
        .to_string(),
    )
    .expect("registry record");
    let digest = Sha256::digest(socket_path.to_str().expect("socket path").as_bytes());
    let key = root.path().join(format!("{process_id}.{digest:x}.key"));
    std::fs::write(
        &key,
        json!({"peerToken":"0123456789abcdef0123456789abcdef"}).to_string(),
    )
    .expect("peer key");
    std::fs::set_permissions(key, std::fs::Permissions::from_mode(0o600)).expect("private key");
    let listener = tokio::net::UnixListener::bind(&socket_path).expect("peer listener");
    let target = SessionRef {
        endpoint: EndpointRef {
            service_id: UuidIdentity::try_from("0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89".to_owned())
                .expect("service ID"),
            endpoint_id: EndpointId::try_from("claude-local".to_owned()).expect("endpoint ID"),
        },
        session_id,
    };
    let route = ClaudeCodePeerDeliveryRoute::new(
        target.endpoint.service_id.clone(),
        Arc::new(ClaudeCodeSessionRegistry::new(root.path().to_owned())),
        Arc::new(ClaudeCodePeerSocket::new(root.path().to_owned())),
    );
    let support = route
        .support(&ScheduleDestination::Existing {
            target: target.clone(),
        })
        .await
        .expect("support");
    assert!(matches!(
        support,
        ScheduleSupport::Supported {
            settlement: SettlementEvidence::WriteOnly
        }
    ));
    assert!(matches!(
        route
            .support(&ScheduleDestination::Fresh {
                endpoint: target.endpoint.clone()
            })
            .await
            .expect("fresh support"),
        ScheduleSupport::Unsupported { missing }
        if missing.contains(&ScheduleCapability::CreateSession)
    ));
    let router = SessionDeliveryRouter::new(vec![Arc::new(route) as Arc<dyn SessionDeliveryRoute>]);
    let initial = router
        .initial_evidence(&ScheduleDestination::Existing {
            target: target.clone(),
        })
        .await
        .expect("initial peer evidence");
    let sink = Arc::new(RecordedRunEvidence(tokio::sync::Mutex::new(Vec::new())));
    let prepared = router
        .prepare_existing_target(&target, sink.as_ref())
        .await
        .expect("existing peer prepared");
    assert_eq!(prepared.target, target);
    let server_sink = Arc::clone(&sink);
    let receiver = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("peer accepted");
        assert!(
            matches!(server_sink.0.lock().await.get(1),
            Some(RouteEffectEvidence::ClaudeCodePeer(peer))
            if peer.write == agent_automation::PeerWriteEffect::Dispatching),
            "dispatch intent before peer I/O"
        );
        let mut lines = BufReader::new(stream).lines();
        let _auth = lines.next_line().await.expect("auth frame").expect("auth");
        let user: Value =
            serde_json::from_str(&lines.next_line().await.expect("user frame").expect("user"))
                .expect("user JSON");
        user
    });
    let inputs: CapturedRunInputs<SessionRef, EndpointRef> = serde_json::from_value(json!({
        "scheduleChangeId":agent_automation::ChangeId::generate(),
        "instructionRevisionId":agent_automation::RevisionId::generate(),
        "instructionText":"Check work",
        "continuity":{"kind":"none"},
        "executionConfiguration":{
            "destination":{"kind":"ownedThread","target":target,"cwd":"/tmp"},
            "executionTimeoutSeconds":120,"model":null,"effort":null
        }
    }))
    .expect("captured inputs");
    let run_id = RunId::generate();
    let submitted = router
        .submit_run(
            ScheduledRunSubmission {
                run_id: run_id.clone(),
                target: target.clone(),
                message: MessageText::try_from("scheduled peer input".to_owned()).expect("input"),
                precondition: DeliveryPrecondition::Unpinned,
                inputs: inputs.clone(),
                recorded: initial,
            },
            sink.as_ref(),
        )
        .await
        .expect("peer run submission");
    assert!(matches!(submitted, RunSubmission::Started(acceptance)
        if matches!(acceptance.execution, RunExecution::ClaudeCodePeer { .. })
            && acceptance.receipt.outcome == DeliveryOutcome::PeerMessageWritten));
    let user = receiver.await.expect("peer receiver");
    assert!(
        user["message"]["content"]
            .as_str()
            .expect("content")
            .contains("scheduled peer input")
    );
    let recorded = sink
        .0
        .lock()
        .await
        .last()
        .expect("written evidence")
        .clone();
    assert!(matches!(recorded,
        RouteEffectEvidence::ClaudeCodePeer(ref peer)
        if peer.write == agent_automation::PeerWriteEffect::Written));
    let context = RunObservationContext {
        run_id,
        phase: RunPhase::Executing,
        recorded,
        inputs,
    };
    let stop_context = context.clone();
    assert!(matches!(
        router
            .observe_settlement(context.clone())
            .await
            .expect("observed"),
        RunSettlement::WrittenWithoutCompletion
    ));
    assert!(matches!(
        router.reconcile_run(context).await.expect("reconciled"),
        RunReconciliation::Settled {
            settlement: RunSettlement::WrittenWithoutCompletion
        }
    ));
    assert!(matches!(
        router
            .request_stop(stop_context, sink.as_ref())
            .await
            .expect("stop outcome"),
        StopRequestOutcome::Unsupported
    ));
}

#![allow(clippy::expect_used, clippy::panic)]
//! A peer scheduled run is a write-only acceptance through the real socket client.
use agent_automation::{
    CapturedRunInputs, ClaudeCodePeerEffectEvidence, PeerWriteEffect, RouteEffectEvidence, RunId,
    RunPhase,
};
use claude_code_peer_messaging::{ClaudeCodePeerSocket, ClaudeCodeSessionRegistry};
use codex_router_host::{
    ClaudeCodePeerDeliveryRoute, CollaborationRuntime, CollaborationRuntimeInputs,
};
use collaboration_client::ControlClient;
use collaboration_protocol::{
    AutomationConfigureRequest, CodexGeneration, DeliveryOutcome, EndpointId, EndpointRef,
    InstructionCreateParams, InstructionText, MachineId, MachineLabel, MessageText, OperationId,
    PushHeaderFacts, PushId, PushKind, PushLineInput, PushOrigin, RouterLink, RunExecution,
    RunShowRequest, RunState, ScheduleCreateRequest, ScheduleEnableRequest, SchedulePrepareRequest,
    SessionId, SessionRef, UuidIdentity, WorkerOutcome, render_push_line,
};
use collaboration_service::{
    DeliveryContractError, DeliveryFuture, DeliveryPrecondition, LoadPolicy,
    RunEvidenceDisposition, RunEvidenceSink, RunObservationContext, RunReconciliation,
    RunSettlement, RunSubmission, ScheduleCapability, ScheduleDestination, ScheduleSupport,
    ScheduledRunExecution, ScheduledRunPayload, ScheduledRunSubmission, SessionDeliveryRoute,
    SessionDeliveryRouter, SettlementEvidence, StopRequestOutcome, layer_zero::PreparedPush,
};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use sqlx::Connection as _;
use std::{os::unix::fs::PermissionsExt as _, path::Path, sync::Arc};
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

fn publish_peer(root: &Path, session_id: &SessionId, socket_path: &Path) {
    let process_id = std::process::id();
    std::fs::write(
        root.join(format!("{process_id}.json")),
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
    let key = root.join(format!("{process_id}.{digest:x}.key"));
    std::fs::write(
        &key,
        json!({"peerToken":"0123456789abcdef0123456789abcdef"}).to_string(),
    )
    .expect("peer key");
    std::fs::set_permissions(key, std::fs::Permissions::from_mode(0o600)).expect("private key");
}

#[tokio::test]
async fn existing_live_peer_run_finishes_as_written_without_summary() {
    let root = tempfile::tempdir().expect("registry root");
    let session_id = SessionId::try_from("peer-run".to_owned()).expect("session ID");
    let socket_path = root.path().join("peer.sock");
    publish_peer(root.path(), &session_id, &socket_path);
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
        target.endpoint.clone(),
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
    assert!(matches!(
        route
            .support(&ScheduleDestination::Fork {
                source: target.clone(),
                through_turn_id: "turn-1".to_owned().try_into().expect("turn ID"),
            })
            .await
            .expect("fork support"),
        ScheduleSupport::Unsupported { missing }
        if missing.contains(&ScheduleCapability::ForkSession)
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
        .prepare_existing_target(&target, "", sink.as_ref())
        .await
        .expect("existing peer prepared");
    assert_eq!(prepared.target, target);
    let run_id = RunId::generate();
    let schedule_id = agent_automation::ScheduleId::generate();
    let push_id = PushId::try_from(uuid::Uuid::now_v7().to_string()).expect("push id");
    let prepared_line = render_push_line(&PushLineInput {
        link: RouterLink::new(
            MachineId::from(target.endpoint.service_id.clone()),
            push_id.clone(),
        ),
        machine_label: MachineLabel::try_from("schedule-fixture".to_owned())
            .expect("machine label"),
        origin: PushOrigin::Router(PushKind::ScheduleRun),
        header_facts: PushHeaderFacts::ScheduleRun {
            schedule_id,
            run_id: run_id.clone(),
        },
        body: Some("Check work".to_owned()),
    })
    .expect("prepared schedule line");
    let prepared_link = prepared_line
        .split_whitespace()
        .last()
        .and_then(|link| RouterLink::parse(link).ok())
        .expect("schedule link");
    assert_eq!(prepared_link.push_id(), &push_id);
    let expected_peer_line = prepared_line.clone();
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
        assert_eq!(
            user.pointer("/message/content").and_then(Value::as_str),
            Some(expected_peer_line.as_str())
        );
        user
    });
    let inputs: CapturedRunInputs<SessionRef, EndpointRef> = serde_json::from_value(json!({
        "scheduleChangeId":agent_automation::ChangeId::generate(),
        "instructionRevisionId":agent_automation::RevisionId::generate(),
        "instructionText":"Check work",
        "continuity":{"kind":"none"},
            "executionConfiguration":{
            "destination":{"kind":"ownedThread","target":target.clone(),"cwd":"/tmp"},
            "executionTimeoutSeconds":120,"model":"fixture-model","effort":"medium"
        }
    }))
    .expect("captured inputs");
    let submitted = router
        .submit_run(
            ScheduledRunSubmission {
                run_id: run_id.clone(),
                target: target.clone(),
                payload: ScheduledRunPayload::Existing {
                    prepared: PreparedPush {
                        push_id: push_id.clone(),
                        line: MessageText::try_from(prepared_line.clone()).expect("input"),
                        load_policy: LoadPolicy::MayLoad,
                    },
                },
                precondition: DeliveryPrecondition::Unpinned,
                inputs: inputs.clone(),
                recorded: initial,
            },
            sink.as_ref(),
        )
        .await
        .expect("peer run submission");
    assert!(matches!(submitted, RunSubmission::Started(acceptance)
        if matches!(acceptance.execution, RunExecution::ClaudeCodePeer { target: ref actual_target, .. }
            if actual_target == &target)
            && acceptance.receipt.outcome == DeliveryOutcome::PeerMessageWritten));
    let user = receiver.await.expect("peer receiver");
    assert_eq!(
        user.pointer("/message/content").and_then(Value::as_str),
        Some(prepared_line.as_str())
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

#[tokio::test]
async fn scheduled_peer_rejects_a_prepared_push_link_that_disagrees_with_its_id() {
    let root = tempfile::tempdir().expect("registry root");
    let target = SessionRef {
        endpoint: EndpointRef {
            service_id: UuidIdentity::try_from("0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89".to_owned())
                .expect("service ID"),
            endpoint_id: EndpointId::try_from("claude-local".to_owned()).expect("endpoint ID"),
        },
        session_id: SessionId::try_from("peer-mismatch".to_owned()).expect("session ID"),
    };
    let route = ClaudeCodePeerDeliveryRoute::new(
        target.endpoint.clone(),
        Arc::new(ClaudeCodeSessionRegistry::new(root.path().to_owned())),
        Arc::new(ClaudeCodePeerSocket::new(root.path().to_owned())),
    );
    let run_id = RunId::generate();
    let schedule_id = agent_automation::ScheduleId::generate();
    let push_id = PushId::try_from(uuid::Uuid::now_v7().to_string()).expect("push id");
    let linked_push_id =
        PushId::try_from(uuid::Uuid::now_v7().to_string()).expect("linked push id");
    assert_ne!(push_id, linked_push_id);
    let line = render_push_line(&PushLineInput {
        link: RouterLink::new(
            MachineId::from(target.endpoint.service_id.clone()),
            linked_push_id,
        ),
        machine_label: MachineLabel::try_from("schedule-fixture".to_owned())
            .expect("machine label"),
        origin: PushOrigin::Router(PushKind::ScheduleRun),
        header_facts: PushHeaderFacts::ScheduleRun {
            schedule_id,
            run_id: run_id.clone(),
        },
        body: Some("Inspect work".to_owned()),
    })
    .expect("prepared schedule line");
    let inputs: CapturedRunInputs<SessionRef, EndpointRef> = serde_json::from_value(json!({
        "scheduleChangeId":agent_automation::ChangeId::generate(),
        "instructionRevisionId":agent_automation::RevisionId::generate(),
        "instructionText":"Inspect work",
        "continuity":{"kind":"none"},
        "executionConfiguration":{
            "destination":{"kind":"ownedThread","target":target.clone(),"cwd":"/tmp"},
            "executionTimeoutSeconds":120,"model":"fixture-model","effort":"medium"
        }
    }))
    .expect("captured inputs");
    let recorded = RouteEffectEvidence::ClaudeCodePeer(ClaudeCodePeerEffectEvidence {
        session_id: String::from(target.session_id.clone())
            .try_into()
            .expect("peer session ID"),
        process_id: 42_u32.try_into().expect("peer process ID"),
        write: PeerWriteEffect::NotDispatched,
    });
    let sink = RecordedRunEvidence(tokio::sync::Mutex::new(Vec::new()));
    let result = route
        .submit_run(
            ScheduledRunSubmission {
                run_id,
                target,
                payload: ScheduledRunPayload::Existing {
                    prepared: PreparedPush {
                        push_id,
                        line: line.try_into().expect("prepared message text"),
                        load_policy: LoadPolicy::MayLoad,
                    },
                },
                precondition: DeliveryPrecondition::Unpinned,
                inputs,
                recorded,
            },
            &sink,
        )
        .await;
    assert!(matches!(
        result,
        Err(DeliveryContractError::InvalidEvidence)
    ));
    assert!(
        sink.0.lock().await.is_empty(),
        "mismatched link crossed the route"
    );
}

#[tokio::test]
async fn host_worker_finalizes_peer_run_as_written() {
    let root = tempfile::tempdir().expect("Host root");
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))
        .expect("private Host root");
    let registry = root.path().join("peer-registry");
    std::fs::create_dir(&registry).expect("registry directory");
    let session_id = SessionId::try_from("scheduled-peer-host".to_owned()).expect("session ID");
    let socket_path = registry.join("peer.sock");
    publish_peer(&registry, &session_id, &socket_path);
    let listener = tokio::net::UnixListener::bind(&socket_path).expect("peer listener");
    let receiver = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("peer accepted");
        let mut lines = BufReader::new(stream).lines();
        let _auth = lines.next_line().await.expect("auth frame").expect("auth");
        let user: Value =
            serde_json::from_str(&lines.next_line().await.expect("user frame").expect("user"))
                .expect("user JSON");
        user
    });
    let runtime = CollaborationRuntime::start(CollaborationRuntimeInputs {
        directory: root.path().to_owned(),
        codex_home: root.path().to_owned(),
        backend_socket: root.path().join("absent-native.sock"),
        mcp_bind: std::net::SocketAddr::from(([127, 0, 0, 1], 0)),
        native_schema: None,
        peer_registry_directory: Some(registry),
        remote_control_server_name: None,
        owner_human_id: None,
    })
    .await
    .expect("Host runtime");
    let target = SessionRef {
        endpoint: EndpointRef {
            service_id: runtime.service_id().clone(),
            endpoint_id: EndpointId::try_from("claude-local".to_owned()).expect("endpoint ID"),
        },
        session_id,
    };
    let mut client = ControlClient::connect(root.path(), "peer-run-host-proof", "1")
        .await
        .expect("Control client");
    client
        .configure_automation(AutomationConfigureRequest {
            operation_id: OperationId::generate(),
            execution_timeout_seconds: 120.try_into().expect("execution timeout"),
            summary_timeout_seconds: 30.try_into().expect("summary timeout"),
        })
        .await
        .expect("automation configured");
    let instruction = client
        .create_instruction(InstructionCreateParams {
            operation_id: OperationId::generate(),
            text: InstructionText::try_from("Check peer work".to_owned()).expect("instruction"),
        })
        .await
        .expect("instruction created");
    let schedule: ScheduleCreateRequest = serde_json::from_value(json!({
        "operationId":OperationId::generate(),
        "definition":{
            "instructionId":instruction.instruction_id,
            "timing":{"kind":"after","seconds":1},
            "enabled":false,
            "destination":{"kind":"unprepared"},
            "executionTimeoutSeconds":120,"model":"fixture-model","effort":"medium"
        }
    }))
    .expect("schedule request");
    let created = client
        .create_schedule(schedule)
        .await
        .expect("schedule created");
    client
        .prepare_schedule(SchedulePrepareRequest {
            operation_id: OperationId::generate(),
            schedule_id: created.schedule_id.clone(),
            destination: collaboration_protocol::DestinationPreparation::Existing {
                target: target.clone(),
                cwd: root.path().display().to_string(),
            },
        })
        .await
        .expect("peer schedule prepared");
    let created = client
        .enable_schedule(ScheduleEnableRequest {
            operation_id: OperationId::generate(),
            schedule_id: created.schedule_id,
        })
        .await
        .expect("peer schedule enabled");
    let mut database = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new().filename(root.path().join("automation.sqlite")),
    )
    .await
    .expect("automation database");
    let run_id = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let found: Option<String> =
                sqlx::query_scalar("SELECT run_id FROM workflow_runs WHERE schedule_id=? LIMIT 1")
                    .bind(created.schedule_id.as_str())
                    .fetch_optional(&mut database)
                    .await
                    .expect("run lookup");
            if let Some(found) = found {
                break RunId::try_from(found).expect("run ID");
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("run admission deadline");
    let finished = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let snapshot = client
                .read_run(RunShowRequest {
                    run_id: run_id.clone(),
                })
                .await
                .expect("run inspection");
            if matches!(snapshot.state, RunState::Finished { .. }) {
                break snapshot;
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    let finished = match finished {
        Ok(finished) => finished,
        Err(_) => {
            let snapshot = client
                .read_run(RunShowRequest { run_id })
                .await
                .expect("timed-out run inspection");
            panic!("peer run finish deadline: {snapshot:?}");
        }
    };
    assert!(matches!(
        finished.state,
        RunState::Finished {
            execution: RunExecution::ClaudeCodePeer { .. },
            outcome: WorkerOutcome::PeerMessageWritten { .. },
            summary_run_id: None,
            ..
        }
    ));
    assert_eq!(
        finished
            .execution_evidence
            .acceptance
            .expect("acceptance receipt")
            .outcome,
        DeliveryOutcome::PeerMessageWritten
    );
    assert!(finished.summary.is_none());
    assert!(
        receiver.await.expect("peer receiver")["message"]["content"]
            .as_str()
            .expect("peer content")
            .contains("Check peer work")
    );
    client.close().await.expect("Control close");
    runtime.shutdown().await.expect("Host shutdown");
}

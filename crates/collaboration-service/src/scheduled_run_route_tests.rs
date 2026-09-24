//! A fake execution route drives provider and peer runs through stored Control inspection.
use super::*;
use crate::{
    DeliveryFuture, FreshSessionRequest, PreparationEvidenceSink, PreparedTarget, RunAcceptance,
    RunEvidenceDisposition, RunEvidenceSink, RunReconciliation, RunSettlement, RunSubmission,
    RunSummarySource, ScheduleCapability, ScheduleDestination, SchedulePreparationFailure,
    SchedulePreparationOutcome, SchedulePreparationRequest, ScheduleSupport,
    ScheduledRunSubmission, SettlementEvidence, StopRequestOutcome,
};
use agent_automation::{
    AttemptId, ClaudeCodePeerEffectEvidence, ContinuityInput, InstructionText, OperationId,
    PeerWriteEffect, ProviderAcpEffectEvidence, ProviderBindingReference, ProviderSettlementEffect,
    ScheduleDefinition, SubmissionEffect, TimingRule,
};
use automation_storage::ScheduleCreate;
use collaboration_client::ControlClient;
use collaboration_protocol::{
    DeliveryClientReceipt, DeliveryNextAction, DeliveryOutcome, DeliveryReceipt, DeliveryRejection,
    DeliveryRejectionReason, DeliveryRouteEvidence, RunShowRequest, SessionReachability,
};
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};

type TestResult<TValue> = Result<TValue, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Clone, Copy)]
enum FakeRouteKind {
    Provider,
    Peer,
}

#[derive(Clone, Copy)]
enum FakeSubmissionPlan {
    Accept,
    Reject,
    Unknown,
}

#[derive(Clone, Copy)]
enum FakeSettlementPlan {
    Completed,
    Failed,
    Interrupted,
    Pending,
    WrittenWithoutCompletion,
}

struct FakeScheduledExecution {
    kind: FakeRouteKind,
    target: SessionRef,
    generation: CodexGeneration,
    attempt_id: AttemptId,
    submissions: AtomicUsize,
    submission_plan: FakeSubmissionPlan,
    settlement_plan: FakeSettlementPlan,
}

impl FakeScheduledExecution {
    fn prepared_evidence(&self) -> RouteEffectEvidence<SessionRef, CodexGeneration> {
        match self.kind {
            FakeRouteKind::Provider => {
                RouteEffectEvidence::ProviderAcp(ProviderAcpEffectEvidence {
                    target: self.target.clone(),
                    generation: self.generation.clone(),
                    binding: ProviderBindingReference::try_from("fixture-binding".to_owned())
                        .unwrap_or_else(|error| panic!("binding: {error}")),
                    attempt_id: self.attempt_id.clone(),
                    submission: SubmissionEffect::NotDispatched,
                    settlement: ProviderSettlementEffect::NotObserved,
                })
            }
            FakeRouteKind::Peer => {
                RouteEffectEvidence::ClaudeCodePeer(ClaudeCodePeerEffectEvidence {
                    session_id: String::from(self.target.session_id.clone())
                        .try_into()
                        .unwrap_or_else(|error| panic!("peer session: {error}")),
                    process_id: 42_u32
                        .try_into()
                        .unwrap_or_else(|error| panic!("process: {error}")),
                    write: PeerWriteEffect::NotDispatched,
                })
            }
        }
    }
}

impl ScheduledRunExecution for FakeScheduledExecution {
    fn prepare_destination<'a>(
        &'a self,
        request: SchedulePreparationRequest,
        sink: &'a dyn PreparationEvidenceSink,
    ) -> DeliveryFuture<'a, SchedulePreparationOutcome> {
        Box::pin(async move {
            let mut evidence = self.prepared_evidence();
            if matches!(
                request.destination,
                collaboration_protocol::DestinationPreparation::Fork { .. }
            ) {
                return Ok(SchedulePreparationOutcome::Failed(
                    SchedulePreparationFailure {
                        kind: collaboration_protocol::ScheduleFailureKind::UnsupportedCapability,
                        explanation: "fork is Codex-only".into(),
                        evidence,
                        uncertain: false,
                    },
                ));
            }
            if let RouteEffectEvidence::ProviderAcp(provider) = &mut evidence {
                provider.submission = SubmissionEffect::NotDispatched;
            }
            if !sink.record_intent(evidence.clone()).await? {
                return Err(DeliveryContractError::EvidencePersistence);
            }
            Ok(SchedulePreparationOutcome::Prepared(PreparedTarget {
                target: self.target.clone(),
                evidence,
            }))
        })
    }

    fn initial_evidence(
        &self,
        _: &ScheduleDestination,
    ) -> DeliveryFuture<'_, RouteEffectEvidence<SessionRef, CodexGeneration>> {
        Box::pin(async move {
            let mut evidence = self.prepared_evidence();
            match &mut evidence {
                RouteEffectEvidence::ProviderAcp(provider) => {
                    provider.submission = SubmissionEffect::NotDispatched;
                }
                RouteEffectEvidence::ClaudeCodePeer(peer) => {
                    peer.write = PeerWriteEffect::NotDispatched;
                }
                RouteEffectEvidence::CodexAppServer(_) => {}
            }
            Ok(evidence)
        })
    }

    fn support(&self, destination: &ScheduleDestination) -> DeliveryFuture<'_, ScheduleSupport> {
        let destination = destination.clone();
        Box::pin(async move {
            if matches!(destination, ScheduleDestination::Fresh { .. }) {
                return Ok(ScheduleSupport::Unsupported {
                    missing: vec![ScheduleCapability::CreateSession],
                });
            }
            if matches!(destination, ScheduleDestination::Fork { .. }) {
                return Ok(ScheduleSupport::Unsupported {
                    missing: vec![ScheduleCapability::ForkSession],
                });
            }
            Ok(ScheduleSupport::Supported {
                settlement: match self.kind {
                    FakeRouteKind::Provider => SettlementEvidence::OperationSettlement,
                    FakeRouteKind::Peer => SettlementEvidence::WriteOnly,
                },
            })
        })
    }

    fn prepare_existing_target<'a>(
        &'a self,
        target: &SessionRef,
        sink: &'a dyn RunEvidenceSink,
    ) -> DeliveryFuture<'a, PreparedTarget> {
        let target = target.clone();
        Box::pin(async move {
            if target != self.target {
                return Err(DeliveryContractError::InvalidEvidence);
            }
            let evidence = self.prepared_evidence();
            if matches!(
                sink.record(evidence.clone()).await?,
                RunEvidenceDisposition::AdmissionRefused
            ) {
                return Err(DeliveryContractError::EvidencePersistence);
            }
            Ok(PreparedTarget { target, evidence })
        })
    }

    fn prepare_fresh_session<'a>(
        &'a self,
        _: FreshSessionRequest,
        sink: &'a dyn RunEvidenceSink,
    ) -> DeliveryFuture<'a, PreparedTarget> {
        self.prepare_existing_target(&self.target, sink)
    }

    fn submit_run<'a>(
        &'a self,
        run: ScheduledRunSubmission,
        sink: &'a dyn RunEvidenceSink,
    ) -> DeliveryFuture<'a, RunSubmission> {
        Box::pin(async move {
            if run.target != self.target {
                return Err(DeliveryContractError::InvalidEvidence);
            }
            if self.submissions.fetch_add(1, Ordering::SeqCst) == 0 {
                return Ok(RunSubmission::NotStartedBusy);
            }
            let mut evidence = run.recorded;
            match &mut evidence {
                RouteEffectEvidence::ProviderAcp(provider) => {
                    provider.submission = SubmissionEffect::Dispatching;
                }
                RouteEffectEvidence::ClaudeCodePeer(peer) => {
                    peer.write = PeerWriteEffect::Dispatching;
                }
                RouteEffectEvidence::CodexAppServer(_) => {}
            }
            let timing = match sink.record(evidence.clone()).await? {
                RunEvidenceDisposition::Recorded {
                    timing: Some(timing),
                } => timing,
                _ => return Err(DeliveryContractError::InvalidEvidence),
            };
            match self.submission_plan {
                FakeSubmissionPlan::Reject => {
                    if let RouteEffectEvidence::ProviderAcp(provider) = &mut evidence {
                        provider.submission = SubmissionEffect::Rejected;
                    }
                    sink.record(evidence).await?;
                    return Ok(RunSubmission::Rejected(DeliveryRejection {
                        reason: DeliveryRejectionReason::Busy,
                        next_action: DeliveryNextAction::RetryLater,
                        client_code: None,
                        detail: Some("fixture rejection".into()),
                    }));
                }
                FakeSubmissionPlan::Unknown => {
                    if let RouteEffectEvidence::ProviderAcp(provider) = &mut evidence {
                        provider.submission = SubmissionEffect::Unknown;
                    }
                    sink.record(evidence).await?;
                    return Ok(RunSubmission::Unknown);
                }
                FakeSubmissionPlan::Accept => {}
            }
            let started_at = crate::wakeup_projection::timestamp(timing.dispatch_started_at_ms)
                .map_err(|_| DeliveryContractError::InvalidEvidence)?;
            let deadline_at = crate::wakeup_projection::timestamp(timing.deadline_at_ms)
                .map_err(|_| DeliveryContractError::InvalidEvidence)?;
            let timeout = timing
                .effective_timeout_seconds
                .try_into()
                .map_err(|_| DeliveryContractError::InvalidEvidence)?;
            let (execution, receipt) = match &mut evidence {
                RouteEffectEvidence::ProviderAcp(provider) => {
                    provider.submission = SubmissionEffect::Accepted;
                    let operation_id = OperationId::try_from(self.attempt_id.as_str().to_owned())
                        .map_err(|_| DeliveryContractError::InvalidEvidence)?;
                    (
                        RunExecution::ProviderAcp {
                            target: self.target.clone(),
                            operation_id: operation_id.clone(),
                            started_at,
                            deadline_at,
                            effective_timeout_seconds: timeout,
                        },
                        DeliveryReceipt {
                            outcome: DeliveryOutcome::Started,
                            reachability: Some(SessionReachability::ProviderAcp),
                            client: Some(DeliveryClientReceipt::ProviderAcp { operation_id }),
                        },
                    )
                }
                RouteEffectEvidence::ClaudeCodePeer(peer) => {
                    peer.write = PeerWriteEffect::Written;
                    (
                        RunExecution::ClaudeCodePeer {
                            target: self.target.clone(),
                            written_at: started_at,
                        },
                        DeliveryReceipt {
                            outcome: DeliveryOutcome::PeerMessageWritten,
                            reachability: Some(SessionReachability::ClaudeCodePeer),
                            client: Some(DeliveryClientReceipt::ClaudeCodePeer),
                        },
                    )
                }
                RouteEffectEvidence::CodexAppServer(_) => {
                    return Err(DeliveryContractError::InvalidEvidence);
                }
            };
            sink.record(evidence).await?;
            Ok(RunSubmission::Started(Box::new(RunAcceptance {
                execution,
                receipt,
            })))
        })
    }

    fn observe_settlement(&self, _: RunObservationContext) -> DeliveryFuture<'_, RunSettlement> {
        Box::pin(async {
            Ok(match self.settlement_plan {
                FakeSettlementPlan::Completed => RunSettlement::Completed {
                    summary_source: RunSummarySource::ProviderResponse {
                        text: "provider completed".into(),
                    },
                },
                FakeSettlementPlan::Failed => RunSettlement::Failed {
                    reason: "fixture operation failed".into(),
                },
                FakeSettlementPlan::Interrupted => RunSettlement::Interrupted,
                FakeSettlementPlan::Pending => RunSettlement::Pending,
                FakeSettlementPlan::WrittenWithoutCompletion => {
                    RunSettlement::WrittenWithoutCompletion
                }
            })
        })
    }

    fn summary_source(&self, _: RunObservationContext) -> DeliveryFuture<'_, RunSummarySource> {
        Box::pin(async {
            Ok(RunSummarySource::ProviderResponse {
                text: "provider completed".into(),
            })
        })
    }

    fn request_stop<'a>(
        &'a self,
        _: RunObservationContext,
        _: &'a dyn RunEvidenceSink,
    ) -> DeliveryFuture<'a, StopRequestOutcome> {
        Box::pin(async { Ok(StopRequestOutcome::Unsupported) })
    }

    fn reconcile_run(&self, _: RunObservationContext) -> DeliveryFuture<'_, RunReconciliation> {
        Box::pin(async { Ok(RunReconciliation::StillUnknown) })
    }
}

async fn provider_worker_fixture(
    submission_plan: FakeSubmissionPlan,
    settlement_plan: FakeSettlementPlan,
) -> TestResult<(
    std::path::PathBuf,
    Arc<Mutex<AutomationStore>>,
    ScheduledRunWorker,
    RunId,
)> {
    let root = std::env::temp_dir().join(format!(
        "provider-worker-{}",
        OperationId::generate().as_str()
    ));
    std::fs::create_dir(&root)?;
    let store = Arc::new(Mutex::new(
        AutomationStore::open(&root.join("automation.sqlite")).await?,
    ));
    let service = "00000000-0000-4000-8000-000000000001";
    let target: SessionRef = serde_json::from_value(json!({
        "endpoint":{"serviceId":service,"endpointId":"claude-local"},
        "sessionId":"provider-worker"
    }))?;
    let generation: CodexGeneration =
        serde_json::from_value(json!({"serviceEpoch":service,"generation":1}))?;
    let fake: Arc<dyn ScheduledRunExecution> = Arc::new(FakeScheduledExecution {
        kind: FakeRouteKind::Provider,
        target: target.clone(),
        generation,
        attempt_id: AttemptId::generate(),
        submissions: AtomicUsize::new(0),
        submission_plan,
        settlement_plan,
    });
    let instruction = store
        .lock()
        .await
        .create_instruction(
            &OperationId::generate(),
            &InstructionText::try_from("Check job".to_owned())?,
            0,
        )
        .await?;
    let schedule = store
        .lock()
        .await
        .create_schedule(&ScheduleCreate::<SessionRef, EndpointRef> {
            operation_id: OperationId::generate(),
            definition: ScheduleDefinition {
                instruction_id: instruction.instruction_id,
                timing: TimingRule::Interval { seconds: 60 },
                enabled: true,
                destination: ExecutionDestination::OwnedThread {
                    target,
                    cwd: root.to_string_lossy().into_owned(),
                },
                execution_timeout_seconds: Some(120),
                model: Some("fixture-model".into()),
                effort: Some("medium".into()),
            },
            imported_continuity: ContinuityInput::None,
            now_ms: 0,
        })
        .await?;
    let run_id = store
        .lock()
        .await
        .enqueue_due_run::<SessionRef, EndpointRef>(&schedule.schedule_id, 60000)
        .await?
        .ok_or("run missing")?;
    store
        .lock()
        .await
        .admit_waiting_run::<SessionRef, EndpointRef>(&schedule.schedule_id, 60000)
        .await?;
    let worker = ScheduledRunWorker {
        store: Arc::clone(&store),
        execution: fake,
        backend: None,
        configuration: crate::AutomationConfigurationHandle::default(),
    };
    Ok((root, store, worker, run_id))
}

#[tokio::test]
async fn fake_provider_submission_and_settlement_variants_preserve_run_state() -> TestResult<()> {
    let cases = [
        (
            FakeSubmissionPlan::Reject,
            FakeSettlementPlan::Pending,
            RunPhase::Preparing,
            false,
        ),
        (
            FakeSubmissionPlan::Unknown,
            FakeSettlementPlan::Pending,
            RunPhase::Uncertain,
            false,
        ),
        (
            FakeSubmissionPlan::Accept,
            FakeSettlementPlan::Failed,
            RunPhase::Finished,
            false,
        ),
        (
            FakeSubmissionPlan::Accept,
            FakeSettlementPlan::Interrupted,
            RunPhase::Finished,
            false,
        ),
        (
            FakeSubmissionPlan::Accept,
            FakeSettlementPlan::Pending,
            RunPhase::Executing,
            false,
        ),
        (
            FakeSubmissionPlan::Accept,
            FakeSettlementPlan::WrittenWithoutCompletion,
            RunPhase::Executing,
            true,
        ),
    ];
    for (submission, settlement, expected_phase, expected_error) in cases {
        let (root, store, worker, run_id) = provider_worker_fixture(submission, settlement).await?;
        worker.step(run_id.clone()).await?;
        worker.step(run_id.clone()).await?;
        let busy = store
            .lock()
            .await
            .read_run::<SessionRef, EndpointRef, CodexGeneration, crate::stored_run_receipt::StoredRunReceipt>(&run_id)
            .await?;
        if busy.phase != RunPhase::Preparing || busy.evidence.timing.is_some() {
            return Err("not-started-busy consumed a dispatch budget".into());
        }
        worker.step(run_id.clone()).await?;
        if matches!(submission, FakeSubmissionPlan::Accept) {
            let observed = worker.step(run_id.clone()).await;
            if expected_error {
                if !matches!(observed, Err(StorageError::InvalidRecord)) {
                    return Err("provider write-only settlement was not rejected".into());
                }
            } else {
                observed?;
            }
        }
        let settled = store
            .lock()
            .await
            .read_run::<SessionRef, EndpointRef, CodexGeneration, crate::stored_run_receipt::StoredRunReceipt>(&run_id)
            .await?;
        if settled.phase != expected_phase {
            return Err(format!(
                "provider route phase mismatch: expected {expected_phase:?}, got {:?}",
                settled.phase
            )
            .into());
        }
        if matches!(submission, FakeSubmissionPlan::Reject) && settled.evidence.timing.is_some() {
            return Err("known rejection retained an execution budget".into());
        }
        match (submission, settlement) {
            (FakeSubmissionPlan::Accept, FakeSettlementPlan::Failed)
                if !matches!(
                    settled.worker_outcome,
                    Some(agent_automation::WorkerOutcome::Failed { .. })
                ) =>
            {
                return Err("failed settlement lost worker outcome".into());
            }
            (FakeSubmissionPlan::Accept, FakeSettlementPlan::Interrupted)
                if !matches!(
                    settled.worker_outcome,
                    Some(agent_automation::WorkerOutcome::Interrupted { .. })
                ) =>
            {
                return Err("interrupted settlement lost worker outcome".into());
            }
            _ => {}
        }
        drop(worker);
        let store = Arc::try_unwrap(store).map_err(|_| "store still referenced")?;
        store.into_inner().close().await?;
        for entry in std::fs::read_dir(&root)? {
            std::fs::remove_file(entry?.path())?;
        }
        std::fs::remove_dir(root)?;
    }
    Ok(())
}

#[tokio::test]
async fn provider_and_peer_routes_drive_run_show_without_native_turns() -> TestResult<()> {
    for kind in [FakeRouteKind::Provider, FakeRouteKind::Peer] {
        let route_name = match kind {
            FakeRouteKind::Provider => "provider",
            FakeRouteKind::Peer => "peer",
        };
        let root = std::env::temp_dir().join(format!(
            "fake-run-route-{}",
            OperationId::generate().as_str()
        ));
        std::fs::create_dir(&root)?;
        let store = Arc::new(Mutex::new(
            AutomationStore::open(&root.join("automation.sqlite")).await?,
        ));
        let service = "00000000-0000-4000-8000-000000000001";
        let target: SessionRef = serde_json::from_value(json!({
            "endpoint":{"serviceId":service,"endpointId":"claude-local"},
            "sessionId":match kind { FakeRouteKind::Provider => "provider-worker", FakeRouteKind::Peer => "peer-worker" }
        }))?;
        let generation: CodexGeneration =
            serde_json::from_value(json!({"serviceEpoch":service,"generation":1}))?;
        let fake: Arc<dyn ScheduledRunExecution> = Arc::new(FakeScheduledExecution {
            kind,
            target: target.clone(),
            generation,
            attempt_id: AttemptId::generate(),
            submissions: AtomicUsize::new(0),
            submission_plan: FakeSubmissionPlan::Accept,
            settlement_plan: FakeSettlementPlan::Completed,
        });
        let instruction = store
            .lock()
            .await
            .create_instruction(
                &OperationId::generate(),
                &InstructionText::try_from("Check job".to_owned())?,
                0,
            )
            .await?;
        let schedule = store
            .lock()
            .await
            .create_schedule(&ScheduleCreate::<SessionRef, EndpointRef> {
                operation_id: OperationId::generate(),
                definition: ScheduleDefinition {
                    instruction_id: instruction.instruction_id,
                    timing: TimingRule::Interval { seconds: 60 },
                    enabled: true,
                    destination: ExecutionDestination::OwnedThread {
                        target: target.clone(),
                        cwd: root.to_string_lossy().into_owned(),
                    },
                    execution_timeout_seconds: Some(120),
                    model: Some("fixture-model".into()),
                    effort: Some("medium".into()),
                },
                imported_continuity: ContinuityInput::None,
                now_ms: 0,
            })
            .await?;
        let run_id = store
            .lock()
            .await
            .enqueue_due_run::<SessionRef, EndpointRef>(&schedule.schedule_id, 60000)
            .await?
            .ok_or("run missing")?;
        store
            .lock()
            .await
            .admit_waiting_run::<SessionRef, EndpointRef>(&schedule.schedule_id, 60000)
            .await?;
        let worker = ScheduledRunWorker {
            store: Arc::clone(&store),
            execution: Arc::clone(&fake),
            backend: None,
            configuration: crate::AutomationConfigurationHandle::default(),
        };
        for stage in 0..3 {
            if let Err(error) = worker.step(run_id.clone()).await {
                let current = store
                    .lock()
                    .await
                    .read_run::<SessionRef, EndpointRef, CodexGeneration, crate::stored_run_receipt::StoredRunReceipt>(&run_id)
                    .await?;
                return Err(format!(
                    "fake {route_name} stage {stage}: {error}; phase={:?}, route={:?}, timing={:?}",
                    current.phase, current.evidence.route, current.evidence.timing
                )
                .into());
            }
        }
        if matches!(kind, FakeRouteKind::Provider) {
            worker
                .step(run_id.clone())
                .await
                .map_err(|error| format!("fake provider settle: {error}"))?;
        }
        let identity =
            crate::ServiceIdentity::new(service, service, &format!("sha256:{}", "a".repeat(64)))?
                .with_automation_store(Arc::clone(&store))
                .with_scheduled_run_execution(fake);
        let (socket, server) = tokio::net::UnixStream::pair()?;
        let service_task = tokio::spawn(crate::serve_control_connection(server, identity));
        let mut client = ControlClient::initialize(socket, "fake-run-show", "1").await?;
        let shown = client.read_run(RunShowRequest { run_id }).await?;
        match (kind, shown.state) {
            (
                FakeRouteKind::Provider,
                collaboration_protocol::RunState::Finished {
                    execution: RunExecution::ProviderAcp { .. },
                    ..
                },
            )
            | (
                FakeRouteKind::Peer,
                collaboration_protocol::RunState::Finished {
                    execution: RunExecution::ClaudeCodePeer { .. },
                    ..
                },
            ) => {}
            _ => return Err("run/show lost the selected route execution".into()),
        }
        if shown.summary.is_some() || shown.execution_evidence.route.is_none() {
            return Err("run/show invented a summary or lost route evidence".into());
        }
        client.close().await?;
        service_task.await??;
        drop(worker);
        drop(store);
        for entry in std::fs::read_dir(&root)? {
            std::fs::remove_file(entry?.path())?;
        }
        std::fs::remove_dir(root)?;
    }
    Ok(())
}

#[tokio::test]
async fn provider_existing_preparation_is_inspectable_and_fresh_activation_is_rejected()
-> TestResult<()> {
    let root = std::env::temp_dir().join(format!(
        "provider-preparation-{}",
        OperationId::generate().as_str()
    ));
    std::fs::create_dir(&root)?;
    let store = Arc::new(Mutex::new(
        AutomationStore::open(&root.join("automation.sqlite")).await?,
    ));
    let service = "00000000-0000-4000-8000-000000000001";
    let target: SessionRef = serde_json::from_value(json!({
        "endpoint":{"serviceId":service,"endpointId":"claude-local"},
        "sessionId":"prepared-provider"
    }))?;
    let generation: CodexGeneration =
        serde_json::from_value(json!({"serviceEpoch":service,"generation":1}))?;
    let fake: Arc<dyn ScheduledRunExecution> = Arc::new(FakeScheduledExecution {
        kind: FakeRouteKind::Provider,
        target: target.clone(),
        generation,
        attempt_id: AttemptId::generate(),
        submissions: AtomicUsize::new(0),
        submission_plan: FakeSubmissionPlan::Accept,
        settlement_plan: FakeSettlementPlan::Completed,
    });
    let instruction = store
        .lock()
        .await
        .create_instruction(
            &OperationId::generate(),
            &InstructionText::try_from("Check job".to_owned())?,
            0,
        )
        .await?;
    let mut schedules = Vec::new();
    for _ in 0..2 {
        let schedule = store
            .lock()
            .await
            .create_schedule(&ScheduleCreate::<SessionRef, EndpointRef> {
                operation_id: OperationId::generate(),
                definition: ScheduleDefinition {
                    instruction_id: instruction.instruction_id.clone(),
                    timing: TimingRule::Interval { seconds: 60 },
                    enabled: false,
                    destination: ExecutionDestination::Unprepared,
                    execution_timeout_seconds: Some(120),
                    model: Some("fixture-model".into()),
                    effort: Some("medium".into()),
                },
                imported_continuity: ContinuityInput::None,
                now_ms: 0,
            })
            .await?;
        schedules.push(schedule.schedule_id);
    }
    let identity =
        crate::ServiceIdentity::new(service, service, &format!("sha256:{}", "a".repeat(64)))?
            .with_automation_store(Arc::clone(&store))
            .with_scheduled_run_execution(fake);
    let (socket, server) = tokio::net::UnixStream::pair()?;
    let service_task = tokio::spawn(crate::serve_control_connection(server, identity.clone()));
    let mut client = ControlClient::initialize(socket, "provider-preparation", "1").await?;
    let fresh = client
        .create_schedule(serde_json::from_value(json!({
            "operationId": OperationId::generate(),
            "definition": {
                "instructionId": instruction.instruction_id.clone(),
                "timing": {"kind": "interval", "seconds": 60},
                "enabled": true,
                "destination": {"kind": "freshEachRun", "endpoint": target.endpoint.clone(), "cwd": root},
                "executionTimeoutSeconds": 120,
                "model": "fixture-model",
                "effort": "medium"
            }
        }))?)
        .await;
    if !matches!(fresh, Err(collaboration_client::ScheduleClientError::Rejected(failure))
        if failure.field.as_deref() == Some("destination")
            && failure.constraint.as_deref().is_some_and(|fix| fix.contains("conversation create")))
    {
        return Err("fresh provider schedule activation lacked the create-first fix".into());
    }
    drop(client);
    service_task.await??;
    let (socket, server) = tokio::net::UnixStream::pair()?;
    let service_task = tokio::spawn(crate::serve_control_connection(server, identity.clone()));
    let mut client =
        ControlClient::initialize(socket, "provider-preparation-existing", "1").await?;
    let operation_id = OperationId::generate();
    let prepared = client
        .prepare_schedule(serde_json::from_value(json!({
            "operationId":operation_id,"scheduleId":schedules[0],
            "destination":{"kind":"existing","target":target.clone(),"cwd":root}
        }))?)
        .await?;
    if !matches!(prepared.definition.destination, collaboration_protocol::ExecutionDestination::OwnedThread { target: prepared_target, .. } if prepared_target == target)
    {
        return Err("provider preparation did not bind the prepared target".into());
    }
    let shown = client
        .read_operation(collaboration_protocol::OperationShowRequest { operation_id })
        .await?;
    if !matches!(
        shown.state,
        collaboration_protocol::OperationState::Succeeded { .. }
    ) {
        return Err("provider preparation did not complete in operation/show".into());
    }
    let fork_operation = OperationId::generate();
    let fork = client
        .prepare_schedule(serde_json::from_value(json!({
            "operationId":fork_operation,"scheduleId":schedules[1],
            "destination":{"kind":"fork","source":target,"throughTurnId":"turn-one","cwd":root}
        }))?)
        .await;
    if !matches!(fork, Err(collaboration_client::ScheduleClientError::Rejected(failure))
        if matches!(failure.kind, collaboration_protocol::ScheduleFailureKind::UnsupportedCapability)
            && matches!(failure.effects, collaboration_protocol::ScheduleEffects::Route { evidence: DeliveryRouteEvidence::ProviderAcp { .. } }))
    {
        return Err("provider fork did not report route-specific unsupported capability".into());
    }
    let (inspection_socket, inspection_server) = tokio::net::UnixStream::pair()?;
    let inspection_task =
        tokio::spawn(crate::serve_control_connection(inspection_server, identity));
    let mut inspection_client =
        ControlClient::initialize(inspection_socket, "provider-operation-show", "1").await?;
    let fork_shown = inspection_client
        .read_operation(collaboration_protocol::OperationShowRequest {
            operation_id: fork_operation,
        })
        .await?;
    if !matches!(fork_shown.state, collaboration_protocol::OperationState::Failed { ref error }
        if matches!(&error.effects, collaboration_protocol::OperationEffects::Route {
            evidence,
        } if matches!(**evidence, DeliveryRouteEvidence::ProviderAcp { .. })))
    {
        return Err("operation/show lost the provider preparation evidence".into());
    }
    inspection_client.close().await?;
    inspection_task.await??;
    let _ = client.close().await;
    service_task.await??;
    drop(store);
    for entry in std::fs::read_dir(&root)? {
        std::fs::remove_file(entry?.path())?;
    }
    std::fs::remove_dir(root)?;
    Ok(())
}

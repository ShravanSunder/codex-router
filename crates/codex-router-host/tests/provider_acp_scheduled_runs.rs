#![allow(clippy::expect_used, clippy::panic)]
//! A provider run waits for an idle actor and summarizes its own prompt settlement.
use agent_automation::{
    CapturedRunInputs, ExecutionTiming, RouteEffectEvidence, RunId, RunPhase, SubmissionEffect,
};
use codex_router_host::{
    ExternalProviderBinding, ExternalProviderLaunch, ExternalProviderRuntime,
    ExternalProviderSupervisor, LiveSessionOwnership, LiveSessionOwnershipCheck,
    ProviderAcpDeliveryRoute,
};
use collaboration_protocol::{
    ChannelDescription, CodexGeneration, ConversationOperationWaitOutput,
    ConversationOperationWaitRequest, ConversationPromptRequest, DeliveryOutcome,
    DestinationPreparation, EndpointAvailability, EndpointDescription, EndpointId, EndpointRef,
    GenerationNumber, MessageContent, MessageText, NonEmptyText, ObservationTimestamp, OperationId,
    PositiveSeconds, ProviderBindingId, ProviderBindingIdentity, ProviderCapabilities,
    ProviderCapability, ProviderCapabilityEvidence, ProviderCapabilityName,
    ProviderCapabilityStatus, ProviderKind, ProviderRequestedPolicy, ProviderRuntimeIdentity,
    ProviderTransport, ProviderWorkingDirectory, RouterAccess, RunExecution, SessionId, SessionRef,
    UuidIdentity,
};
use collaboration_service::{
    DeliveryContractError, DeliveryFuture, DeliveryPrecondition, EndpointDirectory,
    PreparationEvidenceSink, PreparedTarget, ProviderConversationBackend, ProviderOperationStore,
    ProviderSessionRecord, RunEvidenceDisposition, RunEvidenceSink, RunObservationContext,
    RunReconciliation, RunSettlement, RunSubmission, ScheduleCapability, ScheduleDestination,
    SchedulePreparationOutcome, SchedulePreparationRequest, ScheduleSupport, ScheduledRunExecution,
    ScheduledRunSubmission, SessionDeliveryRoute, SessionDeliveryRouter, SettlementEvidence,
};
use serde_json::json;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

struct NoLivePeer;
impl LiveSessionOwnershipCheck for NoLivePeer {
    fn check<'a>(&'a self, _target: &'a SessionRef) -> DeliveryFuture<'a, LiveSessionOwnership> {
        Box::pin(async { Ok(LiveSessionOwnership::NotLive) })
    }
}

struct RecordedRunEvidence(
    tokio::sync::Mutex<Vec<RouteEffectEvidence<SessionRef, CodexGeneration>>>,
);
impl RunEvidenceSink for RecordedRunEvidence {
    fn record(
        &self,
        evidence: RouteEffectEvidence<SessionRef, CodexGeneration>,
    ) -> DeliveryFuture<'_, RunEvidenceDisposition> {
        Box::pin(async move {
            let dispatching = matches!(&evidence,
                RouteEffectEvidence::ProviderAcp(provider)
                if provider.submission == SubmissionEffect::Dispatching);
            self.0.lock().await.push(evidence);
            let now = chrono::Utc::now().timestamp_millis();
            Ok(RunEvidenceDisposition::Recorded {
                timing: dispatching.then_some(ExecutionTiming {
                    dispatch_started_at_ms: now,
                    deadline_at_ms: now + 120_000,
                    effective_timeout_seconds: 120,
                }),
            })
        })
    }

    fn record_stop_intent(&self) -> DeliveryFuture<'_, RunEvidenceDisposition> {
        Box::pin(async { Ok(RunEvidenceDisposition::Recorded { timing: None }) })
    }
}

struct RecordedPreparationIntent(
    tokio::sync::Mutex<Vec<RouteEffectEvidence<SessionRef, CodexGeneration>>>,
);
impl PreparationEvidenceSink for RecordedPreparationIntent {
    fn record_intent(
        &self,
        evidence: RouteEffectEvidence<SessionRef, CodexGeneration>,
    ) -> DeliveryFuture<'_, bool> {
        Box::pin(async move {
            self.0.lock().await.push(evidence);
            Ok(true)
        })
    }

    fn record_prepared<'a>(
        &'a self,
        _prepared: &'a PreparedTarget,
    ) -> DeliveryFuture<'a, automation_storage::ScheduleInspection<SessionRef, EndpointRef>> {
        Box::pin(async { Err(DeliveryContractError::ClientOperation) })
    }

    fn record_failure<'a>(
        &'a self,
        _failure: &'a collaboration_protocol::ScheduleFailure,
        _evidence: RouteEffectEvidence<SessionRef, CodexGeneration>,
        _uncertain: bool,
    ) -> DeliveryFuture<'a, ()> {
        Box::pin(async { Err(DeliveryContractError::ClientOperation) })
    }
}

fn fixture_launch(event_socket: &Path) -> ExternalProviderLaunch {
    let script = format!(
        r#"
import json,socket,sys
request=json.loads(sys.stdin.readline())
print(json.dumps({{'jsonrpc':'2.0','id':request['id'],'result':{{'protocolVersion':1,'agentCapabilities':{{'loadSession':True}},'agentInfo':{{'name':'schedule-fixture','version':'1'}}}}}})); sys.stdout.flush()
request=json.loads(sys.stdin.readline())
assert request['method']=='session/new'
print(json.dumps({{'jsonrpc':'2.0','id':request['id'],'result':{{'sessionId':'scheduled-session'}}}})); sys.stdout.flush()
active=json.loads(sys.stdin.readline())
assert active['method']=='session/prompt'
with socket.socket(socket.AF_UNIX,socket.SOCK_STREAM) as event:
 event.connect({:?})
 event.sendall(b'active')
 event.recv(1)
print(json.dumps({{'jsonrpc':'2.0','id':active['id'],'result':{{'stopReason':'end_turn'}}}})); sys.stdout.flush()
scheduled=json.loads(sys.stdin.readline())
assert scheduled['method']=='session/prompt'
print(json.dumps({{'jsonrpc':'2.0','method':'session/update','params':{{'sessionId':'scheduled-session','update':{{'sessionUpdate':'agent_message_chunk','content':{{'type':'text','text':'provider summary text'}}}}}}}})); sys.stdout.flush()
print(json.dumps({{'jsonrpc':'2.0','id':scheduled['id'],'result':{{'stopReason':'end_turn'}}}})); sys.stdout.flush()
cancelled=json.loads(sys.stdin.readline())
assert cancelled['method']=='session/prompt'
cancel=json.loads(sys.stdin.readline())
assert cancel['method']=='session/cancel'
print(json.dumps({{'jsonrpc':'2.0','id':cancelled['id'],'result':{{'stopReason':'cancelled'}}}})); sys.stdout.flush()
pending=json.loads(sys.stdin.readline())
assert pending['method']=='session/prompt'
cancel=json.loads(sys.stdin.readline())
assert cancel['method']=='session/cancel'
sys.stdin.read()
"#,
        event_socket.display().to_string()
    );
    ExternalProviderLaunch {
        executable: PathBuf::from("/usr/bin/python3"),
        arguments: vec!["-c".to_owned(), script],
        environment: Vec::new(),
    }
}

#[tokio::test]
async fn busy_provider_run_starts_when_idle_and_finishes_without_summary() {
    let root = tempfile::tempdir().expect("fixture root");
    let event_socket = root.path().join("active.sock");
    let listener = tokio::net::UnixListener::bind(&event_socket).expect("active event listener");
    let service_id = UuidIdentity::try_from("0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89".to_owned())
        .expect("service ID");
    let endpoint = EndpointRef {
        service_id: service_id.clone(),
        endpoint_id: EndpointId::try_from("cursor-local".to_owned()).expect("endpoint"),
    };
    let target = SessionRef {
        endpoint: endpoint.clone(),
        session_id: SessionId::try_from("scheduled-session".to_owned()).expect("session"),
    };
    let generation = CodexGeneration {
        service_epoch: UuidIdentity::try_from("1ff962c5-7fa3-4c18-a5ca-1bbe8db09e80".to_owned())
            .expect("epoch"),
        generation: GenerationNumber::try_from(1).expect("generation"),
    };
    let runtime_identity = ProviderRuntimeIdentity {
        provider: ProviderKind::Cursor,
        runtime_name: NonEmptyText::try_from("schedule-fixture".to_owned()).expect("runtime"),
        runtime_version: None,
    };
    let capabilities = ProviderCapabilities::try_from(
        [
            ProviderCapabilityName::Create,
            ProviderCapabilityName::Prompt,
            ProviderCapabilityName::Cancel,
        ]
        .into_iter()
        .map(|name| ProviderCapability {
            name,
            status: ProviderCapabilityStatus::Supported,
            evidence: ProviderCapabilityEvidence::Advertised,
        })
        .collect::<Vec<_>>(),
    )
    .expect("capabilities");
    let binding_id =
        ProviderBindingId::try_from("schedule-binding".to_owned()).expect("binding ID");
    let binding = ProviderBindingIdentity {
        endpoint: endpoint.clone(),
        binding_id: binding_id.clone(),
        runtime: runtime_identity.clone(),
        transport: ProviderTransport::StdioAcp,
        generation: generation.clone(),
        capabilities: capabilities.clone(),
    };
    let runtime = ExternalProviderRuntime::initialize(fixture_launch(&event_socket))
        .await
        .expect("runtime");
    runtime
        .create_session(PathBuf::from("/tmp"))
        .await
        .expect("session/new");
    let store = Arc::new(tokio::sync::Mutex::new(
        ProviderOperationStore::open(&root.path().join("operations.sqlite"))
            .await
            .expect("store"),
    ));
    store
        .lock()
        .await
        .record_session(&ProviderSessionRecord {
            target: target.clone(),
            working_directory: ProviderWorkingDirectory::try_from("/tmp".to_owned()).expect("cwd"),
            requested_policy: ProviderRequestedPolicy {
                access: RouterAccess::WriteRestricted,
            },
            created_by: target.clone(),
            approver: target.clone(),
            updated_at_ms: 1,
        })
        .await
        .expect("session record");
    let supervisor = Arc::new(
        ExternalProviderSupervisor::new(
            vec![ExternalProviderBinding {
                identity: binding,
                runtime,
            }],
            Arc::clone(&store),
        )
        .expect("supervisor"),
    );
    let directory = EndpointDirectory::new(service_id.clone());
    directory
        .publish(EndpointDescription {
            endpoint: endpoint.clone(),
            label: NonEmptyText::try_from("Cursor".to_owned()).expect("label"),
            availability: EndpointAvailability::Available {
                observed_at: ObservationTimestamp::try_from("2026-09-24T12:00:00Z".to_owned())
                    .expect("timestamp"),
            },
            channels: vec![ChannelDescription::ExternalProvider {
                transport: ProviderTransport::StdioAcp,
                binding_id,
                binding_generation: generation.generation,
                runtime: runtime_identity,
                capabilities,
            }],
        })
        .expect("endpoint publication");
    let route = Arc::new(ProviderAcpDeliveryRoute::new(
        service_id.clone(),
        directory,
        Arc::clone(&supervisor),
        Arc::clone(&store),
        Arc::new(NoLivePeer),
    ));
    let router = SessionDeliveryRouter::new(vec![route.clone() as Arc<dyn SessionDeliveryRoute>]);
    let preparation = RecordedPreparationIntent(tokio::sync::Mutex::new(Vec::new()));
    let preparation_operation = OperationId::generate();
    let explicit = router
        .prepare_destination(
            SchedulePreparationRequest {
                operation_id: preparation_operation.clone(),
                schedule_id: agent_automation::ScheduleId::generate(),
                expected_change_id: agent_automation::ChangeId::generate(),
                destination: DestinationPreparation::Existing {
                    target: target.clone(),
                    cwd: "/tmp".into(),
                },
                instruction_text: "Check work".into(),
                model: None,
                effort: String::new(),
            },
            &preparation,
        )
        .await
        .expect("explicit preparation");
    assert!(matches!(explicit,
        SchedulePreparationOutcome::Prepared(prepared)
        if prepared.target == target));
    assert!(matches!(preparation.0.lock().await.as_slice(),
        [RouteEffectEvidence::ProviderAcp(provider)]
        if provider.attempt_id.as_str() == preparation_operation.as_str()));
    assert!(matches!(
        router
            .support(&ScheduleDestination::Existing {
                target: target.clone()
            })
            .await
            .expect("run support"),
        ScheduleSupport::Supported {
            settlement: SettlementEvidence::OperationSettlement
        }
    ));
    assert!(matches!(
        router
            .support(&ScheduleDestination::Fresh {
                endpoint: endpoint.clone(),
            })
            .await
            .expect("fresh support"),
        ScheduleSupport::Unsupported { missing }
        if missing.contains(&ScheduleCapability::CreateSession)
    ));
    assert!(matches!(
        router
            .support(&ScheduleDestination::Fork {
                source: target.clone(),
                through_turn_id: NonEmptyText::try_from("turn".to_owned()).expect("turn"),
            })
            .await
            .expect("fork support"),
        ScheduleSupport::Unsupported { missing }
        if missing.contains(&ScheduleCapability::ForkSession)
    ));
    let fork = router
        .prepare_destination(
            SchedulePreparationRequest {
                operation_id: OperationId::generate(),
                schedule_id: agent_automation::ScheduleId::generate(),
                expected_change_id: agent_automation::ChangeId::generate(),
                destination: DestinationPreparation::Fork {
                    source: target.clone(),
                    through_turn_id: NonEmptyText::try_from("turn".to_owned()).expect("turn"),
                    cwd: "/tmp".into(),
                },
                instruction_text: "Check work".into(),
                model: None,
                effort: String::new(),
            },
            &preparation,
        )
        .await
        .expect("fork rejection");
    assert!(matches!(fork, SchedulePreparationOutcome::Failed(failure)
        if matches!(failure.kind, collaboration_protocol::ScheduleFailureKind::UnsupportedCapability)
            && failure.explanation.contains("conversation create")));
    let sink = RecordedRunEvidence(tokio::sync::Mutex::new(Vec::new()));
    let prepared = router
        .prepare_existing_target(&target, &sink)
        .await
        .expect("existing target prepared");
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
    let run = ScheduledRunSubmission {
        run_id: run_id.clone(),
        target: target.clone(),
        message: MessageText::try_from("scheduled input".to_owned()).expect("message"),
        precondition: DeliveryPrecondition::Unpinned,
        inputs: inputs.clone(),
        recorded: prepared.evidence,
    };
    let active_id = OperationId::generate();
    supervisor
        .prompt(ConversationPromptRequest {
            operation_id: active_id.clone(),
            target: target.clone(),
            generation: Some(generation),
            requested_by: target.clone(),
            approver: target.clone(),
            prompt: MessageContent::HumanUser {
                text: MessageText::try_from("active input".to_owned()).expect("active text"),
            },
        })
        .await
        .expect("active prompt admitted");
    let (mut active_event, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept())
        .await
        .expect("active deadline")
        .expect("active event");
    let mut marker = [0_u8; 6];
    active_event
        .read_exact(&mut marker)
        .await
        .expect("active marker");
    let busy = router
        .submit_run(run.clone(), &sink)
        .await
        .expect("busy result");
    assert!(matches!(busy, RunSubmission::NotStartedBusy));
    assert_eq!(sink.0.lock().await.len(), 1, "busy run dispatched");
    active_event
        .write_all(b"x")
        .await
        .expect("release active prompt");
    let settled_active = supervisor
        .wait(ConversationOperationWaitRequest {
            operation_id: active_id,
            timeout_seconds: PositiveSeconds::try_from(3).expect("timeout"),
        })
        .await
        .expect("active settled");
    assert!(matches!(
        settled_active.output,
        ConversationOperationWaitOutput::Available { .. }
    ));

    let started = router
        .submit_run(run, &sink)
        .await
        .expect("idle run result");
    assert!(matches!(started, RunSubmission::Started(acceptance)
        if matches!(acceptance.execution, RunExecution::ProviderAcp { .. })
            && acceptance.receipt.outcome == DeliveryOutcome::Started));
    let recorded = sink
        .0
        .lock()
        .await
        .last()
        .expect("accepted evidence")
        .clone();
    let next_inputs = inputs.clone();
    let context = RunObservationContext {
        run_id,
        phase: RunPhase::Executing,
        recorded,
        inputs,
    };
    assert!(matches!(
        router
            .observe_settlement(context.clone())
            .await
            .expect("settlement"),
        RunSettlement::Completed {
            summary_source: None
        }
    ));
    let restarted_supervisor = Arc::new(
        ExternalProviderSupervisor::new(Vec::new(), Arc::clone(&store))
            .expect("restarted supervisor"),
    );
    let restarted_route = Arc::new(ProviderAcpDeliveryRoute::new(
        service_id,
        EndpointDirectory::new(target.endpoint.service_id.clone()),
        Arc::clone(&restarted_supervisor),
        store,
        Arc::new(NoLivePeer),
    ));
    let restarted_router =
        SessionDeliveryRouter::new(vec![restarted_route as Arc<dyn SessionDeliveryRoute>]);
    assert!(matches!(
        restarted_router
            .observe_settlement(context.clone())
            .await
            .expect("restarted settlement"),
        RunSettlement::Completed {
            summary_source: None
        }
    ));
    let mut crash_context = context.clone();
    crash_context.run_id = RunId::generate();
    if let RouteEffectEvidence::ProviderAcp(provider) = &mut crash_context.recorded {
        provider.attempt_id = agent_automation::AttemptId::generate();
        provider.submission = SubmissionEffect::Dispatching;
    }
    let mut missing_accepted = crash_context.clone();
    if let RouteEffectEvidence::ProviderAcp(provider) = &mut missing_accepted.recorded {
        provider.submission = SubmissionEffect::Accepted;
    }
    assert!(matches!(
        restarted_router
            .reconcile_run(crash_context)
            .await
            .expect("crash reconciliation"),
        RunReconciliation::KnownNotSubmitted
    ));
    assert!(matches!(
        restarted_router
            .reconcile_run(missing_accepted)
            .await
            .expect("missing accepted reconciliation"),
        RunReconciliation::StillUnknown
    ));
    assert!(matches!(
        router.reconcile_run(context).await.expect("reconcile"),
        RunReconciliation::Settled {
            settlement: RunSettlement::Completed { .. }
        }
    ));

    let stop_sink = RecordedRunEvidence(tokio::sync::Mutex::new(Vec::new()));
    let prepared_stop = router
        .prepare_existing_target(&target, &stop_sink)
        .await
        .expect("stop target prepared");
    let stop_run_id = RunId::generate();
    let stop_submission = router
        .submit_run(
            ScheduledRunSubmission {
                run_id: stop_run_id.clone(),
                target: target.clone(),
                message: MessageText::try_from("cancel scheduled input".to_owned())
                    .expect("cancel input"),
                precondition: DeliveryPrecondition::Unpinned,
                inputs: next_inputs.clone(),
                recorded: prepared_stop.evidence,
            },
            &stop_sink,
        )
        .await
        .expect("stop run started");
    assert!(matches!(stop_submission, RunSubmission::Started(_)));
    let pending_inputs = next_inputs.clone();
    let stop_context = RunObservationContext {
        run_id: stop_run_id,
        phase: RunPhase::Executing,
        recorded: stop_sink
            .0
            .lock()
            .await
            .last()
            .expect("stop run evidence")
            .clone(),
        inputs: next_inputs,
    };
    assert!(matches!(
        router
            .request_stop(stop_context.clone(), &stop_sink)
            .await
            .expect("stop request"),
        collaboration_service::StopRequestOutcome::Requested
    ));
    assert!(matches!(
        router
            .observe_settlement(stop_context)
            .await
            .expect("cancelled settlement"),
        RunSettlement::Interrupted
    ));

    let pending_sink = RecordedRunEvidence(tokio::sync::Mutex::new(Vec::new()));
    let prepared_pending = router
        .prepare_existing_target(&target, &pending_sink)
        .await
        .expect("pending target prepared");
    let pending_run_id = RunId::generate();
    assert!(matches!(
        router
            .submit_run(
                ScheduledRunSubmission {
                    run_id: pending_run_id.clone(),
                    target,
                    message: MessageText::try_from("never settled".to_owned())
                        .expect("pending input"),
                    precondition: DeliveryPrecondition::Unpinned,
                    inputs: pending_inputs.clone(),
                    recorded: prepared_pending.evidence,
                },
                &pending_sink,
            )
            .await
            .expect("pending run started"),
        RunSubmission::Started(_)
    ));
    let pending_context = RunObservationContext {
        run_id: pending_run_id,
        phase: RunPhase::Executing,
        recorded: pending_sink
            .0
            .lock()
            .await
            .last()
            .expect("pending run evidence")
            .clone(),
        inputs: pending_inputs,
    };
    assert!(matches!(
        router
            .request_stop(pending_context.clone(), &pending_sink)
            .await
            .expect("pending stop"),
        collaboration_service::StopRequestOutcome::Requested
    ));
    assert!(matches!(
        router
            .observe_settlement(pending_context)
            .await
            .expect("pending observation"),
        RunSettlement::Pending
    ));
    route.shutdown_queue().await;
    supervisor.shutdown().await.expect("supervisor shutdown");
    restarted_supervisor
        .shutdown()
        .await
        .expect("restarted supervisor shutdown");
}

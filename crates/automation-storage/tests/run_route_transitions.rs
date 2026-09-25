use agent_automation::{
    AttemptId, CessationEvidence, ClaudeCodePeerEffectEvidence, ContinuityInput,
    ExecutionDestination, InstructionText, NativeEffectEvidence, OperationId, PeerWriteEffect,
    PreparationEffect, ProviderAcpEffectEvidence, ProviderBindingReference,
    ProviderSettlementEffect, RouteEffectEvidence, RouteSettlementState, RunId, RunPhase,
    ScheduleDefinition, SubmissionEffect, TimingRule, WorkerOutcome,
};
use automation_storage::{
    AutomationStore, RunAdmission, RunCompletion, RunDispatchIntent, RunPreparationIntent,
    RunStopIdentity, RunSubmissionOutcome, RunSubmissionResult, ScheduleCreate, SummaryAdmission,
    SummaryCompletion, SummaryProgress,
};
use sqlx::Connection;

async fn admitted_run(store: &mut AutomationStore) -> Result<RunId, Box<dyn std::error::Error>> {
    let instruction = store
        .create_instruction(
            &OperationId::generate(),
            &InstructionText::try_from("Check job".to_owned())?,
            0,
        )
        .await?;
    let schedule = store
        .create_schedule(&ScheduleCreate::<String, String> {
            operation_id: OperationId::generate(),
            definition: ScheduleDefinition {
                instruction_id: instruction.instruction_id,
                timing: TimingRule::Interval { seconds: 60 },
                enabled: true,
                destination: ExecutionDestination::OwnedThread {
                    target: "target".into(),
                    cwd: "/isolated-fixture".into(),
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
        .enqueue_due_run::<String, String>(&schedule.schedule_id, 60000)
        .await?
        .ok_or("missing run")?;
    if !matches!(
        store
            .admit_waiting_run::<String, String>(&schedule.schedule_id, 61000)
            .await?,
        RunAdmission::Admitted { .. }
    ) {
        return Err("missing run admission".into());
    }
    Ok(run_id)
}

fn provider_evidence(
    attempt_id: AttemptId,
    submission: SubmissionEffect,
    settlement: ProviderSettlementEffect,
) -> Result<RouteEffectEvidence<String, String>, Box<dyn std::error::Error>> {
    Ok(RouteEffectEvidence::ProviderAcp(
        ProviderAcpEffectEvidence {
            target: "target".into(),
            generation: "generation-1".into(),
            binding: ProviderBindingReference::try_from("binding-1".to_owned())?,
            attempt_id,
            submission,
            settlement,
        },
    ))
}

fn peer_evidence(
    write: PeerWriteEffect,
) -> Result<RouteEffectEvidence<String, String>, Box<dyn std::error::Error>> {
    Ok(RouteEffectEvidence::ClaudeCodePeer(
        ClaudeCodePeerEffectEvidence {
            session_id: "target".to_owned().try_into()?,
            process_id: 42.try_into()?,
            write,
        },
    ))
}

#[tokio::test]
async fn provider_run_stop_waits_for_exact_operation_settlement()
-> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::temp_dir().join(format!(
        "provider-run-{}.sqlite",
        OperationId::generate().as_str()
    ));
    let mut store = AutomationStore::open(&path).await?;
    let run_id = admitted_run(&mut store).await?;
    let attempt_id = AttemptId::generate();
    if store
        .read_run::<String, String, String, String>(&run_id)
        .await?
        .evidence
        .route
        .is_some()
    {
        return Err("waiting run selected a client before preparation".into());
    }
    if store
        .complete_run_settlement::<String, String, String, String>(RunCompletion {
            run_id: run_id.clone(),
            settlement: RunStopIdentity::NativeTurn("never-started".into()),
            outcome: WorkerOutcome::Completed { explanation: None },
            now_ms: 62000,
        })
        .await
        .is_ok()
    {
        return Err("unselected run accepted completion".into());
    }
    if store
        .begin_run_dispatch::<String, String, String, String>(RunDispatchIntent {
            run_id: run_id.clone(),
            effects: provider_evidence(
                attempt_id.clone(),
                SubmissionEffect::Dispatching,
                ProviderSettlementEffect::NotObserved,
            )?,
            configured_timeout_seconds: 3600,
            now_ms: 62000,
        })
        .await
        .is_ok()
    {
        return Err("unselected run dispatched without a persisted route".into());
    }
    if !store
        .begin_run_preparation::<String, String, String, String>(RunPreparationIntent {
            run_id: run_id.clone(),
            effects: provider_evidence(
                attempt_id.clone(),
                SubmissionEffect::NotDispatched,
                ProviderSettlementEffect::NotObserved,
            )?,
        })
        .await?
    {
        return Err("provider route preparation was not recorded".into());
    }
    if store
        .begin_run_dispatch::<String, String, String, String>(RunDispatchIntent {
            run_id: run_id.clone(),
            effects: provider_evidence(
                AttemptId::generate(),
                SubmissionEffect::Dispatching,
                ProviderSettlementEffect::NotObserved,
            )?,
            configured_timeout_seconds: 3600,
            now_ms: 62000,
        })
        .await
        .is_ok()
    {
        return Err("different provider operation dispatched the run".into());
    }
    store
        .begin_run_dispatch::<String, String, String, String>(RunDispatchIntent {
            run_id: run_id.clone(),
            effects: provider_evidence(
                attempt_id.clone(),
                SubmissionEffect::Dispatching,
                ProviderSettlementEffect::NotObserved,
            )?,
            configured_timeout_seconds: 3600,
            now_ms: 62000,
        })
        .await?;
    if store
        .record_run_submission::<String, String, String, String>(RunSubmissionResult {
            run_id: run_id.clone(),
            effects: provider_evidence(
                AttemptId::generate(),
                SubmissionEffect::Unknown,
                ProviderSettlementEffect::NotObserved,
            )?,
            outcome: RunSubmissionOutcome::Unknown {
                explanation: "different operation".into(),
            },
        })
        .await
        .is_ok()
    {
        return Err("unrelated provider operation changed the run".into());
    }
    store
        .record_run_submission::<String, String, String, String>(RunSubmissionResult {
            run_id: run_id.clone(),
            effects: provider_evidence(
                attempt_id.clone(),
                SubmissionEffect::Accepted,
                ProviderSettlementEffect::NotObserved,
            )?,
            outcome: RunSubmissionOutcome::ProviderAdmitted {
                receipt: "operation admitted".to_owned(),
            },
        })
        .await?;
    if !store
        .begin_run_stop::<String, String, String, String>(
            &run_id,
            RunStopIdentity::ProviderOperation(attempt_id.clone()),
        )
        .await?
    {
        return Err("provider cancellation intent was not recorded".into());
    }
    let stopping = store
        .read_run::<String, String, String, String>(&run_id)
        .await?;
    if stopping.phase != RunPhase::Stopping || stopping.worker_outcome.is_some() {
        return Err("cancellation finalized a running provider operation".into());
    }
    if store
        .complete_run_settlement::<String, String, String, String>(RunCompletion {
            run_id: run_id.clone(),
            settlement: RunStopIdentity::ProviderOperation(AttemptId::generate()),
            outcome: WorkerOutcome::Completed { explanation: None },
            now_ms: 63000,
        })
        .await?
    {
        return Err("another provider operation settled this run".into());
    }
    if !store
        .complete_run_settlement::<String, String, String, String>(RunCompletion {
            run_id: run_id.clone(),
            settlement: RunStopIdentity::ProviderOperation(attempt_id),
            outcome: WorkerOutcome::Completed { explanation: None },
            now_ms: 63000,
        })
        .await?
    {
        return Err("exact provider operation settlement did not finish the run".into());
    }
    let finished = store
        .read_run::<String, String, String, String>(&run_id)
        .await?;
    if finished.phase != RunPhase::Finished
        || !matches!(
            finished.evidence.route.as_ref(),
            Some(RouteEffectEvidence::ProviderAcp(_))
        )
        || finished
            .evidence
            .route
            .as_ref()
            .map(RouteEffectEvidence::settlement_state)
            != Some(RouteSettlementState::ProviderOperationConfirmed)
    {
        return Err("provider settlement lost route identity or worker outcome".into());
    }
    store.close().await?;
    std::fs::remove_file(path)?;
    Ok(())
}

#[tokio::test]
async fn peer_written_run_finishes_without_claiming_turn_completion()
-> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::temp_dir().join(format!(
        "peer-run-{}.sqlite",
        OperationId::generate().as_str()
    ));
    let mut store = AutomationStore::open(&path).await?;
    let run_id = admitted_run(&mut store).await?;
    if !store
        .begin_run_preparation::<String, String, String, String>(RunPreparationIntent {
            run_id: run_id.clone(),
            effects: peer_evidence(PeerWriteEffect::NotDispatched)?,
        })
        .await?
    {
        return Err("peer route preparation was not recorded".into());
    }
    if store
        .begin_run_dispatch::<String, String, String, String>(RunDispatchIntent {
            run_id: run_id.clone(),
            effects: peer_evidence(PeerWriteEffect::Written)?,
            configured_timeout_seconds: 3600,
            now_ms: 62000,
        })
        .await
        .is_ok()
    {
        return Err("peer write cannot bypass dispatch intent".into());
    }
    store
        .begin_run_dispatch::<String, String, String, String>(RunDispatchIntent {
            run_id: run_id.clone(),
            effects: peer_evidence(PeerWriteEffect::Dispatching)?,
            configured_timeout_seconds: 3600,
            now_ms: 62000,
        })
        .await?;
    if store
        .record_run_submission::<String, String, String, String>(RunSubmissionResult {
            run_id: run_id.clone(),
            effects: peer_evidence(PeerWriteEffect::Written)?,
            outcome: RunSubmissionOutcome::PeerWritten {
                written_at_ms: 61000,
                receipt: "peer receipt".into(),
            },
        })
        .await
        .is_ok()
    {
        return Err("peer write predated its dispatch budget".into());
    }
    store
        .record_run_submission::<String, String, String, String>(RunSubmissionResult {
            run_id: run_id.clone(),
            effects: peer_evidence(PeerWriteEffect::Written)?,
            outcome: RunSubmissionOutcome::PeerWritten {
                written_at_ms: 63000,
                receipt: "peer receipt".into(),
            },
        })
        .await?;
    let finished = store
        .read_run::<String, String, String, String>(&run_id)
        .await?;
    if finished.phase != RunPhase::Finished
        || finished.native_turn_id.is_some()
        || !matches!(
            finished.evidence.route.as_ref(),
            Some(RouteEffectEvidence::ClaudeCodePeer(_))
        )
        || finished
            .evidence
            .route
            .as_ref()
            .map(RouteEffectEvidence::settlement_state)
            != Some(RouteSettlementState::PeerMessageWritten)
    {
        return Err("peer write claimed a native turn or remained active".into());
    }
    if store
        .begin_run_stop::<String, String, String, String>(
            &run_id,
            RunStopIdentity::NativeTurn("none".into()),
        )
        .await?
    {
        return Err("final peer write accepted a stop request".into());
    }
    store.close().await?;
    std::fs::remove_file(path)?;
    Ok(())
}

#[tokio::test]
async fn peer_written_evidence_finishes_after_uncertain_submission()
-> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::temp_dir().join(format!(
        "peer-reconcile-{}.sqlite",
        OperationId::generate().as_str()
    ));
    let mut store = AutomationStore::open(&path).await?;
    let run_id = admitted_run(&mut store).await?;
    store
        .begin_run_preparation::<String, String, String, String>(RunPreparationIntent {
            run_id: run_id.clone(),
            effects: peer_evidence(PeerWriteEffect::NotDispatched)?,
        })
        .await?;
    store
        .begin_run_dispatch::<String, String, String, String>(RunDispatchIntent {
            run_id: run_id.clone(),
            effects: peer_evidence(PeerWriteEffect::Dispatching)?,
            configured_timeout_seconds: 120,
            now_ms: 62_000,
        })
        .await?;
    store
        .record_run_submission::<String, String, String, String>(RunSubmissionResult {
            run_id: run_id.clone(),
            effects: peer_evidence(PeerWriteEffect::Written)?,
            outcome: RunSubmissionOutcome::Unknown {
                explanation: "reply lost after full write".into(),
            },
        })
        .await?;
    let uncertain = store
        .read_run::<String, String, String, String>(&run_id)
        .await?;
    if uncertain.phase != RunPhase::Uncertain {
        return Err("peer write was not retained as uncertain".into());
    }
    if !store
        .complete_run_settlement::<String, String, String, String>(RunCompletion {
            run_id: run_id.clone(),
            settlement: RunStopIdentity::PeerMessageWritten,
            outcome: WorkerOutcome::PeerMessageWritten {
                explanation: "Peer message written; receiver completion was not observed.".into(),
            },
            now_ms: 63_000,
        })
        .await?
    {
        return Err("recorded peer write did not settle".into());
    }
    let finished = store
        .read_run::<String, String, String, String>(&run_id)
        .await?;
    if finished.phase != RunPhase::Finished
        || finished.evidence.acceptance.is_some()
        || !matches!(
            finished.worker_outcome,
            Some(WorkerOutcome::PeerMessageWritten { .. })
        )
    {
        return Err("peer recovery fabricated client acceptance or completion".into());
    }
    store.close().await?;
    std::fs::remove_file(path)?;
    Ok(())
}

#[tokio::test]
async fn provider_summary_reference_feeds_the_next_fresh_run()
-> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::temp_dir().join(format!(
        "provider-summary-{}.sqlite",
        OperationId::generate().as_str()
    ));
    let mut store = AutomationStore::open(&path).await?;
    let instruction = store
        .create_instruction(
            &OperationId::generate(),
            &InstructionText::try_from("Check provider job".to_owned())?,
            0,
        )
        .await?;
    let schedule = store
        .create_schedule(&ScheduleCreate::<String, String> {
            operation_id: OperationId::generate(),
            definition: ScheduleDefinition {
                instruction_id: instruction.instruction_id,
                timing: TimingRule::Interval { seconds: 60 },
                enabled: true,
                destination: ExecutionDestination::FreshEachRun {
                    endpoint: "provider-endpoint".into(),
                    cwd: "/isolated-fixture".into(),
                },
                execution_timeout_seconds: Some(120),
                model: Some("fixture-model".into()),
                effort: Some("medium".into()),
            },
            imported_continuity: ContinuityInput::None,
            now_ms: 0,
        })
        .await?;
    let first_run = store
        .enqueue_due_run::<String, String>(&schedule.schedule_id, 60000)
        .await?
        .ok_or("first run missing")?;
    store
        .admit_waiting_run::<String, String>(&schedule.schedule_id, 61000)
        .await?;
    let provider_attempt = AttemptId::generate();
    let preparing = provider_evidence(
        provider_attempt.clone(),
        SubmissionEffect::NotDispatched,
        ProviderSettlementEffect::NotObserved,
    )?;
    store
        .begin_run_preparation::<String, String, String, String>(RunPreparationIntent {
            run_id: first_run.clone(),
            effects: preparing.clone(),
        })
        .await?;
    store
        .begin_run_dispatch::<String, String, String, String>(RunDispatchIntent {
            run_id: first_run.clone(),
            effects: provider_evidence(
                provider_attempt.clone(),
                SubmissionEffect::Dispatching,
                ProviderSettlementEffect::NotObserved,
            )?,
            configured_timeout_seconds: 3600,
            now_ms: 62000,
        })
        .await?;
    store
        .record_run_submission::<String, String, String, String>(RunSubmissionResult {
            run_id: first_run.clone(),
            effects: provider_evidence(
                provider_attempt.clone(),
                SubmissionEffect::Accepted,
                ProviderSettlementEffect::NotObserved,
            )?,
            outcome: RunSubmissionOutcome::ProviderAdmitted {
                receipt: "provider accepted".into(),
            },
        })
        .await?;
    if !store
        .complete_run_settlement::<String, String, String, String>(RunCompletion {
            run_id: first_run.clone(),
            settlement: RunStopIdentity::ProviderOperation(provider_attempt.clone()),
            outcome: WorkerOutcome::Completed { explanation: None },
            now_ms: 63000,
        })
        .await?
    {
        return Err("provider operation did not settle its exact run".into());
    }
    let attempt = store
        .begin_required_summary::<String, String, String, String>(&SummaryAdmission {
            run_id: first_run.clone(),
            timeout_seconds: 900,
            now_ms: 64000,
        })
        .await?;
    if attempt.source_reference
        != (agent_automation::SummarySourceReference::ProviderOperation {
            attempt_id: provider_attempt.clone(),
        })
    {
        return Err("summary admission invented a native turn source".into());
    }
    let summary_turn = "summary-turn".to_owned();
    let summary_target = "summary-worker".to_owned();
    store
        .record_summary_progress::<String, String>(SummaryProgress {
            run_id: first_run.clone(),
            attempt_id: attempt.attempt_id.clone(),
            phase: agent_automation::SummaryPhase::Running,
            effects: NativeEffectEvidence {
                target: Some(summary_target.clone()),
                generation: Some("summary-generation".into()),
                client_user_message_id: Some(attempt.attempt_id.as_str().into()),
                native_turn_id: Some(summary_turn.clone()),
                native_submission_id: None,
                allocation: PreparationEffect::Accepted,
                resume: PreparationEffect::NotRequested,
                submission: SubmissionEffect::Accepted,
                cessation: CessationEvidence::Unconfirmed,
            },
            target: Some(summary_target),
            native_turn_id: Some(summary_turn.clone()),
            explanation: None,
        })
        .await?;
    if !store
        .complete_summary::<String, String>(SummaryCompletion {
            run_id: first_run.clone(),
            attempt_id: attempt.attempt_id,
            native_turn_id: summary_turn,
            text: InstructionText::try_from("Provider work completed".to_owned())?,
            now_ms: 65000,
        })
        .await?
    {
        return Err("provider summary did not complete".into());
    }
    let next_run = store
        .enqueue_due_run::<String, String>(&schedule.schedule_id, 120000)
        .await?
        .ok_or("successor run missing")?;
    let next = store
        .admit_waiting_run::<String, String>(&schedule.schedule_id, 121000)
        .await?;
    let RunAdmission::Admitted { inputs, .. } = next else {
        return Err("successor run was not admitted".into());
    };
    match inputs.continuity {
        ContinuityInput::LocalSummary {
            source_run_id,
            source_reference:
                agent_automation::SummarySourceReference::ProviderOperation { attempt_id },
            text,
            ..
        } if source_run_id == first_run
            && attempt_id == provider_attempt
            && text == "Provider work completed" => {}
        _ => return Err("successor did not inherit exact provider summary provenance".into()),
    }
    if next_run == first_run {
        return Err("summary reused the prior run identity".into());
    }
    store.close().await?;
    std::fs::remove_file(path)?;
    Ok(())
}

#[tokio::test]
async fn old_native_waiting_placeholder_prepares_as_codex_evidence()
-> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::temp_dir().join(format!(
        "legacy-run-{}.sqlite",
        OperationId::generate().as_str()
    ));
    let mut store = AutomationStore::open(&path).await?;
    let run_id = admitted_run(&mut store).await?;
    store.close().await?;
    let mut connection = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new().filename(&path),
    )
    .await?;
    let legacy = serde_json::json!({
        "native": {
            "target":null,"generation":null,"clientUserMessageId":null,
            "nativeTurnId":null,"nativeSubmissionId":null,
            "allocation":"notRequested","resume":"notRequested",
            "submission":"notDispatched","cessation":"notApplicable"
        },
        "timing":null,"acceptance":null
    });
    sqlx::query("UPDATE workflow_runs SET execution_evidence_json=? WHERE run_id=?")
        .bind(legacy.to_string())
        .bind(run_id.as_str())
        .execute(&mut connection)
        .await?;
    connection.close().await?;
    let mut store = AutomationStore::open(&path).await?;
    let initial = NativeEffectEvidence {
        target: Some("target".to_owned()),
        generation: Some("generation-1".to_owned()),
        client_user_message_id: None,
        native_turn_id: None,
        native_submission_id: None,
        allocation: PreparationEffect::NotRequested,
        resume: PreparationEffect::NotRequested,
        submission: SubmissionEffect::NotDispatched,
        cessation: CessationEvidence::NotApplicable,
    };
    if !store
        .begin_run_preparation::<String, String, String, String>(RunPreparationIntent {
            run_id: run_id.clone(),
            effects: initial.into(),
        })
        .await?
    {
        return Err("legacy native placeholder did not prepare".into());
    }
    let prepared = store
        .read_run::<String, String, String, String>(&run_id)
        .await?;
    if !matches!(
        prepared.evidence.route,
        Some(RouteEffectEvidence::CodexAppServer(_))
    ) {
        return Err("legacy native placeholder changed client identity".into());
    }
    store.close().await?;
    std::fs::remove_file(path)?;
    Ok(())
}

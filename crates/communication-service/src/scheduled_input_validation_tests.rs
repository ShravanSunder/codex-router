//! Locally invalid assembled input must not become an uncertain native allocation.
use super::*;
use agent_automation::{
    ContinuityInput, InstructionText, OperationId, ScheduleDefinition, TimingRule,
};
use automation_storage::{RunAdmission, ScheduleCreate, ScheduleEdit, ScheduleMutation};
use serde_json::json;

#[tokio::test]
async fn oversized_combined_input_fails_locally_and_releases_admission()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    exercise_input_validation(false).await
}
#[tokio::test]
async fn oversized_input_after_preparation_preserves_target_without_submission()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    exercise_input_validation(true).await
}
async fn exercise_input_validation(
    prepared: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let root = std::path::PathBuf::from("/tmp").join(format!(
        "scheduled-input-validation-{}",
        OperationId::generate().as_str()
    ));
    std::fs::create_dir(&root)?;
    let store = Arc::new(Mutex::new(
        AutomationStore::open(&root.join("automation.sqlite")).await?,
    ));
    let endpoint: EndpointRef = serde_json::from_value(
        json!({"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"}),
    )?;
    let generation: CodexGeneration = serde_json::from_value(
        json!({"serviceEpoch":"00000000-0000-4000-8000-000000000002","generation":1}),
    )?;
    let instruction = store
        .lock()
        .await
        .create_instruction(
            &OperationId::generate(),
            &InstructionText::try_from("x".repeat(if prepared { 1_048_500 } else { 1_000_000 }))?,
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
                destination: ExecutionDestination::FreshEachRun {
                    endpoint: endpoint.clone(),
                    cwd: root.to_string_lossy().into_owned(),
                },
                execution_timeout_seconds: None,
            },
            imported_continuity: ContinuityInput::ImportedSummary {
                text: "y".repeat(100_000),
                source_run_id: "prior-run".into(),
                source_target: SessionRef {
                    endpoint: endpoint.clone(),
                    session_id: "prior-thread".to_owned().try_into()?,
                },
                import_operation_id: OperationId::generate(),
            },
            now_ms: 0,
        })
        .await?;
    let run = store
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
    let socket_path = root.join("native.sock");
    let listener = tokio::net::UnixListener::bind(&socket_path)?;
    let gate = crate::NativeGenerationGate::default();
    gate.activate(generation, socket_path, None)?;
    if prepared {
        let mut effects = store
            .lock()
            .await
            .read_run::<SessionRef, EndpointRef, CodexGeneration, NativeSendReceipt>(&run)
            .await?
            .evidence
            .native;
        effects.target = Some(SessionRef {
            endpoint: endpoint.clone(),
            session_id: "prepared-thread".to_owned().try_into()?,
        });
        store
            .lock()
            .await
            .record_run_target::<SessionRef, EndpointRef, CodexGeneration, NativeSendReceipt>(
                RunPreparedTarget {
                    run_id: run.clone(),
                    effects,
                    binding: ThreadBindingClaim {
                        schedule_id: schedule.schedule_id.clone(),
                        service_id: String::from(endpoint.service_id.clone()),
                        endpoint_id: String::from(endpoint.endpoint_id.clone()),
                        thread_id: "prepared-thread".into(),
                        now_ms: 60000,
                    },
                },
            )
            .await?;
    }
    let worker = ScheduledRunWorker {
        store: store.clone(),
        backend: Some(NativeControlBackend {
            endpoint,
            gate,
            codex_home: root.clone(),
        }),
        configuration: crate::AutomationConfigurationHandle::default(),
    };
    worker.step(run.clone()).await?;
    let failed = store
        .lock()
        .await
        .read_run::<SessionRef, EndpointRef, CodexGeneration, NativeSendReceipt>(&run)
        .await?;
    if failed.phase != RunPhase::PreparationFailed
        || failed.evidence.native.allocation != PreparationEffect::NotRequested
        || failed.evidence.timing.is_some()
    {
        return Err(
            "local input failure became native uncertainty or consumed execution budget".into(),
        );
    }
    if failed.evidence.native.target.is_some() != prepared {
        return Err("local validation lost earlier preparation evidence".into());
    }
    if tokio::time::timeout(std::time::Duration::from_millis(50), listener.accept())
        .await
        .is_ok()
    {
        return Err("invalid assembled input contacted native backend".into());
    }
    let next_instruction = store
        .lock()
        .await
        .create_instruction(
            &OperationId::generate(),
            &InstructionText::try_from("Short task".to_owned())?,
            61000,
        )
        .await?;
    let mut definition = schedule.definition;
    definition.instruction_id = next_instruction.instruction_id;
    store
        .lock()
        .await
        .mutate_schedule(&ScheduleMutation {
            operation_id: OperationId::generate(),
            schedule_id: schedule.schedule_id.clone(),
            edit: ScheduleEdit::Replace {
                expected_change_id: schedule.change_id,
                definition,
            },
            now_ms: 61000,
        })
        .await?;
    store
        .lock()
        .await
        .enqueue_due_run::<SessionRef, EndpointRef>(&schedule.schedule_id, 120000)
        .await?
        .ok_or("successor missing")?;
    if !matches!(
        store
            .lock()
            .await
            .admit_waiting_run::<SessionRef, EndpointRef>(&schedule.schedule_id, 120000)
            .await?,
        RunAdmission::Admitted { .. }
    ) {
        return Err("known local input failure blocked successor admission".into());
    }
    drop(worker);
    drop(store);
    drop(listener);
    for entry in std::fs::read_dir(&root)? {
        std::fs::remove_file(entry?.path())?;
    }
    std::fs::remove_dir(root)?;
    Ok(())
}

use agent_automation::{
    ContinuityInput, ExecutionDestination, InstructionText, OperationId, ScheduleDefinition,
    TimingRule,
};
use automation_storage::{
    AutomationStore, ExternalAdmission, ExternalAdmissionResult, PreparationIntent, PreparedThread,
    ScheduleCreate,
};
use serde_json::json;
#[tokio::test]
async fn prepared_identity_and_schedule_result_commit_together()
-> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::temp_dir().join(format!(
        "prepared-commit-{}.sqlite",
        OperationId::generate().as_str()
    ));
    let mut store = AutomationStore::open(&path).await?;
    let instruction = store
        .create_instruction(
            &OperationId::generate(),
            &InstructionText::try_from("Check status".to_owned())?,
            0,
        )
        .await?;
    let schedule = store
        .create_schedule(&ScheduleCreate::<String, String> {
            operation_id: OperationId::generate(),
            definition: ScheduleDefinition {
                instruction_id: instruction.instruction_id,
                timing: TimingRule::Interval { seconds: 60 },
                enabled: false,
                destination: ExecutionDestination::Unprepared,
                execution_timeout_seconds: None,
            },
            imported_continuity: ContinuityInput::None,
            now_ms: 0,
        })
        .await?;
    let admission = ExternalAdmission {
        operation_id: OperationId::generate(),
        schedule_id: schedule.schedule_id.clone(),
        canonical_request: br#"{"kind":"fresh"}"#.to_vec(),
        evidence: json!({"allocation":"notDispatched"}),
        now_ms: 0,
    };
    store
        .admit_schedule_preparation::<_, serde_json::Value>(&admission)
        .await?;
    if !store
        .record_preparation_intent(&PreparationIntent {
            operation_id: admission.operation_id.clone(),
            schedule_id: schedule.schedule_id.clone(),
            evidence: json!({"allocation":"unknown"}),
        })
        .await?
    {
        return Err("native intent not persisted".into());
    }
    let completed = store
        .complete_thread_preparation::<_, String, _>(PreparedThread {
            operation_id: admission.operation_id.clone(),
            schedule_id: schedule.schedule_id.clone(),
            expected_change_id: schedule.change_id,
            target: "native-B".to_owned(),
            service_id: "service-fixture".into(),
            endpoint_id: "codex-local".into(),
            thread_id: "native-B".into(),
            cwd: "/fresh-fixture".into(),
            evidence: json!({"allocation":"accepted","target":"native-B"}),
            now_ms: 1,
        })
        .await?;
    if !matches!(
        completed.record.definition.destination,
        ExecutionDestination::OwnedThread { .. }
    ) || completed.record.definition.enabled
    {
        return Err("preparation did not preserve explicit activation boundary".into());
    }
    store.close().await?;
    let mut store = AutomationStore::open(&path).await?;
    let replay = store
        .admit_schedule_preparation::<_, serde_json::Value>(&admission)
        .await?;
    if !matches!(replay,ExternalAdmissionResult::Existing(record) if record.status=="succeeded"&&record.result.is_some())
    {
        return Err("completion receipt lost on reopen".into());
    }
    if store
        .record_preparation_intent(&PreparationIntent {
            operation_id: admission.operation_id,
            schedule_id: schedule.schedule_id,
            evidence: json!({}),
        })
        .await?
    {
        return Err("completed preparation repeated intent".into());
    }
    store.close().await?;
    std::fs::remove_file(path)?;
    Ok(())
}

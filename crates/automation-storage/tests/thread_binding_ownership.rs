use agent_automation::{
    ContinuityInput, ExecutionDestination, InstructionText, OperationId, ScheduleDefinition,
    TimingRule,
};
use automation_storage::{AutomationStore, ScheduleCreate, ThreadBindingClaim};
#[tokio::test]
async fn exact_thread_address_has_one_schedule_owner_even_when_disabled()
-> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::temp_dir().join(format!(
        "thread-binding-{}.sqlite",
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
    let definition = ScheduleDefinition::<String, String> {
        instruction_id: instruction.instruction_id,
        timing: TimingRule::Interval { seconds: 60 },
        enabled: false,
        destination: ExecutionDestination::Unprepared,
        execution_timeout_seconds: None,
    };
    let first = store
        .create_schedule(&ScheduleCreate {
            operation_id: OperationId::generate(),
            definition: definition.clone(),
            imported_continuity: ContinuityInput::None,
            now_ms: 0,
        })
        .await?;
    let second = store
        .create_schedule(&ScheduleCreate {
            operation_id: OperationId::generate(),
            definition,
            imported_continuity: ContinuityInput::None,
            now_ms: 1,
        })
        .await?;
    let claim = ThreadBindingClaim {
        schedule_id: first.schedule_id,
        service_id: "service-fixture".into(),
        endpoint_id: "codex-local".into(),
        thread_id: "native-thread-fixture".into(),
        now_ms: 2,
    };
    let binding = store.claim_thread_binding(&claim).await?;
    let again = store.claim_thread_binding(&claim).await?;
    if binding != again {
        return Err("same owner generated duplicate binding".into());
    }
    let conflict = store
        .claim_thread_binding(&ThreadBindingClaim {
            schedule_id: second.schedule_id,
            now_ms: 3,
            ..claim
        })
        .await;
    if conflict.is_ok() {
        return Err("disabled schedule lost its exclusive ownership".into());
    }
    store.close().await?;
    std::fs::remove_file(path)?;
    Ok(())
}

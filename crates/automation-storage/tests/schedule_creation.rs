use agent_automation::{
    ContinuityInput, ExecutionDestination, InstructionText, OperationId, ScheduleDefinition,
    TimingRule,
};
use automation_storage::{AutomationStore, ScheduleCreate};
use sqlx::{Connection, SqliteConnection};

#[tokio::test]
async fn schedule_creation_replays_without_resetting_timer_anchor()
-> Result<(), Box<dyn std::error::Error>> {
    // Arrange: reusable instructions and an enabled interval schedule.
    let path = std::env::temp_dir().join(format!(
        "automation-schedule-{}.sqlite",
        uuid::Uuid::now_v7()
    ));
    let mut store = AutomationStore::open(&path).await?;
    let instruction = store
        .create_instruction(
            &OperationId::generate(),
            &InstructionText::try_from("Check repository".to_owned())?,
            100,
        )
        .await?;
    let mut request = ScheduleCreate::<String, String> {
        operation_id: OperationId::generate(),
        definition: ScheduleDefinition {
            instruction_id: instruction.instruction_id,
            timing: TimingRule::Interval { seconds: 600 },
            enabled: true,
            destination: ExecutionDestination::FreshEachRun {
                endpoint: "debug".into(),
                cwd: "/work".into(),
            },
            execution_timeout_seconds: None,
        },
        imported_continuity: ContinuityInput::None,
        now_ms: 1000,
    };
    // Act: retry after the caller lost the initial response; current time differs.
    let first = store.create_schedule(&request).await?;
    request.now_ms = 9000;
    let replay = store.create_schedule(&request).await?;
    // Assert: current config and timer progress remain separate and stable.
    if first != replay || first.next_due_at_ms != Some(601000) {
        return Err("schedule replay reset its timing".into());
    }
    store.close().await?;
    let mut check =
        SqliteConnection::connect_with(&sqlx::sqlite::SqliteConnectOptions::new().filename(&path))
            .await?;
    let timer: (i64, i64) =
        sqlx::query_as("SELECT anchor_at_ms,next_due_at_ms FROM schedule_timing_state")
            .fetch_one(&mut check)
            .await?;
    if timer != (1000, 601000) {
        return Err("timer record missing or incorrect".into());
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM schedule_definitions")
        .fetch_one(&mut check)
        .await?;
    if count != 1 {
        return Err("duplicate schedule created".into());
    }
    check.close().await?;
    std::fs::remove_file(path)?;
    Ok(())
}

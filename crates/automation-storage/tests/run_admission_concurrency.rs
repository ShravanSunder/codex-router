use agent_automation::{ChangeId, InstructionText, OperationId, RunId, ScheduleId};
use automation_storage::{AutomationStore, RunAdmission};
use sqlx::{Connection, SqliteConnection};

#[tokio::test]
async fn two_connections_admit_only_one_run_and_capture_current_inputs()
-> Result<(), Box<dyn std::error::Error>> {
    // Arrange: real file-backed SQLite, two independent repository connections and one waiting Run.
    let path = std::env::temp_dir().join(format!(
        "automation-admission-{}.sqlite",
        uuid::Uuid::now_v7()
    ));
    let mut first = AutomationStore::open(&path).await?;
    let instruction = first
        .create_instruction(
            &OperationId::generate(),
            &InstructionText::try_from("Inspect repository".to_owned())?,
            100,
        )
        .await?;
    let schedule_id = ScheduleId::generate();
    let run_id = RunId::generate();
    let mut seed = SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&path)
            .foreign_keys(true),
    )
    .await?;
    let definition = serde_json::json!({"instructionId":instruction.instruction_id,"timing":{"kind":"interval","seconds":600},"enabled":true,"destination":{"kind":"freshEachRun","endpoint":"debug-endpoint","cwd":"/work-a"},"executionTimeoutSeconds":120});
    sqlx::query(
        "INSERT INTO schedule_definitions VALUES (?,?,?,1,?,'{\"kind\":\"none\"}',100,100)",
    )
    .bind(schedule_id.as_str())
    .bind(ChangeId::generate().as_str())
    .bind(instruction.instruction_id.as_str())
    .bind(definition.to_string())
    .execute(&mut seed)
    .await?;
    sqlx::query("INSERT INTO workflow_runs(run_id,schedule_id,due_at_ms,run_status,execution_evidence_json) VALUES (?,?,100,'waiting','{}')")
        .bind(run_id.as_str()).bind(schedule_id.as_str()).execute(&mut seed).await?;
    seed.close().await?;
    let mut second = AutomationStore::open(&path).await?;
    // Act: both repository instances try admission concurrently.
    let (left, right) = tokio::join!(
        first.admit_waiting_run::<String, String>(&schedule_id, 200),
        second.admit_waiting_run::<String, String>(&schedule_id, 200)
    );
    let outcomes = [left?, right?];
    let mut admitted = 0;
    let mut occupied = 0;
    for outcome in outcomes {
        match outcome {
            RunAdmission::Admitted {
                run_id: actual,
                inputs,
            } => {
                admitted += 1;
                if actual != run_id
                    || inputs.instruction_text.as_str() != "Inspect repository"
                    || inputs.execution_configuration.execution_timeout_seconds != Some(120)
                {
                    return Err("admission did not capture exact Run inputs".into());
                }
            }
            RunAdmission::Occupied { run_id: actual } => {
                occupied += 1;
                if actual != run_id {
                    return Err("wrong occupying Run".into());
                }
            }
            RunAdmission::NoWaitingRun => return Err("waiting Run was lost".into()),
            RunAdmission::DestinationUnprepared { .. } => {
                return Err("prepared destination was unexpectedly lost".into());
            }
        }
    }
    // Assert: one winner, one occupant observation, no stored definition pointers.
    if admitted != 1 || occupied != 1 {
        return Err("concurrent admission contract violated".into());
    }
    first.close().await?;
    second.close().await?;
    std::fs::remove_file(path)?;
    Ok(())
}

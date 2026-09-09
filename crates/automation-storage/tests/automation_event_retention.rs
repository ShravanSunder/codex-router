//! Retention removes only expired events; current state and instruction history remain authoritative.
use agent_automation::{EventId, InstructionText, OperationId};
use automation_storage::AutomationStore;
use sqlx::Connection;

#[tokio::test]
async fn cleanup_uses_calendar_months_and_preserves_boundary_and_current_records()
-> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::temp_dir().join(format!(
        "automation-retention-{}.sqlite",
        OperationId::generate().as_str()
    ));
    let mut store = AutomationStore::open(&path).await?;
    let instruction = store
        .create_instruction(
            &OperationId::generate(),
            &InstructionText::try_from("Retained instructions".to_owned())?,
            0,
        )
        .await?;
    let now = chrono::DateTime::parse_from_rfc3339("2026-08-31T12:00:00Z")?.timestamp_millis();
    let cutoff = chrono::DateTime::parse_from_rfc3339("2026-06-30T12:00:00Z")?.timestamp_millis();
    let mut observer = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&path)
            .foreign_keys(true),
    )
    .await?;
    for timestamp in [cutoff - 1, cutoff, now] {
        sqlx::query("INSERT INTO automation_events(event_id,subject_kind,subject_id,event_kind,event_body_json,recorded_at_ms) VALUES (?,'instruction',?,'fixture','{}',?)")
            .bind(EventId::generate().as_str()).bind(instruction.instruction_id.as_str()).bind(timestamp).execute(&mut observer).await?;
    }
    let mut removed = 0;
    for _ in 0..5 {
        let count = store.prune_automation_events(now, 1).await?;
        if count > 1 {
            return Err("cleanup exceeded its transaction batch limit".into());
        }
        removed += count;
        if count == 0 {
            break;
        }
    }
    if removed != 2 {
        return Err(
            "cleanup did not remove precisely the events older than the calendar cutoff".into(),
        );
    }
    let retained: Vec<i64> =
        sqlx::query_scalar("SELECT recorded_at_ms FROM automation_events ORDER BY recorded_at_ms")
            .fetch_all(&mut observer)
            .await?;
    if retained != vec![cutoff, now] {
        return Err("cleanup deleted the exact cutoff or retained expired history".into());
    }
    if store.read_instruction(&instruction.instruction_id).await? != instruction {
        return Err("event cleanup changed current instructions or their revision".into());
    }
    let receipts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM operation_receipts")
        .fetch_one(&mut observer)
        .await?;
    let revisions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM instruction_revisions")
        .fetch_one(&mut observer)
        .await?;
    if receipts != 1 || revisions != 1 {
        return Err("event cleanup removed replay or instruction history".into());
    }
    observer.close().await?;
    store.close().await?;
    std::fs::remove_file(path)?;
    Ok(())
}

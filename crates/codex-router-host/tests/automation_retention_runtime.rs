//! Host starts event maintenance after acquiring its own listeners, without a native runtime.
use codex_router_host::{CommunicationRuntime, CommunicationRuntimeInputs};
use communication_protocol::{InstructionText, OperationId};
use sqlx::Connection;
use std::{os::unix::fs::DirBuilderExt, time::Duration};

#[tokio::test]
async fn owned_host_automatically_prunes_events_without_deleting_current_state()
-> Result<(), Box<dyn std::error::Error>> {
    let root = std::path::PathBuf::from("/tmp").join(format!(
        "automation-cleanup-{}",
        OperationId::generate().as_str()
    ));
    std::fs::DirBuilder::new().mode(0o700).create(&root)?;
    let database = root.join("automation.sqlite");
    let mut store = automation_storage::AutomationStore::open(&database).await?;
    let instruction = store
        .create_instruction(
            &OperationId::generate(),
            &InstructionText::try_from("Preserve current work".to_owned())?,
            0,
        )
        .await?;
    store.close().await?;
    let runtime = CommunicationRuntime::start(CommunicationRuntimeInputs {
        directory: root.clone(),
        codex_home: root.clone(),
        backend_socket: root.join("unavailable.sock"),
        native_schema: None,
    })
    .await?;
    let mut observer = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&database)
            .foreign_keys(true),
    )
    .await?;
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut interval = tokio::time::interval(Duration::from_millis(10));
        loop {
            interval.tick().await;
            let remaining: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM automation_events WHERE recorded_at_ms=0")
                    .fetch_one(&mut observer)
                    .await?;
            if remaining == 0 {
                return Ok::<_, sqlx::Error>(());
            }
        }
    })
    .await??;
    let mut store = automation_storage::AutomationStore::open(&database).await?;
    if store.read_instruction(&instruction.instruction_id).await? != instruction {
        return Err("Host event maintenance changed current instructions".into());
    }
    store.close().await?;
    observer.close().await?;
    runtime.shutdown().await?;
    for entry in std::fs::read_dir(&root)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            return Err("unexpected fixture entry".into());
        }
        std::fs::remove_file(entry.path())?;
    }
    std::fs::remove_dir(root)?;
    Ok(())
}

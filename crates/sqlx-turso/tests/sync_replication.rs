//! Turso Sync through the driver against a real `tursodb` 0.8.1 sync server: a persistent
//! reader, atomic admissions under interleaved pulls, a hub outage, replicated migrations and a
//! directory hub serving two project databases
#![allow(clippy::panic_in_result_fn)]

mod support;

use sqlx_turso::{TursoConnection, sqlx::Connection};
use support::{
    TestResult,
    project_admission::{ProjectSnapshot, TaskRow, admit_task, complete_task, initialize_project},
    scratch_store::{
        PROJECT_STORE_MIGRATOR, ensure_no_dangling_foreign_keys, expected_fingerprint,
        foreign_keys_enabled, migrate_in_owned_transaction, project_store_migrator_through,
        schema_fingerprint, set_foreign_keys,
    },
    sync_server::{SyncServer, open_synced_store, pull, push},
};

async fn record_count(connection: &mut TursoConnection) -> sqlx::Result<i64> {
    sqlx_turso::query_file_scalar!("tests/queries/record_count.sql")
        .fetch_one(connection)
        .await
}

async fn insert_record(connection: &mut TursoConnection, sequence: i64, body: &str) -> TestResult {
    sqlx_turso::query!(
        "INSERT INTO records (sequence, body) VALUES (?, ?)",
        sequence,
        body
    )
    .execute(connection)
    .await?;
    Ok(())
}

/// A writer that migrated and initialized the project and pushed it, plus a reader that
/// bootstrapped from the hub
struct ReplicatedProject {
    _directory: tempfile::TempDir,
    server: SyncServer,
    writer: TursoConnection,
    reader: TursoConnection,
}

impl ReplicatedProject {
    async fn start() -> TestResult<Self> {
        let directory = tempfile::tempdir()?;
        let server = SyncServer::start_single_file(directory.path()).await?;
        let mut writer =
            open_synced_store(&directory.path().join("writer.db"), &server.base_url()).await?;
        migrate_in_owned_transaction(&mut writer, &PROJECT_STORE_MIGRATOR).await?;
        initialize_project(&mut writer).await?;
        push(&writer).await?;
        let reader =
            open_synced_store(&directory.path().join("reader.db"), &server.base_url()).await?;
        Ok(Self {
            _directory: directory,
            server,
            writer,
            reader,
        })
    }

    async fn stop(mut self) -> TestResult {
        self.writer.close().await?;
        self.reader.close().await?;
        self.server.shutdown().await
    }
}

#[tokio::test]
async fn a_persistent_reader_sees_pushed_rows_only_after_it_pulls() -> TestResult {
    // Arrange
    let mut project = ReplicatedProject::start().await?;

    for sequence in 1..=4_i64 {
        // Act
        insert_record(&mut project.writer, sequence, &format!("item-{sequence}")).await?;
        push(&project.writer).await?;
        let before_pull = record_count(&mut project.reader).await?;
        pull(&project.reader).await?;
        let after_pull = record_count(&mut project.reader).await?;

        // Assert: the same reader connection and its cached statement see the new row
        assert_eq!((before_pull, after_pull), (sequence - 1, sequence));
    }

    sqlx_turso::query!(
        "UPDATE records SET body = ? WHERE sequence = ?",
        "updated",
        1_i64
    )
    .execute(&mut project.writer)
    .await?;
    push(&project.writer).await?;
    pull(&project.reader).await?;
    let record = sqlx_turso::query_file!("tests/queries/record_by_sequence.sql", 1_i64)
        .fetch_one(&mut project.reader)
        .await?;
    assert_eq!(record.body, "updated");
    project.stop().await
}

#[tokio::test]
async fn admissions_replicate_atomically_while_the_reader_pulls() -> TestResult {
    // Arrange
    let mut project = ReplicatedProject::start().await?;
    let mut observed_event_counts = Vec::new();

    // Act: each round the writer admits and completes tasks and pushes while the reader pulls
    // and reads a snapshot; every snapshot must agree exactly within itself
    for round in 0..8_i64 {
        let writer = &mut project.writer;
        let reader = &mut project.reader;
        let (written, observed) = tokio::join!(
            async move {
                for sequence in round * 3 + 1..=round * 3 + 3 {
                    admit_task(writer, sequence).await?;
                    complete_task(writer, sequence).await?;
                }
                push(writer).await
            },
            async move {
                pull(reader).await?;
                ProjectSnapshot::read(reader).await
            }
        );
        written?;
        observed_event_counts.push(observed?.events.len());
        pull(&project.reader).await?;
        let reader_snapshot = ProjectSnapshot::read(&mut project.reader).await?;
        let writer_snapshot = ProjectSnapshot::read(&mut project.writer).await?;

        // Assert
        assert_eq!(reader_snapshot, writer_snapshot, "round {round}");
    }

    let final_snapshot = ProjectSnapshot::read(&mut project.reader).await?;
    assert_eq!(final_snapshot.tasks.len(), 24);
    assert_eq!(final_snapshot.events.len(), 48);
    assert_eq!(final_snapshot.dependencies.len(), 23);
    assert!(
        observed_event_counts
            .windows(2)
            .all(|pair| matches!(pair, [earlier, later] if earlier <= later)),
        "the reader's feed went backwards: {observed_event_counts:?}"
    );
    project.stop().await
}

#[tokio::test]
async fn local_writes_continue_while_the_hub_is_down_and_replicate_after_restart() -> TestResult {
    // Arrange
    let mut project = ReplicatedProject::start().await?;
    for sequence in 1..=3 {
        admit_task(&mut project.writer, sequence).await?;
    }
    push(&project.writer).await?;
    pull(&project.reader).await?;

    // Act
    project.server.shutdown().await?;
    let push_while_down = push(&project.writer).await;
    for sequence in 4..=6 {
        admit_task(&mut project.writer, sequence).await?;
    }
    project.server.restart().await?;
    push(&project.writer).await?;
    pull(&project.reader).await?;

    // Assert: the failed push returned an error (within the bound) instead of hanging
    assert!(push_while_down.is_err(), "push succeeded with the hub down");
    let reader_snapshot = ProjectSnapshot::read(&mut project.reader).await?;
    assert_eq!(reader_snapshot.tasks.len(), 6);
    assert_eq!(
        reader_snapshot,
        ProjectSnapshot::read(&mut project.writer).await?
    );
    project.stop().await
}

async fn reinsert_tasks(connection: &mut TursoConnection, tasks: &[TaskRow]) -> TestResult {
    for task in tasks {
        sqlx_turso::query!(
            "INSERT INTO tasks (id, milestone_id, status, revision) VALUES (?, ?, ?, ?)",
            task.id,
            task.milestone_id,
            task.status,
            task.revision
        )
        .execute(&mut *connection)
        .await?;
    }
    Ok(())
}

#[tokio::test]
async fn add_column_and_foreign_key_off_rebuild_migrations_replicate() -> TestResult {
    // Arrange
    let mut project = ReplicatedProject::start().await?;
    for sequence in 1..=3 {
        admit_task(&mut project.writer, sequence).await?;
    }
    complete_task(&mut project.writer, 2).await?;
    push(&project.writer).await?;
    pull(&project.reader).await?;
    let before = ProjectSnapshot::read(&mut project.writer).await?;

    // Act: add a column, replicate; then rebuild tasks under the foreign-key-off policy
    let add_column = project_store_migrator_through(2);
    migrate_in_owned_transaction(&mut project.writer, &add_column).await?;
    push(&project.writer).await?;
    pull(&project.reader).await?;
    let reader_after_add_column = schema_fingerprint(&mut project.reader).await?;

    let rebuild = project_store_migrator_through(3);
    set_foreign_keys(&mut project.writer, false).await?;
    let mut transaction = project.writer.begin_with("BEGIN IMMEDIATE").await?;
    rebuild.run_direct(None, &mut *transaction, false).await?;
    reinsert_tasks(&mut transaction, &before.tasks).await?;
    ensure_no_dangling_foreign_keys(&mut transaction).await?;
    transaction.commit().await?;
    set_foreign_keys(&mut project.writer, true).await?;
    push(&project.writer).await?;
    pull(&project.reader).await?;

    // Assert
    assert_eq!(
        reader_after_add_column,
        expected_fingerprint(&add_column).await?
    );
    let expected_after_rebuild = expected_fingerprint(&rebuild).await?;
    assert_eq!(
        schema_fingerprint(&mut project.writer).await?,
        expected_after_rebuild
    );
    assert_eq!(
        schema_fingerprint(&mut project.reader).await?,
        expected_after_rebuild
    );
    assert!(foreign_keys_enabled(&mut project.writer).await?);
    assert!(foreign_keys_enabled(&mut project.reader).await?);
    assert_eq!(ProjectSnapshot::read(&mut project.reader).await?, before);
    assert_eq!(ProjectSnapshot::read(&mut project.writer).await?, before);
    project.stop().await
}

#[tokio::test]
async fn a_directory_hub_keeps_two_project_databases_apart() -> TestResult {
    // Arrange
    let directory = tempfile::tempdir()?;
    let mut server = SyncServer::start_directory(directory.path()).await?;
    let mut stores = Vec::new();
    for (project, rows) in [("project-a", 2_i64), ("project-b", 3)] {
        let url = server.database_url(project);
        let mut writer =
            open_synced_store(&directory.path().join(format!("{project}-writer.db")), &url).await?;
        migrate_in_owned_transaction(&mut writer, &PROJECT_STORE_MIGRATOR).await?;
        for sequence in 1..=rows {
            insert_record(&mut writer, sequence, project).await?;
        }
        push(&writer).await?;
        stores.push((project, rows, url, writer));
    }

    for (project, rows, url, writer) in stores {
        // Act
        let mut reader =
            open_synced_store(&directory.path().join(format!("{project}-reader.db")), &url).await?;
        let count = record_count(&mut reader).await?;
        let foreign_bodies = sqlx_turso::query_scalar!(
            r#"SELECT COUNT(*) AS "count!: i64" FROM records WHERE body != ?"#,
            project
        )
        .fetch_one(&mut reader)
        .await?;

        // Assert
        assert_eq!((count, foreign_bodies), (rows, 0), "{project}");
        reader.close().await?;
        writer.close().await?;
    }
    server.shutdown().await
}

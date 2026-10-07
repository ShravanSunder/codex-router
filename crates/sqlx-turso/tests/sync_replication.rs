//! Turso Sync through the driver against a real `tursodb` 0.8.1 sync server: a persistent
//! reader, atomic admissions under interleaved pulls, a hub outage, replicated migrations and a
//! directory hub serving two project databases
#![allow(clippy::panic_in_result_fn)]

mod support;

use std::{
    collections::BTreeSet,
    time::{Duration, Instant},
};

use sqlx_turso::{TursoConnection, sqlx::Connection};
use support::{
    TestResult,
    project_admission::{ProjectSnapshot, TaskRow, admit_task, complete_task, initialize_project},
    scratch_store::{
        PROJECT_STORE_MIGRATOR, ensure_no_dangling_foreign_keys, expected_fingerprint,
        foreign_keys_enabled, migrate_in_owned_transaction, project_store_migrator_through,
        project_store_migrator_with_copy_rename_rebuild, schema_fingerprint, set_foreign_keys,
    },
    sync_server::{REMOTE_TIMEOUT, SyncServer, bounded, open_synced_store, pull, push},
};
use tokio::{sync::watch, time::timeout};

/// A push against a stopped hub must fail well inside this, not hang until a caller's bound
const FAST_FAILURE_BOUND: Duration = Duration::from_secs(5);

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

/// Pushes, then waits until the reader has finished two more observations: the second of them
/// starts after this push completed and ends before the writer moves on, so it sees exactly
/// this push's state
async fn push_then_wait_for_the_reader(
    writer: &TursoConnection,
    reader_observations: &mut watch::Receiver<usize>,
) -> TestResult {
    push(writer).await?;
    let target = *reader_observations.borrow_and_update() + 2;
    timeout(
        REMOTE_TIMEOUT,
        reader_observations.wait_for(|observed| *observed >= target),
    )
    .await
    .map_err(|_elapsed| "the reader stopped observing")?
    .map(|_observed| ())?;
    Ok(())
}

/// Proves, against a real hub: while the writer commits admissions (task state, event and
/// ledger decision in one transaction) and pushes after every transaction, a reader on another
/// worker thread pulls and reads snapshots continuously. Every snapshot agrees exactly
/// ([`ProjectSnapshot::read`] fails otherwise), the feed never goes backwards, and the reader
/// sees every intermediate feed position, not just round boundaries. The reader's pulls overlap
/// the writer's next transaction and push; the test does not force a pull into the middle of a
/// single push's network exchange.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn admissions_replicate_atomically_while_the_reader_pulls_concurrently() -> TestResult {
    // Arrange
    const TASKS: i64 = 12;
    let final_feed_length = usize::try_from(2 * TASKS)?;
    let ReplicatedProject {
        _directory,
        mut server,
        writer,
        reader,
    } = ReplicatedProject::start().await?;
    let (observation_sender, mut reader_observations) = watch::channel(0_usize);
    let (writer_finished_sender, writer_finished) = watch::channel(false);

    // Act
    let writing = tokio::spawn(async move {
        let mut writer = writer;
        for sequence in 1..=TASKS {
            admit_task(&mut writer, sequence).await?;
            push_then_wait_for_the_reader(&writer, &mut reader_observations).await?;
            complete_task(&mut writer, sequence).await?;
            push_then_wait_for_the_reader(&writer, &mut reader_observations).await?;
        }
        writer_finished_sender.send_replace(true);
        TestResult::Ok(writer)
    });
    let reading = tokio::spawn(async move {
        let mut reader = reader;
        let mut feed_lengths = Vec::new();
        loop {
            let writer_done = *writer_finished.borrow();
            pull(&reader).await?;
            let snapshot = ProjectSnapshot::read(&mut reader).await?;
            feed_lengths.push(snapshot.events.len());
            observation_sender.send_modify(|observed| *observed += 1);
            if writer_done && snapshot.events.len() == final_feed_length {
                return TestResult::Ok((reader, feed_lengths));
            }
            if writer_finished.has_changed().is_err() && !*writer_finished.borrow() {
                return Err("the writer stopped before finishing".into());
            }
        }
    });
    let (written, read) = timeout(Duration::from_secs(120), async {
        tokio::join!(writing, reading)
    })
    .await
    .map_err(|_elapsed| "writer and reader did not finish within 120 s")?;
    let mut writer = written??;
    let (mut reader, feed_lengths) = read??;

    // Assert
    let observed: BTreeSet<usize> = feed_lengths.iter().copied().collect();
    let every_position: BTreeSet<usize> = (1..=final_feed_length).collect();
    assert!(
        every_position.is_subset(&observed),
        "the reader missed feed positions: {feed_lengths:?}"
    );
    assert!(
        feed_lengths
            .windows(2)
            .all(|pair| matches!(pair, [earlier, later] if earlier <= later)),
        "the reader's feed went backwards: {feed_lengths:?}"
    );
    let final_snapshot = ProjectSnapshot::read(&mut reader).await?;
    assert_eq!(final_snapshot, ProjectSnapshot::read(&mut writer).await?);
    assert_eq!(final_snapshot.tasks.len(), 12);
    assert_eq!(final_snapshot.dependencies.len(), 11);
    writer.close().await?;
    reader.close().await?;
    server.shutdown().await
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
    let started = Instant::now();
    let push_while_down = timeout(FAST_FAILURE_BOUND, project.writer.sync_push()).await;
    let failed_after = started.elapsed();
    for sequence in 4..=6 {
        admit_task(&mut project.writer, sequence).await?;
    }
    project.server.restart().await?;
    push(&project.writer).await?;
    pull(&project.reader).await?;

    // Assert: the push itself failed fast with an error, rather than timing out or succeeding
    assert!(
        matches!(push_while_down, Ok(Err(_))),
        "push with the hub down must return an error within {FAST_FAILURE_BOUND:?}: \
         {push_while_down:?} after {failed_after:?}"
    );
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

/// Known limitation, kept as a negative test: Turso 0.8.1 Sync cannot push a transaction that
/// writes rows into a table that is dropped or renamed before it commits. The SQL-only
/// copy-and-rename rebuild does exactly that. It commits locally, its push fails on the hub
/// with a replay syntax error, and the push is not atomic on the hub: a reader then pulls a
/// half-applied migration (`tasks` dropped, `tasks_rebuilt` created, not renamed). The drop,
/// recreate and reinsert shape above replicates.
#[tokio::test]
async fn a_sql_only_copy_and_rename_rebuild_commits_locally_but_cannot_be_pushed() -> TestResult {
    // Arrange
    let mut project = ReplicatedProject::start().await?;
    for sequence in 1..=3 {
        admit_task(&mut project.writer, sequence).await?;
    }
    push(&project.writer).await?;
    pull(&project.reader).await?;
    let migrator = project_store_migrator_with_copy_rename_rebuild();

    // Act
    set_foreign_keys(&mut project.writer, false).await?;
    migrate_in_owned_transaction(&mut project.writer, &migrator).await?;
    set_foreign_keys(&mut project.writer, true).await?;
    let pushed = bounded("sync push", project.writer.sync_push()).await;
    pull(&project.reader).await?;

    // Assert
    assert_eq!(
        schema_fingerprint(&mut project.writer).await?,
        expected_fingerprint(&migrator).await?
    );
    let error = pushed.expect_err("the hub cannot replay the copied rows");
    assert!(
        error.to_string().contains("syntax error")
            && error.to_string().contains("BATCH_STEP_ERROR"),
        "{error}"
    );
    let reader_tables: Vec<String> = schema_fingerprint(&mut project.reader)
        .await?
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    assert!(
        reader_tables.iter().any(|name| name == "tasks_rebuilt")
            && !reader_tables.iter().any(|name| name == "tasks"),
        "expected the hub to hold the half-applied rebuild: {reader_tables:?}"
    );
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

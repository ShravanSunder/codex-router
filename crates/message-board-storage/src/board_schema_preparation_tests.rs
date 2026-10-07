#![allow(clippy::unwrap_used)]

use crate::{
    BoardMigrationVersion, BoardSchemaPreparation, BoardSchemaPreparationError, BoardStore,
};
use sqlx::Connection;

#[path = "board_schema_preparation_test_support.rs"]
mod support;

use support::{
    apply_prefix_defect_database, capture_snapshot, commit_schema_change_from_wal_writer,
    create_full_native_database, create_legacy_database, create_pre_subscription_prefix,
    create_wal_current_subscription_fixture, open_writer, replace_native_history_with_view,
};

#[tokio::test]
async fn prepare_schema_does_not_create_a_missing_database() {
    let database_path = std::env::temp_dir().join(format!(
        "board-schema-preparation-missing-{}.sqlite",
        uuid::Uuid::now_v7()
    ));
    assert!(!database_path.exists());

    let error = BoardStore::prepare_schema(&database_path)
        .await
        .expect_err("missing database must be rejected");

    assert!(matches!(
        error,
        BoardSchemaPreparationError::UnreadableStore { .. }
    ));
    assert!(!database_path.exists());
}

#[tokio::test]
async fn prepare_schema_reports_the_exact_pre_subscription_suffix_without_backfill() {
    let fixture = create_pre_subscription_prefix("pre_subscription").await;
    let before = capture_snapshot(fixture.database.path()).await;
    assert!(!before.subscriptions_exist);
    assert_eq!(before.domain_counts, (1, 1, 1, 1, 1));

    let preparation = BoardStore::prepare_schema(fixture.database.path())
        .await
        .expect("actual pre-subscription prefix should prepare");
    match preparation {
        BoardSchemaPreparation::Pending { migrations } => {
            assert_eq!(
                migrations
                    .as_slice()
                    .iter()
                    .map(|version| version.get())
                    .collect::<Vec<_>>(),
                vec![202609170001, 202610010001, 202610020001]
            );
        }
        BoardSchemaPreparation::Current => {
            panic!("pre-subscription native prefix must report its exact suffix");
        }
    }

    assert_eq!(
        capture_snapshot(fixture.database.path()).await,
        before,
        "prepare must preserve the pre-subscription database byte-for-byte"
    );
    assert!(!before.subscriptions_exist);
}

#[tokio::test]
async fn prepare_schema_reports_current_without_mutating_the_database() {
    let database = create_full_native_database("current").await;
    let before = capture_snapshot(database.path()).await;

    assert!(matches!(
        BoardStore::prepare_schema(database.path()).await,
        Ok(BoardSchemaPreparation::Current)
    ));

    assert_eq!(capture_snapshot(database.path()).await, before);
}

#[tokio::test]
async fn prepare_schema_keeps_history_and_current_validation_in_one_wal_snapshot() {
    let fixture = create_wal_current_subscription_fixture("wal_snapshot").await;
    let database_path = fixture.database.path().to_path_buf();
    let before = capture_snapshot(&database_path).await;
    let (history_read_sender, history_read_receiver) = tokio::sync::oneshot::channel();
    let (writer_result_sender, writer_result_receiver) = tokio::sync::oneshot::channel();
    let writer_path = database_path.clone();
    let writer_task = tokio::spawn(async move {
        history_read_receiver
            .await
            .expect("inspector should reach the post-history checkpoint");
        let result = commit_schema_change_from_wal_writer(
            &writer_path,
            "ALTER TABLE board_projects ADD COLUMN snapshot_probe TEXT",
        )
        .await;
        writer_result_sender
            .send(result.clone())
            .expect("inspector should receive writer completion");
        result
    });

    let preparation = crate::board_schema_preparation::prepare_schema_with_checkpoint(
        &database_path,
        move || async move {
            history_read_sender
                .send(())
                .expect("WAL writer should await native history read");
            writer_result_receiver
                .await
                .expect("WAL writer should report its commit")
                .expect("WAL writer should add its fixture-only column");
        },
    )
    .await;
    writer_task
        .await
        .expect("external schema writer should finish")
        .expect("external schema writer should commit");
    assert!(
        matches!(&preparation, Ok(BoardSchemaPreparation::Current)),
        "first inspection should use its original SQLite snapshot: {preparation:?}"
    );

    let after_writer_commit = capture_snapshot(&database_path).await;
    assert_eq!(
        after_writer_commit.migration_history,
        before.migration_history
    );
    assert_eq!(after_writer_commit.user_version, before.user_version);
    assert_eq!(after_writer_commit.domain_counts, before.domain_counts);
    assert_eq!(after_writer_commit.project_rows, before.project_rows);
    assert_eq!(
        after_writer_commit.subscription_count,
        before.subscription_count
    );
    assert!(
        after_writer_commit
            .schema_objects
            .iter()
            .any(|(_, name, _, sql)| {
                name == "board_projects"
                    && sql
                        .as_deref()
                        .is_some_and(|definition| definition.contains("snapshot_probe"))
            })
    );

    let error = BoardStore::prepare_schema(&database_path)
        .await
        .expect_err("a later inspection should observe the committed schema change");
    assert!(matches!(
        error,
        BoardSchemaPreparationError::InvalidCurrentSchema { .. }
    ));
    let after_later_inspection = capture_snapshot(&database_path).await;
    // A WAL checkpoint may move the committed fixture DDL into the main file.
    assert_eq!(
        after_later_inspection.migration_history,
        after_writer_commit.migration_history
    );
    assert_eq!(
        after_later_inspection.schema_objects,
        after_writer_commit.schema_objects
    );
    assert_eq!(
        after_later_inspection.user_version,
        after_writer_commit.user_version
    );
    assert_eq!(
        after_later_inspection.domain_counts,
        after_writer_commit.domain_counts
    );
    assert_eq!(
        after_later_inspection.project_rows,
        after_writer_commit.project_rows
    );
    assert_eq!(
        after_later_inspection.subscription_count,
        after_writer_commit.subscription_count
    );
}

#[tokio::test]
async fn prepare_schema_refuses_legacy_history_without_adopting_it() {
    let database = create_legacy_database("legacy").await;
    let before = std::fs::read(database.path()).expect("legacy bytes should read");

    let error = BoardStore::prepare_schema(database.path())
        .await
        .expect_err("legacy board history must not be adopted during preparation");

    assert!(matches!(
        error,
        BoardSchemaPreparationError::UnrecognizedNativeHistory
    ));
    assert_eq!(
        std::fs::read(database.path()).expect("legacy bytes should remain readable"),
        before
    );
}

#[tokio::test]
async fn prepare_schema_refuses_a_view_in_place_of_native_migration_history() {
    let database = create_full_native_database("history_view").await;
    replace_native_history_with_view(database.path()).await;
    let before = capture_snapshot(database.path()).await;
    assert!(
        before
            .schema_objects
            .iter()
            .any(|(kind, name, _, _)| kind == "view" && name == "_sqlx_migrations")
    );

    let preparation = BoardStore::prepare_schema(database.path()).await;

    assert!(
        matches!(
            preparation,
            Err(BoardSchemaPreparationError::UnrecognizedNativeHistory)
        ),
        "a view cannot stand in for the native SQLx history table: {preparation:?}"
    );
    assert_eq!(capture_snapshot(database.path()).await, before);
}

#[tokio::test]
async fn prepare_schema_pending_result_connects_to_the_ordinary_writer_and_backfill() {
    let fixture = create_pre_subscription_prefix("prepare_then_writer").await;
    let before = capture_snapshot(fixture.database.path()).await;

    assert!(matches!(
        BoardStore::prepare_schema(fixture.database.path()).await,
        Ok(BoardSchemaPreparation::Pending { .. })
    ));
    assert_eq!(capture_snapshot(fixture.database.path()).await, before);

    let mut store = BoardStore::open(fixture.database.path())
        .await
        .expect("ordinary writer should apply migrations and backfill");
    let subscription_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM thread_subscriptions WHERE reader_key=? AND scope_id=?",
    )
    .bind(&fixture.reader_key)
    .bind(&fixture.root_id)
    .fetch_one(&mut store.connection)
    .await
    .expect("ordinary migration should create the matching subscription");
    assert_eq!(subscription_count, 1);
    let preserved_project: Option<String> =
        sqlx::query_scalar("SELECT name FROM board_projects WHERE project_id=?")
            .bind(&fixture.project_id)
            .fetch_optional(&mut store.connection)
            .await
            .expect("seeded project should remain readable");
    assert_eq!(preserved_project.as_deref(), Some("Project"));
    store.close().await.expect("ordinary writer should close");

    assert!(matches!(
        BoardStore::prepare_schema(fixture.database.path()).await,
        Ok(BoardSchemaPreparation::Current)
    ));
}

#[derive(Clone, Copy)]
enum HistoryDefect {
    Dirty,
    ChecksumMismatch,
    ChecksumStorageClass,
    NewerThanImage,
    UnknownMigration,
    InvalidOrder,
    InvalidVersion,
    InvalidVersionStorageClass,
    NullVersion,
    InvalidSuccess,
    InvalidSuccessStorageClass,
}

fn error_matches_defect(error: BoardSchemaPreparationError, defect: HistoryDefect) -> bool {
    use BoardSchemaPreparationError as Error;
    match defect {
        HistoryDefect::Dirty => matches!(error, Error::DirtyMigration),
        HistoryDefect::ChecksumMismatch => matches!(error, Error::ChecksumMismatch),
        HistoryDefect::ChecksumStorageClass => matches!(
            error,
            Error::ChecksumMismatch | Error::UnreadableStore { .. }
        ),
        HistoryDefect::NewerThanImage => matches!(error, Error::SchemaNewerThanImage),
        HistoryDefect::UnknownMigration => matches!(error, Error::UnknownAppliedMigration),
        HistoryDefect::InvalidOrder => matches!(error, Error::InvalidAppliedOrder),
        HistoryDefect::InvalidVersion => matches!(error, Error::InvalidMigrationVersion),
        HistoryDefect::InvalidVersionStorageClass | HistoryDefect::NullVersion => {
            matches!(error, Error::InvalidMigrationVersion)
        }
        HistoryDefect::InvalidSuccessStorageClass => {
            matches!(error, Error::InvalidMigrationHistory)
        }
        HistoryDefect::InvalidSuccess => matches!(error, Error::InvalidMigrationHistory),
    }
}

#[tokio::test]
async fn prepare_schema_rejects_invalid_native_history_without_repair() {
    for (index, defect) in [
        HistoryDefect::Dirty,
        HistoryDefect::ChecksumMismatch,
        HistoryDefect::ChecksumStorageClass,
        HistoryDefect::NewerThanImage,
        HistoryDefect::UnknownMigration,
        HistoryDefect::InvalidOrder,
        HistoryDefect::InvalidVersion,
        HistoryDefect::InvalidVersionStorageClass,
        HistoryDefect::NullVersion,
        HistoryDefect::InvalidSuccess,
        HistoryDefect::InvalidSuccessStorageClass,
    ]
    .into_iter()
    .enumerate()
    {
        let database = create_full_native_database(&format!("invalid_history_{index}")).await;
        apply_prefix_defect_database(database.path(), defect).await;
        let before = capture_snapshot(database.path()).await;

        let error = BoardStore::prepare_schema(database.path())
            .await
            .expect_err("invalid native history must be rejected");
        let error_detail = format!("{error:?}");
        assert!(
            error_matches_defect(error, defect),
            "history defect {index} should map to its explicit preparation error: {error_detail}"
        );
        assert_eq!(capture_snapshot(database.path()).await, before);
    }
}

#[tokio::test]
async fn prepare_schema_rejects_damaged_current_schema_without_repair() {
    let database = create_full_native_database("damaged_schema").await;
    let mut connection = open_writer(database.path(), false).await;
    sqlx::query("DROP TABLE actor_board_cooldowns")
        .execute(&mut connection)
        .await
        .expect("one current schema object should be removed");
    connection
        .close()
        .await
        .expect("damaged schema connection should close");
    let before = capture_snapshot(database.path()).await;

    let error = BoardStore::prepare_schema(database.path())
        .await
        .expect_err("current history with damaged schema must be rejected");

    assert!(matches!(
        error,
        BoardSchemaPreparationError::InvalidCurrentSchema { .. }
    ));
    assert_eq!(capture_snapshot(database.path()).await, before);
}

#[tokio::test]
async fn prepare_schema_rejects_current_foreign_key_damage_without_repair() {
    let database = create_full_native_database("foreign_key_damage").await;
    let mut connection = open_writer(database.path(), false).await;
    sqlx::query(
        "INSERT INTO board_topics(topic_id,board_id,name,description)
         VALUES('orphan-topic','missing-board','Orphan','')",
    )
    .execute(&mut connection)
    .await
    .expect("fixture connection leaves foreign-key enforcement off");
    connection
        .close()
        .await
        .expect("foreign-key fixture connection should close");
    let before = capture_snapshot(database.path()).await;

    let error = BoardStore::prepare_schema(database.path())
        .await
        .expect_err("foreign-key damage must be rejected");

    assert!(matches!(
        error,
        BoardSchemaPreparationError::InvalidCurrentSchema { .. }
    ));
    assert_eq!(capture_snapshot(database.path()).await, before);
}

#[tokio::test]
async fn prepare_schema_rejects_invalid_activity_checkpoint_without_repair() {
    let database = create_full_native_database("invalid_checkpoint").await;
    let mut connection = open_writer(database.path(), false).await;
    sqlx::query("UPDATE activity_checkpoint SET last_sequence=-1 WHERE singleton=1")
        .execute(&mut connection)
        .await
        .expect("checkpoint fixture should become invalid");
    connection
        .close()
        .await
        .expect("checkpoint fixture connection should close");
    let before = capture_snapshot(database.path()).await;

    let error = BoardStore::prepare_schema(database.path())
        .await
        .expect_err("invalid activity checkpoint must be rejected");

    assert!(matches!(
        error,
        BoardSchemaPreparationError::InvalidCurrentSchema { .. }
    ));
    assert_eq!(capture_snapshot(database.path()).await, before);
}

#[test]
fn board_migration_version_accepts_only_positive_values() {
    assert_eq!(
        BoardMigrationVersion::new(1).map(BoardMigrationVersion::get),
        Some(1)
    );
    assert!(BoardMigrationVersion::new(0).is_none());
    assert!(BoardMigrationVersion::new(-1).is_none());
}

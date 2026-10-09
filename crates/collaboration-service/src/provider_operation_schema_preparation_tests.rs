#![allow(clippy::unwrap_used)]

use crate::{
    ProviderOperationMigrationVersion, ProviderOperationSchemaPreparation,
    ProviderOperationSchemaPreparationError, ProviderOperationStore,
};
use collaboration_protocol::{
    OperationId, ProviderOperationEffect, ProviderOperationStage, ProviderReconciliationState,
};
use sqlx::Connection;

#[path = "provider_operation_schema_preparation_test_support.rs"]
mod support;

use support::{
    SEEDED_OPERATION_ID, apply_history_defect, capture_snapshot,
    commit_schema_change_from_wal_writer, create_full_native_database, create_legacy_database,
    create_native_prefix_database, create_wal_current_native_database, open_writer,
    replace_native_history_with_view,
};

#[tokio::test]
async fn prepare_schema_does_not_create_a_missing_database() {
    let database_path = std::env::temp_dir().join(format!(
        "provider-operation-schema-preparation-missing-{}.sqlite",
        uuid::Uuid::now_v7()
    ));
    assert!(!database_path.exists());

    let error = ProviderOperationStore::prepare_schema(&database_path)
        .await
        .expect_err("missing database must be rejected");

    assert!(matches!(
        error,
        ProviderOperationSchemaPreparationError::UnreadableStore { .. }
    ));
    assert!(!database_path.exists());
}

#[tokio::test]
async fn prepare_schema_reports_the_exact_native_prefix_without_mutation() {
    let database = create_native_prefix_database("native_prefix").await;
    let before = capture_snapshot(database.path()).await;
    assert!(!before.terminal_stop_reason_exists);
    assert!(before.operation_row.is_some());

    let preparation = ProviderOperationStore::prepare_schema(database.path())
        .await
        .expect("actual provider-operation prefix should prepare");
    match preparation {
        ProviderOperationSchemaPreparation::Pending { migrations } => {
            assert_eq!(
                migrations
                    .as_slice()
                    .iter()
                    .map(|version| version.get())
                    .collect::<Vec<_>>(),
                vec![202609250001]
            );
        }
        ProviderOperationSchemaPreparation::Current => {
            panic!("native prefix must report its exact remaining migration");
        }
    }

    assert_eq!(capture_snapshot(database.path()).await, before);
}

#[tokio::test]
async fn prepare_schema_reports_current_without_mutating_the_database() {
    let database = create_full_native_database("current").await;
    let before = capture_snapshot(database.path()).await;

    assert!(matches!(
        ProviderOperationStore::prepare_schema(database.path()).await,
        Ok(ProviderOperationSchemaPreparation::Current)
    ));

    assert_eq!(capture_snapshot(database.path()).await, before);
}

#[tokio::test]
async fn prepare_schema_keeps_history_and_current_validation_in_one_wal_snapshot() {
    let database = create_wal_current_native_database("wal_snapshot").await;
    let database_path = database.path().to_path_buf();
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
            "ALTER TABLE provider_operations ADD COLUMN snapshot_probe TEXT",
        )
        .await;
        writer_result_sender
            .send(result.clone())
            .expect("inspector should receive writer completion");
        result
    });

    let preparation = crate::provider_operation_schema_preparation::prepare_schema_with_checkpoint(
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
        matches!(
            &preparation,
            Ok(ProviderOperationSchemaPreparation::Current)
        ),
        "first inspection should use its original SQLite snapshot: {preparation:?}"
    );

    let after_writer_commit = capture_snapshot(&database_path).await;
    assert_eq!(
        after_writer_commit.migration_history,
        before.migration_history
    );
    assert_eq!(after_writer_commit.user_version, before.user_version);
    assert_eq!(
        after_writer_commit.terminal_stop_reason_exists,
        before.terminal_stop_reason_exists
    );
    assert_eq!(after_writer_commit.operation_row, before.operation_row);
    assert!(
        after_writer_commit
            .schema_objects
            .iter()
            .any(|(_, name, _, sql)| {
                name == "provider_operations"
                    && sql
                        .as_deref()
                        .is_some_and(|definition| definition.contains("snapshot_probe"))
            })
    );

    let error = ProviderOperationStore::prepare_schema(&database_path)
        .await
        .expect_err("a later inspection should observe the committed schema change");
    assert!(matches!(
        error,
        ProviderOperationSchemaPreparationError::InvalidCurrentSchema { .. }
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
        after_later_inspection.terminal_stop_reason_exists,
        after_writer_commit.terminal_stop_reason_exists
    );
    assert_eq!(
        after_later_inspection.operation_row,
        after_writer_commit.operation_row
    );
}

#[tokio::test]
async fn prepare_schema_refuses_unrecognized_history_without_adopting_it() {
    let database = create_legacy_database("legacy").await;
    let before = std::fs::read(database.path()).expect("legacy database bytes should read");

    let error = ProviderOperationStore::prepare_schema(database.path())
        .await
        .expect_err("preparation must not create or adopt provider-operation history");

    assert!(matches!(
        error,
        ProviderOperationSchemaPreparationError::UnrecognizedNativeHistory
    ));
    assert_eq!(
        std::fs::read(database.path()).expect("legacy database bytes should remain readable"),
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

    let preparation = ProviderOperationStore::prepare_schema(database.path()).await;

    assert!(
        matches!(
            &preparation,
            Err(ProviderOperationSchemaPreparationError::UnrecognizedNativeHistory)
        ),
        "a view cannot stand in for the native SQLx history table: {preparation:?}"
    );
    assert_eq!(capture_snapshot(database.path()).await, before);
}

#[tokio::test]
async fn prepare_schema_pending_result_connects_to_the_ordinary_writer() {
    let database = create_native_prefix_database("prepare_then_writer").await;
    let before = capture_snapshot(database.path()).await;

    assert!(matches!(
        ProviderOperationStore::prepare_schema(database.path()).await,
        Ok(ProviderOperationSchemaPreparation::Pending { .. })
    ));
    assert_eq!(capture_snapshot(database.path()).await, before);

    let operation_id = OperationId::try_from(SEEDED_OPERATION_ID.to_owned())
        .expect("fixture operation identity should be valid");
    let mut store = ProviderOperationStore::open(database.path())
        .await
        .expect("ordinary writer should apply the pending migration");
    let record = store
        .inspect(&operation_id)
        .await
        .expect("seeded operation should inspect")
        .expect("seeded operation should remain");
    assert_eq!(record.stage, ProviderOperationStage::Admitted);
    assert_eq!(record.effect, ProviderOperationEffect::None);
    assert_eq!(
        record.reconciliation_state,
        ProviderReconciliationState::Unresolved
    );
    assert_eq!(record.admitted_at_ms, 1_000);
    assert_eq!(record.updated_at_ms, 1_000);
    assert_eq!(record.terminal_stop_reason, None);
    store.close().await.expect("ordinary writer should close");

    assert!(matches!(
        ProviderOperationStore::prepare_schema(database.path()).await,
        Ok(ProviderOperationSchemaPreparation::Current)
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

fn error_matches_defect(
    error: ProviderOperationSchemaPreparationError,
    defect: HistoryDefect,
) -> bool {
    use ProviderOperationSchemaPreparationError as Error;
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
        HistoryDefect::InvalidSuccess | HistoryDefect::InvalidSuccessStorageClass => {
            matches!(error, Error::InvalidMigrationHistory)
        }
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
        apply_history_defect(database.path(), defect).await;
        let before = capture_snapshot(database.path()).await;

        let error = ProviderOperationStore::prepare_schema(database.path())
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
    sqlx::query("ALTER TABLE provider_operations DROP COLUMN terminal_stop_reason")
        .execute(&mut connection)
        .await
        .expect("current schema column should be damaged");
    connection
        .close()
        .await
        .expect("damaged schema fixture should close");
    let before = capture_snapshot(database.path()).await;

    let error = ProviderOperationStore::prepare_schema(database.path())
        .await
        .expect_err("current history with damaged target schema must be rejected");

    assert!(matches!(
        error,
        ProviderOperationSchemaPreparationError::InvalidCurrentSchema { .. }
    ));
    assert_eq!(capture_snapshot(database.path()).await, before);
}

#[test]
fn provider_operation_migration_version_accepts_only_positive_values() {
    assert_eq!(
        ProviderOperationMigrationVersion::new(1).map(ProviderOperationMigrationVersion::get),
        Some(1)
    );
    assert!(ProviderOperationMigrationVersion::new(0).is_none());
    assert!(ProviderOperationMigrationVersion::new(-1).is_none());
}

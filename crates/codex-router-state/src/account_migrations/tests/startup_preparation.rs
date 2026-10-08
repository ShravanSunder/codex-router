use super::*;
use crate::schema_preparation::{
    AccountBootstrapKind, AccountSchemaPreparation, AccountStartupSchemaPreparation,
    StateSchemaPreparationError,
};
#[derive(Debug, Eq, PartialEq)]
struct StateSnapshot {
    bytes: Vec<u8>,
    schema: Vec<(String, String, String, Option<String>)>,
    version: i64,
}
async fn state_snapshot(path: &Path) -> StateSnapshot {
    let bytes = std::fs::read(path).expect("real state bytes");
    let mut connection = open_test_connection(path, false).await;
    let schema =
        sqlx::query_as("SELECT type,name,tbl_name,sql FROM sqlite_schema ORDER BY type,name")
            .fetch_all(&mut connection)
            .await
            .expect("literal schema snapshot");
    let version = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(&mut connection)
        .await
        .expect("literal version snapshot");
    connection.close().await.expect("snapshot closes");
    StateSnapshot {
        bytes,
        schema,
        version,
    }
}
#[tokio::test]
async fn startup_prepares_existing_empty_and_recognized_legacy_without_mutation() {
    for case in 0..10 {
        let database = TemporaryDatabase::new(&format!("startup_recognized_{case}"));
        match case {
            0 => {
                let connection = open_test_connection(database.path(), true).await;
                connection.close().await.expect("empty DB closes");
            }
            1 => create_legacy_v7_database(database.path()).await,
            2 => create_legacy_v7_current_lease_database(database.path()).await,
            3 => create_legacy_v7_without_lease_table(database.path()).await,
            4 => {
                create_legacy_v7_current_lease_database(database.path()).await;
                execute_fixture_sql(database.path(), "PRAGMA user_version=8").await;
            }
            5 => create_legacy_v9_history_shape(database.path(), 0).await,
            6 => {
                create_legacy_v7_current_lease_database(database.path()).await;
                execute_fixture_sql(database.path(), "PRAGMA user_version=10").await;
            }
            7 => create_legacy_v11_database(database.path()).await,
            8 => {
                create_legacy_v13_database(database.path(), "v12-preserved").await;
                execute_fixture_sql(
                    database.path(),
                    "DROP TABLE session_account_affinities; PRAGMA user_version=12",
                )
                .await;
            }
            9 => create_legacy_v13_database(database.path(), "v13-preserved").await,
            _ => panic!("bounded fixture case"),
        }
        let before = state_snapshot(database.path()).await;
        let preparation = AsyncSqliteStateStore::prepare_startup_schema(database.path())
            .await
            .expect("existing bootstrap owner accepts known shape");
        assert_eq!(
            preparation,
            AccountStartupSchemaPreparation::Bootstrap {
                kind: if case == 0 {
                    AccountBootstrapKind::ExistingEmpty
                } else {
                    AccountBootstrapKind::RecognizedLegacy
                }
            }
        );
        assert_eq!(state_snapshot(database.path()).await, before);
        assert!(
            matches!(
                AsyncSqliteStateStore::prepare_schema(database.path()).await,
                Err(StateSchemaPreparationError::UnrecognizedNativeHistory)
            ),
            "Replacement/native contract remains strict"
        );
    }
}
#[tokio::test]
async fn startup_reuses_validation_for_every_version_zero_initialization_prefix() {
    for prefix in 0..=LEGACY_V0_TABLE_STATEMENTS.len() {
        let database = TemporaryDatabase::new(&format!("startup_v0_prefix_{prefix}"));
        let mut connection = open_test_connection(database.path(), true).await;
        for statement in &LEGACY_V0_TABLE_STATEMENTS[..prefix] {
            sqlx::raw_sql(*statement)
                .execute(&mut connection)
                .await
                .expect("existing initialization prefix");
        }
        connection.close().await.expect("prefix fixture closes");
        let before = state_snapshot(database.path()).await;
        let kind = if prefix == 0 {
            AccountBootstrapKind::ExistingEmpty
        } else {
            AccountBootstrapKind::RecognizedLegacy
        };
        assert_eq!(
            AsyncSqliteStateStore::prepare_startup_schema(database.path())
                .await
                .expect("known prefix"),
            AccountStartupSchemaPreparation::Bootstrap { kind }
        );
        assert_eq!(state_snapshot(database.path()).await, before);
    }
}
#[tokio::test]
async fn startup_refuses_invalid_bootstrap_shapes_without_repair() {
    for case in 0..3 {
        let database = TemporaryDatabase::new(&format!("startup_invalid_{case}"));
        match case {
            0 => create_conflicting_v13_database(database.path()).await,
            1 => {
                create_legacy_v13_database(database.path(), "future").await;
                execute_fixture_sql(database.path(), "PRAGMA user_version=99").await;
            }
            2 => {
                create_legacy_v13_database(database.path(), "leftover").await;
                execute_fixture_sql(
                    database.path(),
                    "CREATE TABLE account_routing_policies_v12 (value TEXT)",
                )
                .await;
            }
            _ => panic!("bounded fixture case"),
        }
        let before = state_snapshot(database.path()).await;
        assert!(matches!(
            AsyncSqliteStateStore::prepare_startup_schema(database.path()).await,
            Err(StateSchemaPreparationError::InvalidBootstrapSchema { .. })
        ));
        assert_eq!(state_snapshot(database.path()).await, before);
    }
}
#[tokio::test]
async fn startup_preserves_native_pending_current_and_error_rules_without_bootstrap_fallback() {
    let database = TemporaryDatabase::new("startup_native");
    let mut connection = open_test_connection(database.path(), true).await;
    let migrator = sqlx::migrate::Migrator {
        migrations: std::borrow::Cow::Owned(
            MIGRATOR
                .iter()
                .cloned()
                .rev()
                .skip(1)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect(),
        ),
        ..sqlx::migrate::Migrator::DEFAULT
    };
    migrator
        .run(&mut connection)
        .await
        .expect("real native prefix");
    connection.close().await.expect("prefix closes");
    let before = state_snapshot(database.path()).await;
    let strict = AsyncSqliteStateStore::prepare_schema(database.path())
        .await
        .expect("strict prefix");
    assert!(matches!(strict, AccountSchemaPreparation::Pending { .. }));
    assert_eq!(
        AsyncSqliteStateStore::prepare_startup_schema(database.path())
            .await
            .expect("native Pending"),
        AccountStartupSchemaPreparation::Native { schema: strict }
    );
    assert_eq!(state_snapshot(database.path()).await, before);
    let writer = AsyncSqliteStateStore::open(database.path())
        .await
        .expect("existing migration engine");
    writer.close().await.expect("writer closes");
    assert_eq!(
        AsyncSqliteStateStore::prepare_startup_schema(database.path())
            .await
            .expect("native Current"),
        AccountStartupSchemaPreparation::Native {
            schema: AccountSchemaPreparation::Current
        }
    );
    for (sql, expected) in [
        ("UPDATE _sqlx_migrations SET success=0", "dirty"),
        ("UPDATE _sqlx_migrations SET checksum=X'00'", "checksum"),
        (
            "INSERT INTO _sqlx_migrations(version,description,success,checksum,execution_time) VALUES(209901010001,'newer',1,X'00',1)",
            "newer",
        ),
        (
            "DELETE FROM _sqlx_migrations WHERE version=(SELECT MIN(version) FROM _sqlx_migrations)",
            "order",
        ),
    ] {
        let database = TemporaryDatabase::new(&format!("startup_native_{expected}"));
        let writer = AsyncSqliteStateStore::open(database.path())
            .await
            .expect("independent current native fixture");
        writer.close().await.expect("independent fixture closes");
        execute_fixture_sql(database.path(), sql).await;
        let before = state_snapshot(database.path()).await;
        let error = AsyncSqliteStateStore::prepare_startup_schema(database.path())
            .await
            .expect_err("invalid native never falls back");
        assert!(matches!(
            (expected, error),
            ("dirty", StateSchemaPreparationError::DirtyMigration)
                | ("checksum", StateSchemaPreparationError::ChecksumMismatch)
                | ("newer", StateSchemaPreparationError::SchemaNewerThanImage)
                | ("order", StateSchemaPreparationError::InvalidAppliedOrder)
        ));
        assert_eq!(state_snapshot(database.path()).await, before);
    }
}
#[tokio::test]
async fn startup_never_creates_a_missing_database() {
    let database = TemporaryDatabase::new("startup_missing");
    assert!(!database.path().exists());
    assert!(matches!(
        AsyncSqliteStateStore::prepare_startup_schema(database.path()).await,
        Err(StateSchemaPreparationError::UnreadableStore { .. })
    ));
    assert!(!database.path().exists());
}

#[tokio::test]
async fn startup_preserves_the_existing_empty_early_branch_and_actionable_errors() {
    let database = TemporaryDatabase::new("startup_empty_early_branch");
    let connection = open_test_connection(database.path(), true).await;
    connection.close().await.expect("empty fixture closes");
    execute_fixture_sql(
        database.path(),
        "CREATE TABLE account_routing_policies_v12(value TEXT)",
    )
    .await;
    let before = state_snapshot(database.path()).await;
    assert_eq!(
        AsyncSqliteStateStore::prepare_startup_schema(database.path())
            .await
            .expect("same empty branch as migration owner"),
        AccountStartupSchemaPreparation::Bootstrap {
            kind: AccountBootstrapKind::ExistingEmpty
        }
    );
    assert_eq!(state_snapshot(database.path()).await, before);
    let database = TemporaryDatabase::new("startup_actionable_version_error");
    let connection = open_test_connection(database.path(), true).await;
    connection
        .close()
        .await
        .expect("unsupported fixture closes");
    execute_fixture_sql(database.path(), "PRAGMA user_version=6").await;
    let error = AsyncSqliteStateStore::prepare_startup_schema(database.path())
        .await
        .expect_err("unsupported version remains actionable");
    let StateSchemaPreparationError::InvalidBootstrapSchema { source } = &error else {
        panic!("bootstrap owner error expected: {error}");
    };
    assert_eq!(
        *source,
        crate::sqlite::StateStoreError::UnsupportedSchemaVersion { version: 6 }
    );
    assert_eq!(error.to_string(), source.to_string());
}

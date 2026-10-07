#![allow(clippy::unwrap_used)]

use collaboration_protocol::{
    CodexGeneration, ConversationBindingIdentity, EndpointId, EndpointRef, GenerationNumber,
    NonEmptyText, OperationId, ProviderBindingId, ProviderBindingIdentity, ProviderCapabilities,
    ProviderCapability, ProviderCapabilityEvidence, ProviderCapabilityName,
    ProviderCapabilityStatus, ProviderKind, ProviderOperationEffect, ProviderOperationKind,
    ProviderOperationStage, ProviderReconciliationState, ProviderRuntimeIdentity,
    ProviderTransport, UuidIdentity,
};
use sqlx::{
    Connection, Row, SqliteConnection,
    migrate::Migrator,
    sqlite::{SqliteConnectOptions, SqliteJournalMode},
};
use std::{borrow::Cow, path::PathBuf};

pub(super) const SEEDED_OPERATION_ID: &str = "018f1f62-6571-7ef0-8f0c-001122334499";

pub(super) struct TemporaryProviderOperationDatabase {
    path: PathBuf,
}

impl TemporaryProviderOperationDatabase {
    pub(super) fn new(label: &str) -> Self {
        Self {
            path: std::env::temp_dir().join(format!(
                "provider-operation-schema-preparation-{label}-{}.sqlite",
                uuid::Uuid::now_v7()
            )),
        }
    }

    pub(super) fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl Drop for TemporaryProviderOperationDatabase {
    fn drop(&mut self) {
        for suffix in ["", "-journal", "-wal", "-shm"] {
            let sidecar = format!("{}{suffix}", self.path.display());
            let _ = std::fs::remove_file(sidecar);
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(super) struct DatabaseSnapshot {
    pub(super) main_database_bytes: Vec<u8>,
    pub(super) migration_history: Vec<(String, String, String, String, String, String)>,
    pub(super) schema_objects: Vec<(String, String, String, Option<String>)>,
    pub(super) user_version: i64,
    pub(super) terminal_stop_reason_exists: bool,
    pub(super) operation_row: Option<ProviderOperationRowSnapshot>,
}

#[derive(Debug, Eq, PartialEq, sqlx::FromRow)]
pub(super) struct ProviderOperationRowSnapshot {
    pub(super) operation_id: String,
    pub(super) operation_kind: String,
    pub(super) binding_json: String,
    pub(super) stage: String,
    pub(super) effect: String,
    pub(super) reconciliation_state: String,
    pub(super) admitted_at_ms: i64,
    pub(super) dispatched_at_ms: Option<i64>,
    pub(super) terminal_at_ms: Option<i64>,
    pub(super) updated_at_ms: i64,
}

pub(super) async fn create_native_prefix_database(
    label: &str,
) -> TemporaryProviderOperationDatabase {
    let database = TemporaryProviderOperationDatabase::new(label);
    let mut connection = open_writer(database.path(), true).await;
    let migrations = crate::provider_operation_store::MIGRATOR
        .iter()
        .take(crate::provider_operation_store::MIGRATOR.iter().count() - 1)
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(
        migrations.last().map(|migration| migration.version),
        Some(202609240002),
        "fixture must stop before terminal stop-reason migration"
    );
    let prefix_migrator = Migrator {
        migrations: Cow::Owned(migrations),
        ..Migrator::DEFAULT
    };
    prefix_migrator
        .run(&mut connection)
        .await
        .expect("actual provider-operation migration prefix should apply");
    insert_seeded_operation(&mut connection).await;
    connection
        .close()
        .await
        .expect("native-prefix fixture connection should close");
    database
}

pub(super) async fn create_full_native_database(label: &str) -> TemporaryProviderOperationDatabase {
    let database = TemporaryProviderOperationDatabase::new(label);
    let mut connection = open_writer(database.path(), true).await;
    crate::provider_operation_store::MIGRATOR
        .run(&mut connection)
        .await
        .expect("actual full provider-operation migration set should apply");
    insert_seeded_operation(&mut connection).await;
    connection
        .close()
        .await
        .expect("full-schema fixture connection should close");
    database
}

pub(super) async fn create_wal_current_native_database(
    label: &str,
) -> TemporaryProviderOperationDatabase {
    let database = create_native_prefix_database(label).await;
    let mut store = crate::ProviderOperationStore::open(database.path())
        .await
        .expect("ordinary provider-operation writer should complete the current schema");
    let journal_mode: String = sqlx::query_scalar("PRAGMA journal_mode")
        .fetch_one(&mut store.connection)
        .await
        .expect("provider-operation journal mode should query");
    assert_eq!(journal_mode.to_ascii_lowercase(), "wal");
    store
        .close()
        .await
        .expect("ordinary provider-operation writer should close");
    database
}

pub(super) async fn commit_schema_change_from_wal_writer(
    path: &std::path::Path,
    statement: &'static str,
) -> Result<(), String> {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(false)
        .foreign_keys(false)
        .journal_mode(SqliteJournalMode::Wal);
    let mut connection = SqliteConnection::connect_with(&options)
        .await
        .map_err(|error| error.to_string())?;
    let mut transaction = connection
        .begin()
        .await
        .map_err(|error| error.to_string())?;
    if let Err(error) = sqlx::query(statement).execute(&mut *transaction).await {
        let _ = transaction.rollback().await;
        let _ = connection.close().await;
        return Err(error.to_string());
    }
    transaction
        .commit()
        .await
        .map_err(|error| error.to_string())?;
    connection.close().await.map_err(|error| error.to_string())
}

async fn insert_seeded_operation(connection: &mut SqliteConnection) {
    let operation_id = OperationId::try_from(SEEDED_OPERATION_ID.to_owned())
        .expect("seeded operation identity should be valid");
    let operation_kind =
        crate::provider_operation_store::encode_enum(ProviderOperationKind::ConversationPrompt)
            .expect("operation kind should encode");
    let binding_json =
        serde_json::to_string(&seeded_binding()).expect("operation binding should encode");
    let stage = crate::provider_operation_store::encode_enum(ProviderOperationStage::Admitted)
        .expect("operation stage should encode");
    let effect = crate::provider_operation_store::encode_enum(ProviderOperationEffect::None)
        .expect("operation effect should encode");
    let reconciliation_state =
        crate::provider_operation_store::encode_enum(ProviderReconciliationState::Unresolved)
            .expect("reconciliation state should encode");
    sqlx::query(
        "INSERT INTO provider_operations(
            operation_id,operation_kind,binding_json,
            target_service_id,target_endpoint_id,target_session_id,
            stage,effect,reconciliation_state,admitted_at_ms,
            dispatched_at_ms,terminal_at_ms,updated_at_ms
         ) VALUES(?,?,?,NULL,NULL,NULL,?,?,?,?,NULL,NULL,?)",
    )
    .bind(operation_id.as_str())
    .bind(operation_kind)
    .bind(binding_json)
    .bind(stage)
    .bind(effect)
    .bind(reconciliation_state)
    .bind(1_000_i64)
    .bind(1_000_i64)
    .execute(&mut *connection)
    .await
    .expect("seeded provider operation should insert");
}

fn seeded_binding() -> ConversationBindingIdentity {
    let service_id = UuidIdentity::try_from("0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89".to_owned())
        .expect("service identity should be valid");
    let endpoint_id =
        EndpointId::try_from("claude-local".to_owned()).expect("endpoint identity should be valid");
    let binding_id = ProviderBindingId::try_from("claude-bridge-1".to_owned())
        .expect("binding identity should be valid");
    let service_epoch = UuidIdentity::try_from("1ff962c5-7fa3-4c18-a5ca-1bbe8db09e80".to_owned())
        .expect("service epoch should be valid");
    ConversationBindingIdentity::ExternalProvider {
        binding: ProviderBindingIdentity {
            endpoint: EndpointRef {
                service_id,
                endpoint_id,
            },
            binding_id,
            runtime: ProviderRuntimeIdentity {
                provider: ProviderKind::ClaudeCode,
                runtime_name: NonEmptyText::try_from("claude-agent-acp".to_owned())
                    .expect("runtime name should be valid"),
                runtime_version: Some(
                    NonEmptyText::try_from("1.2.3".to_owned())
                        .expect("runtime version should be valid"),
                ),
            },
            transport: ProviderTransport::StdioAcp,
            generation: CodexGeneration {
                service_epoch,
                generation: GenerationNumber::try_from(7)
                    .expect("provider generation should be valid"),
            },
            capabilities: ProviderCapabilities::try_from(vec![ProviderCapability {
                name: ProviderCapabilityName::Prompt,
                status: ProviderCapabilityStatus::Supported,
                evidence: ProviderCapabilityEvidence::Advertised,
            }])
            .expect("provider capabilities should be valid"),
        },
    }
}

pub(super) async fn create_legacy_database(label: &str) -> TemporaryProviderOperationDatabase {
    let database = TemporaryProviderOperationDatabase::new(label);
    let mut connection = open_writer(database.path(), true).await;
    sqlx::raw_sql(
        "CREATE TABLE legacy_provider_operations(value TEXT NOT NULL); PRAGMA user_version=1;",
    )
    .execute(&mut connection)
    .await
    .expect("legacy database should seed");
    sqlx::query("INSERT INTO legacy_provider_operations(value) VALUES('preserve-me')")
        .execute(&mut connection)
        .await
        .expect("legacy row should seed");
    connection
        .close()
        .await
        .expect("legacy fixture connection should close");
    database
}

pub(super) async fn replace_native_history_with_view(path: &std::path::Path) {
    let mut connection = open_writer(path, false).await;
    let history_rows: Vec<(i64, String, i64, Vec<u8>, i64)> = sqlx::query_as(
        "SELECT version,description,success,checksum,execution_time
         FROM _sqlx_migrations ORDER BY version",
    )
    .fetch_all(&mut connection)
    .await
    .expect("authentic migration rows should query before replacing the table");
    let selects = history_rows
        .into_iter()
        .map(|(version, description, success, checksum, execution_time)| {
            let description = description.replace('\'', "''");
            let checksum = checksum
                .iter()
                .map(|byte| format!("{byte:02X}"))
                .collect::<String>();
            format!(
                "SELECT {version} AS version,'{description}' AS description,{success} AS success,X'{checksum}' AS checksum,{execution_time} AS execution_time"
            )
        })
        .collect::<Vec<_>>();
    sqlx::query("DROP TABLE _sqlx_migrations")
        .execute(&mut connection)
        .await
        .expect("native history table should be replaced in the fixture");
    let view_sql = format!(
        "CREATE VIEW _sqlx_migrations AS {}",
        selects.join(" UNION ALL ")
    );
    sqlx::raw_sql(sqlx::AssertSqlSafe(view_sql))
        .execute(&mut connection)
        .await
        .expect("fixture view should project the authentic migration rows");
    connection
        .close()
        .await
        .expect("history view fixture should close");
}

pub(super) async fn open_writer(path: &std::path::Path, create: bool) -> SqliteConnection {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(create)
        .foreign_keys(false)
        .journal_mode(SqliteJournalMode::Delete);
    SqliteConnection::connect_with(&options)
        .await
        .expect("test database should open")
}

pub(super) async fn capture_snapshot(path: &std::path::Path) -> DatabaseSnapshot {
    let main_database_bytes = std::fs::read(path).expect("main database bytes should read");
    let options = SqliteConnectOptions::new()
        .filename(path)
        .read_only(true)
        .create_if_missing(false)
        .pragma("query_only", "ON");
    let mut connection = SqliteConnection::connect_with(&options)
        .await
        .expect("snapshot connection should open read-only");
    let migration_history = sqlx::query_as(
        "SELECT typeof(version),CAST(version AS TEXT),typeof(success),CAST(success AS TEXT),
                typeof(checksum),hex(checksum)
         FROM _sqlx_migrations ORDER BY version",
    )
    .fetch_all(&mut connection)
    .await
    .expect("raw migration history should query");
    let schema_objects =
        sqlx::query_as("SELECT type,name,tbl_name,sql FROM sqlite_schema ORDER BY type,name")
            .fetch_all(&mut connection)
            .await
            .expect("schema objects should query");
    let user_version = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(&mut connection)
        .await
        .expect("user version should query");
    let columns = sqlx::query("PRAGMA table_info(provider_operations)")
        .fetch_all(&mut connection)
        .await
        .expect("provider operation columns should query")
        .into_iter()
        .map(|row| row.try_get::<String, _>("name"))
        .collect::<Result<Vec<_>, _>>()
        .expect("provider operation column names should decode");
    let terminal_stop_reason_exists = columns
        .iter()
        .any(|column| column == "terminal_stop_reason");
    let operation_row: Option<ProviderOperationRowSnapshot> = sqlx::query_as(
        "SELECT operation_id,operation_kind,binding_json,stage,effect,
                reconciliation_state,admitted_at_ms,dispatched_at_ms,terminal_at_ms,updated_at_ms
         FROM provider_operations WHERE operation_id=?",
    )
    .bind(SEEDED_OPERATION_ID)
    .fetch_optional(&mut connection)
    .await
    .expect("seeded operation should query");
    connection
        .close()
        .await
        .expect("snapshot connection should close");
    DatabaseSnapshot {
        main_database_bytes,
        migration_history,
        schema_objects,
        user_version,
        terminal_stop_reason_exists,
        operation_row,
    }
}

pub(super) async fn apply_history_defect(path: &std::path::Path, defect: super::HistoryDefect) {
    let mut connection = open_writer(path, false).await;
    let first_version = crate::provider_operation_store::MIGRATOR
        .iter()
        .next()
        .expect("image has an initial migration")
        .version;
    let newest_version = crate::provider_operation_store::MIGRATOR
        .iter()
        .last()
        .expect("image has a newest migration")
        .version;
    if matches!(
        defect,
        super::HistoryDefect::InvalidVersionStorageClass
            | super::HistoryDefect::NullVersion
            | super::HistoryDefect::InvalidSuccessStorageClass
    ) {
        rebuild_migration_history_table(&mut connection).await;
    }
    match defect {
        super::HistoryDefect::Dirty => {
            sqlx::query("UPDATE _sqlx_migrations SET success=0 WHERE version=?")
                .bind(first_version)
                .execute(&mut connection)
                .await
                .expect("migration should become dirty");
        }
        super::HistoryDefect::ChecksumMismatch => {
            sqlx::query("UPDATE _sqlx_migrations SET checksum=X'00' WHERE version=?")
                .bind(first_version)
                .execute(&mut connection)
                .await
                .expect("migration checksum should change");
        }
        super::HistoryDefect::ChecksumStorageClass => {
            sqlx::query("UPDATE _sqlx_migrations SET checksum='not-a-blob' WHERE version=?")
                .bind(first_version)
                .execute(&mut connection)
                .await
                .expect("migration checksum storage class should change");
        }
        super::HistoryDefect::NewerThanImage => {
            sqlx::query(
                "INSERT INTO _sqlx_migrations(version,description,success,checksum,execution_time)
                 VALUES(?,'future migration',1,X'00',1)",
            )
            .bind(newest_version + 1)
            .execute(&mut connection)
            .await
            .expect("newer migration should insert");
        }
        super::HistoryDefect::UnknownMigration => {
            sqlx::query(
                "INSERT INTO _sqlx_migrations(version,description,success,checksum,execution_time)
                 VALUES(202609220001,'unknown migration',1,X'00',1)",
            )
            .execute(&mut connection)
            .await
            .expect("unknown migration should insert");
        }
        super::HistoryDefect::InvalidOrder => {
            sqlx::query("DELETE FROM _sqlx_migrations WHERE version=?")
                .bind(first_version)
                .execute(&mut connection)
                .await
                .expect("native history should stop being a prefix");
        }
        super::HistoryDefect::InvalidVersion => {
            sqlx::query("UPDATE _sqlx_migrations SET version=0 WHERE version=?")
                .bind(first_version)
                .execute(&mut connection)
                .await
                .expect("migration version should become invalid");
        }
        super::HistoryDefect::InvalidVersionStorageClass => {
            sqlx::query("UPDATE _sqlx_migrations SET version='not-an-integer' WHERE version=?")
                .bind(first_version)
                .execute(&mut connection)
                .await
                .expect("migration version storage class should change");
        }
        super::HistoryDefect::NullVersion => {
            sqlx::query("UPDATE _sqlx_migrations SET version=NULL WHERE version=?")
                .bind(first_version)
                .execute(&mut connection)
                .await
                .expect("migration version should become NULL");
        }
        super::HistoryDefect::InvalidSuccess => {
            sqlx::query("UPDATE _sqlx_migrations SET success=2 WHERE version=?")
                .bind(first_version)
                .execute(&mut connection)
                .await
                .expect("migration success should become nonboolean");
        }
        super::HistoryDefect::InvalidSuccessStorageClass => {
            sqlx::query("UPDATE _sqlx_migrations SET success='not-a-boolean' WHERE version=?")
                .bind(first_version)
                .execute(&mut connection)
                .await
                .expect("migration success storage class should change");
        }
    }
    connection
        .close()
        .await
        .expect("corrupted history connection should close");
}

async fn rebuild_migration_history_table(connection: &mut SqliteConnection) {
    sqlx::query("DROP TABLE _sqlx_migrations")
        .execute(&mut *connection)
        .await
        .expect("native migration history table should drop in fixture");
    sqlx::query(
        "CREATE TABLE _sqlx_migrations(
            version INTEGER, description TEXT, success BOOLEAN, checksum BLOB, execution_time BIGINT
         )",
    )
    .execute(&mut *connection)
    .await
    .expect("fixture should recreate history with permissive test columns");
    for migration in crate::provider_operation_store::MIGRATOR.iter() {
        sqlx::query(
            "INSERT INTO _sqlx_migrations(version,description,success,checksum,execution_time)
             VALUES(?,?,1,?,0)",
        )
        .bind(migration.version)
        .bind(migration.description.as_ref())
        .bind(migration.checksum.as_ref())
        .execute(&mut *connection)
        .await
        .expect("real migration metadata should reseed history");
    }
}

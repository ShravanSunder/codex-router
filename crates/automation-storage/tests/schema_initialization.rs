use automation_storage::{AutomationStore, StorageError};
use sqlx::{Connection, Row, SqliteConnection};
use std::path::{Path, PathBuf};

const LEGACY_V1_SCHEMA: &str = include_str!("fixtures/automation_v1.sql");

macro_rules! ensure {
    ($condition:expr) => {
        if !$condition {
            return Err(format!("assertion failed: {}", stringify!($condition)).into());
        }
    };
    ($condition:expr, $context:expr) => {
        if !$condition {
            return Err(
                format!("assertion failed: {}: {}", stringify!($condition), $context).into(),
            );
        }
    };
}

macro_rules! ensure_eq {
    ($left:expr, $right:expr) => {{
        let left = &$left;
        let right = &$right;
        if left != right {
            return Err(format!("values differ: left={left:?}, right={right:?}").into());
        }
    }};
    ($left:expr, $right:expr, $context:expr) => {{
        let left = &$left;
        let right = &$right;
        if left != right {
            return Err(format!(
                "values differ: left={left:?}, right={right:?}: {}",
                $context
            )
            .into());
        }
    }};
}

struct TestDatabase {
    path: PathBuf,
}

impl TestDatabase {
    fn new(label: &str) -> Self {
        Self {
            path: std::env::temp_dir().join(format!(
                "automation-schema-{label}-{}.sqlite",
                uuid::Uuid::now_v7()
            )),
        }
    }
}

impl Drop for TestDatabase {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

async fn connect(path: &Path) -> Result<SqliteConnection, sqlx::Error> {
    SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(true),
    )
    .await
}

async fn create_legacy_v1(path: &Path, schema: &str) -> Result<(), sqlx::Error> {
    let mut connection = connect(path).await?;
    sqlx::raw_sql(sqlx::AssertSqlSafe(schema))
        .execute(&mut connection)
        .await?;
    sqlx::query("PRAGMA user_version=1")
        .execute(&mut connection)
        .await?;
    connection.close().await
}

async fn open_error(
    path: &Path,
    expectation: &str,
) -> Result<StorageError, Box<dyn std::error::Error>> {
    match AutomationStore::open(path).await {
        Err(error) => Ok(error),
        Ok(store) => {
            store.close().await?;
            Err(expectation.into())
        }
    }
}

async fn seed_all_domain_tables(path: &Path) -> Result<(), sqlx::Error> {
    let mut connection = connect(path).await?;
    let mut transaction = connection.begin().await?;
    sqlx::query("INSERT INTO instruction_documents VALUES ('instruction-1','revision-1','instruction bytes',101)").execute(&mut *transaction).await?;
    sqlx::query("INSERT INTO instruction_revisions VALUES ('revision-1','instruction-1','instruction bytes','source-revision',102)").execute(&mut *transaction).await?;
    sqlx::query("INSERT INTO schedule_definitions VALUES ('schedule-1','change-1','instruction-1',1,'{\"timing\":\"kept\"}','{\"summary\":null}',103,104)").execute(&mut *transaction).await?;
    sqlx::query("INSERT INTO schedule_timing_state VALUES ('schedule-1','change-1',105,106,107)")
        .execute(&mut *transaction)
        .await?;
    sqlx::query("INSERT INTO thread_bindings VALUES ('binding-1','schedule-1','service-1','endpoint-1','thread-1',108)").execute(&mut *transaction).await?;
    sqlx::query("INSERT INTO workflow_runs VALUES ('run-1','schedule-1',109,'uncertain','{\"input\":\"kept\"}','binding-1','turn-1',110,111,3600,'{\"nativeInputAccepted\":true}','{\"status\":\"kept\"}','{\"attempt\":1}',NULL,NULL,NULL)").execute(&mut *transaction).await?;
    sqlx::query("INSERT INTO wakeup_definitions VALUES ('wake-1','wake-change-1','active','{\"message\":\"kept\"}',112,113,114,115,'{\"fired\":true}',NULL,116,117)").execute(&mut *transaction).await?;
    sqlx::query("INSERT INTO mailbox_deliveries VALUES ('delivery-1','wake-1','occurrence-1',118,119,'{\"thread\":\"kept\"}','{\"text\":\"kept\"}','queue','{\"generation\":7}','uncertain',120,121,'{\"attempt\":2}',NULL,122)").execute(&mut *transaction).await?;
    sqlx::query(
        "UPDATE wakeup_definitions SET pending_delivery_id='delivery-1' WHERE wakeup_id='wake-1'",
    )
    .execute(&mut *transaction)
    .await?;
    sqlx::query("INSERT INTO operation_receipts VALUES ('operation-1','submit',x'0001FF','run-1','uncertain','{\"effect\":\"kept\"}',NULL,NULL,123)").execute(&mut *transaction).await?;
    sqlx::query("INSERT INTO automation_events(event_sequence,event_id,subject_kind,subject_id,event_kind,event_body_json,recorded_at_ms) VALUES (41,'event-1','run','run-1','uncertain','{\"evidence\":\"kept\"}',124)").execute(&mut *transaction).await?;
    sqlx::query("INSERT INTO automation_events(event_sequence,event_id,subject_kind,subject_id,event_kind,event_body_json,recorded_at_ms) VALUES (79,'event-high-water','run','run-1','temporary','{}',125)").execute(&mut *transaction).await?;
    sqlx::query("DELETE FROM automation_events WHERE event_id='event-high-water'")
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    connection.close().await
}

async fn assert_preserved_domain_state(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let mut connection = connect(path).await?;
    let instruction: (String, String, i64) = sqlx::query_as(
        "SELECT current_revision_id,instruction_text,updated_at_ms FROM instruction_documents WHERE instruction_id='instruction-1'",
    ).fetch_one(&mut connection).await?;
    ensure_eq!(
        instruction,
        ("revision-1".into(), "instruction bytes".into(), 101)
    );
    let revision: (String, Option<String>, i64) = sqlx::query_as("SELECT instruction_text,source_revision_id,recorded_at_ms FROM instruction_revisions WHERE revision_id='revision-1'").fetch_one(&mut connection).await?;
    ensure_eq!(
        revision,
        (
            "instruction bytes".into(),
            Some("source-revision".into()),
            102
        )
    );
    let schedule: (i64, String, String) = sqlx::query_as("SELECT enabled,definition_json,imported_continuity_json FROM schedule_definitions WHERE schedule_id='schedule-1'").fetch_one(&mut connection).await?;
    ensure_eq!(
        schedule,
        (
            1,
            "{\"timing\":\"kept\"}".into(),
            "{\"summary\":null}".into()
        )
    );
    let timing: (i64, i64, Option<i64>) = sqlx::query_as("SELECT anchor_at_ms,evaluated_through_ms,next_due_at_ms FROM schedule_timing_state WHERE schedule_id='schedule-1'").fetch_one(&mut connection).await?;
    ensure_eq!(timing, (105, 106, Some(107)));
    let binding: (String, String, String) = sqlx::query_as("SELECT service_id,endpoint_id,thread_id FROM thread_bindings WHERE thread_binding_id='binding-1'").fetch_one(&mut connection).await?;
    ensure_eq!(
        binding,
        ("service-1".into(), "endpoint-1".into(), "thread-1".into())
    );
    let run: (String, Option<String>, String, Option<String>) = sqlx::query_as(
        "SELECT run_status,captured_inputs_json,execution_evidence_json,worker_outcome_json FROM workflow_runs WHERE run_id='run-1'",
    ).fetch_one(&mut connection).await?;
    ensure_eq!(
        run,
        (
            "uncertain".into(),
            Some("{\"input\":\"kept\"}".into()),
            "{\"nativeInputAccepted\":true}".into(),
            Some("{\"status\":\"kept\"}".into())
        )
    );
    let wake: (String, String, Option<String>) = sqlx::query_as("SELECT wakeup_status,definition_json,pending_delivery_id FROM wakeup_definitions WHERE wakeup_id='wake-1'").fetch_one(&mut connection).await?;
    ensure_eq!(
        wake,
        (
            "active".into(),
            "{\"message\":\"kept\"}".into(),
            Some("delivery-1".into())
        )
    );
    let delivery: (String, String, Option<String>) = sqlx::query_as("SELECT delivery_status,message_json,latest_attempt_json FROM mailbox_deliveries WHERE delivery_id='delivery-1'").fetch_one(&mut connection).await?;
    ensure_eq!(
        delivery,
        (
            "uncertain".into(),
            "{\"text\":\"kept\"}".into(),
            Some("{\"attempt\":2}".into())
        )
    );
    let receipt: (Vec<u8>, String, String) = sqlx::query_as(
        "SELECT canonical_request,operation_status,effect_evidence_json FROM operation_receipts WHERE operation_id='operation-1'",
    ).fetch_one(&mut connection).await?;
    ensure_eq!(
        receipt,
        (
            vec![0, 1, 255],
            "uncertain".into(),
            "{\"effect\":\"kept\"}".into()
        )
    );
    let event: (i64, String) = sqlx::query_as(
        "SELECT event_sequence,event_body_json FROM automation_events WHERE event_id='event-1'",
    )
    .fetch_one(&mut connection)
    .await?;
    ensure_eq!(event, (41, "{\"evidence\":\"kept\"}".into()));
    let high_water: i64 =
        sqlx::query_scalar("SELECT seq FROM sqlite_sequence WHERE name='automation_events'")
            .fetch_one(&mut connection)
            .await?;
    ensure_eq!(high_water, 79);
    let foreign_key_failures = sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(&mut connection)
        .await?;
    ensure!(foreign_key_failures.is_empty());
    connection.close().await?;
    Ok(())
}

async fn migration_history(path: &Path) -> Result<Vec<(i64, bool, Vec<u8>)>, sqlx::Error> {
    let mut connection = connect(path).await?;
    let rows =
        sqlx::query("SELECT version,success,checksum FROM _sqlx_migrations ORDER BY version")
            .fetch_all(&mut connection)
            .await?
            .into_iter()
            .map(|row| (row.get(0), row.get(1), row.get(2)))
            .collect();
    connection.close().await?;
    Ok(rows)
}

async fn has_migration_history_table(path: &Path) -> Result<bool, sqlx::Error> {
    let mut connection = connect(path).await?;
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='_sqlx_migrations')",
    )
    .fetch_one(&mut connection)
    .await?;
    connection.close().await?;
    Ok(exists)
}

#[tokio::test]
async fn fresh_database_runs_native_baseline_and_reopens() -> Result<(), Box<dyn std::error::Error>>
{
    let database = TestDatabase::new("fresh");
    AutomationStore::open(&database.path).await?.close().await?;
    let history = migration_history(&database.path).await?;
    ensure_eq!(history.len(), 1);
    ensure!(history[0].1);
    ensure!(!history[0].2.is_empty());
    AutomationStore::open(&database.path).await?.close().await?;
    ensure_eq!(migration_history(&database.path).await?, history);
    Ok(())
}

#[tokio::test]
async fn exact_v1_is_adopted_without_changing_domain_state()
-> Result<(), Box<dyn std::error::Error>> {
    let database = TestDatabase::new("adopt-v1");
    create_legacy_v1(&database.path, LEGACY_V1_SCHEMA).await?;
    seed_all_domain_tables(&database.path).await?;
    AutomationStore::open(&database.path).await?.close().await?;
    assert_preserved_domain_state(&database.path).await?;
    let history = migration_history(&database.path).await?;
    ensure_eq!(history.len(), 1);
    AutomationStore::open(&database.path).await?.close().await?;
    assert_preserved_domain_state(&database.path).await?;
    ensure_eq!(migration_history(&database.path).await?, history);
    Ok(())
}

#[tokio::test]
async fn incompatible_v1_shapes_are_rejected_without_adoption()
-> Result<(), Box<dyn std::error::Error>> {
    let variants = [
        (
            "column-type",
            LEGACY_V1_SCHEMA.replacen(
                "instruction_text TEXT NOT NULL",
                "instruction_text BLOB NOT NULL",
                1,
            ),
        ),
        (
            "column-nullability",
            LEGACY_V1_SCHEMA.replacen("updated_at_ms INTEGER NOT NULL", "updated_at_ms INTEGER", 1),
        ),
        (
            "column-default",
            LEGACY_V1_SCHEMA.replacen(
                "updated_at_ms INTEGER NOT NULL",
                "updated_at_ms INTEGER NOT NULL DEFAULT 7",
                1,
            ),
        ),
        (
            "enabled-check",
            LEGACY_V1_SCHEMA.replacen(
                "enabled INTEGER NOT NULL CHECK (enabled IN (0,1))",
                "enabled INTEGER NOT NULL",
                1,
            ),
        ),
        (
            "extra-check",
            LEGACY_V1_SCHEMA.replacen(
                "operation_status TEXT NOT NULL,",
                "operation_status TEXT NOT NULL CHECK(length(operation_status)>0),",
                1,
            ),
        ),
        (
            "foreign-key-target",
            LEGACY_V1_SCHEMA.replacen(
                "REFERENCES schedule_definitions(schedule_id)",
                "REFERENCES instruction_documents(instruction_id)",
                1,
            ),
        ),
        (
            "foreign-key-deferral",
            LEGACY_V1_SCHEMA.replacen(" DEFERRABLE INITIALLY DEFERRED", "", 1),
        ),
        (
            "required-index",
            LEGACY_V1_SCHEMA.replacen(
                "CREATE INDEX event_cleanup ON automation_events(recorded_at_ms, event_sequence);",
                "CREATE INDEX event_cleanup ON automation_events(event_sequence, recorded_at_ms);",
                1,
            ),
        ),
        (
            "index-sort-order",
            LEGACY_V1_SCHEMA.replacen(
                "CREATE INDEX event_cleanup ON automation_events(recorded_at_ms, event_sequence);",
                "CREATE INDEX event_cleanup ON automation_events(recorded_at_ms DESC, event_sequence);",
                1,
            ),
        ),
        (
            "index-collation",
            LEGACY_V1_SCHEMA.replacen(
                "CREATE INDEX event_cleanup ON automation_events(recorded_at_ms, event_sequence);",
                "CREATE INDEX event_cleanup ON automation_events(recorded_at_ms COLLATE NOCASE, event_sequence);",
                1,
            ),
        ),
        (
            "strict-table",
            LEGACY_V1_SCHEMA.replacen(
                " committed_at_ms INTEGER NOT NULL\n);",
                " committed_at_ms INTEGER NOT NULL\n) STRICT;",
                1,
            ),
        ),
        (
            "without-rowid",
            LEGACY_V1_SCHEMA.replacen(
                " next_due_at_ms INTEGER\n);",
                " next_due_at_ms INTEGER\n) WITHOUT ROWID;",
                1,
            ),
        ),
        (
            "missing-domain-table",
            LEGACY_V1_SCHEMA.replacen(
                "CREATE TABLE schedule_timing_state",
                "CREATE TABLE unexpected_timing_state",
                1,
            ),
        ),
        (
            "extra-domain-table",
            format!(
                "{LEGACY_V1_SCHEMA}\nCREATE TABLE unexpected_domain_table(id TEXT PRIMARY KEY);"
            ),
        ),
        (
            "unexpected-trigger",
            format!(
                "{LEGACY_V1_SCHEMA}\nCREATE TRIGGER unexpected_trigger AFTER INSERT ON automation_events BEGIN SELECT 1; END;"
            ),
        ),
    ];

    for (label, schema) in variants {
        let database = TestDatabase::new(label);
        create_legacy_v1(&database.path, &schema).await?;
        let mut connection = connect(&database.path).await?;
        sqlx::query("INSERT INTO automation_events(event_sequence,event_id,subject_kind,subject_id,event_kind,event_body_json,recorded_at_ms) VALUES (41,'sentinel','run','run-1','kept','{\"kept\":true}',1)").execute(&mut connection).await?;
        connection.close().await?;

        let error = open_error(&database.path, "incompatible v1 schema must be rejected").await?;
        ensure!(
            matches!(error, StorageError::InvalidSchema),
            "{label}: {error}"
        );
        ensure!(
            !has_migration_history_table(&database.path).await?,
            "{label}"
        );
        let mut connection = connect(&database.path).await?;
        let sentinel: (i64, String) = sqlx::query_as("SELECT event_sequence,event_body_json FROM automation_events WHERE event_id='sentinel'").fetch_one(&mut connection).await?;
        ensure_eq!(sentinel, (41, "{\"kept\":true}".into()), "{label}");
        let version: i64 = sqlx::query_scalar("PRAGMA user_version")
            .fetch_one(&mut connection)
            .await?;
        ensure_eq!(version, 1, "{label}");
        let high_water: i64 =
            sqlx::query_scalar("SELECT seq FROM sqlite_sequence WHERE name='automation_events'")
                .fetch_one(&mut connection)
                .await?;
        ensure_eq!(high_water, 41, "{label}");
        connection.close().await?;
    }
    Ok(())
}

#[tokio::test]
async fn unknown_legacy_version_remains_unsupported() -> Result<(), Box<dyn std::error::Error>> {
    let database = TestDatabase::new("unsupported");
    create_legacy_v1(&database.path, LEGACY_V1_SCHEMA).await?;
    let mut connection = connect(&database.path).await?;
    sqlx::query("PRAGMA user_version=2")
        .execute(&mut connection)
        .await?;
    connection.close().await?;
    let error = open_error(&database.path, "future legacy version must be rejected").await?;
    ensure!(matches!(error, StorageError::UnsupportedVersion(2)));
    ensure!(!has_migration_history_table(&database.path).await?);
    Ok(())
}

#[tokio::test]
async fn conflicting_native_history_is_rejected_without_domain_mutation()
-> Result<(), Box<dyn std::error::Error>> {
    let database = TestDatabase::new("history-mismatch");
    AutomationStore::open(&database.path).await?.close().await?;
    let mut connection = connect(&database.path).await?;
    sqlx::query("INSERT INTO automation_events(event_id,subject_kind,subject_id,event_kind,event_body_json,recorded_at_ms) VALUES ('sentinel','run','run-1','kept','{}',1)").execute(&mut connection).await?;
    sqlx::query("UPDATE _sqlx_migrations SET checksum=x'00'")
        .execute(&mut connection)
        .await?;
    connection.close().await?;

    let error = open_error(&database.path, "changed native migration must be rejected").await?;
    ensure!(matches!(error, StorageError::InvalidSchema));
    let mut connection = connect(&database.path).await?;
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM automation_events WHERE event_id='sentinel'")
            .fetch_one(&mut connection)
            .await?;
    ensure_eq!(count, 1);
    let checksum: Vec<u8> = sqlx::query_scalar("SELECT checksum FROM _sqlx_migrations")
        .fetch_one(&mut connection)
        .await?;
    ensure_eq!(checksum, vec![0]);
    connection.close().await?;
    Ok(())
}

#[tokio::test]
async fn dirty_native_history_is_rejected_without_repair() -> Result<(), Box<dyn std::error::Error>>
{
    let database = TestDatabase::new("history-dirty");
    AutomationStore::open(&database.path).await?.close().await?;
    let mut connection = connect(&database.path).await?;
    sqlx::query("UPDATE _sqlx_migrations SET success=FALSE")
        .execute(&mut connection)
        .await?;
    connection.close().await?;

    let error = open_error(&database.path, "dirty native history must be rejected").await?;
    ensure!(matches!(error, StorageError::InvalidSchema));
    let mut connection = connect(&database.path).await?;
    let success: bool = sqlx::query_scalar("SELECT success FROM _sqlx_migrations")
        .fetch_one(&mut connection)
        .await?;
    ensure!(!success);
    connection.close().await?;
    Ok(())
}

#[tokio::test]
async fn held_writer_returns_database_error_without_partial_history_then_retry_succeeds()
-> Result<(), Box<dyn std::error::Error>> {
    let database = TestDatabase::new("contention");
    let mut lock = connect(&database.path).await?;
    sqlx::query("BEGIN IMMEDIATE").execute(&mut lock).await?;
    let error = open_error(&database.path, "held writer must prevent initialization").await?;
    ensure!(matches!(error, StorageError::Database(_)));
    sqlx::query("ROLLBACK").execute(&mut lock).await?;
    lock.close().await?;
    ensure!(!has_migration_history_table(&database.path).await?);
    AutomationStore::open(&database.path).await?.close().await?;
    ensure_eq!(migration_history(&database.path).await?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn concurrent_fresh_openers_converge_on_one_history_row()
-> Result<(), Box<dyn std::error::Error>> {
    let database = TestDatabase::new("concurrent");
    let (first, second) = tokio::join!(
        AutomationStore::open(&database.path),
        AutomationStore::open(&database.path)
    );
    first?.close().await?;
    second?.close().await?;
    ensure_eq!(migration_history(&database.path).await?.len(), 1);
    Ok(())
}

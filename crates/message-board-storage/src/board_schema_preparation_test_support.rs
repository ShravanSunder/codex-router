#![allow(clippy::unwrap_used)]

use sqlx::{
    Connection, SqliteConnection,
    migrate::Migrator,
    sqlite::{SqliteConnectOptions, SqliteJournalMode},
};
use std::{borrow::Cow, path::PathBuf};

pub(super) struct TemporaryBoardDatabase {
    path: PathBuf,
}

impl TemporaryBoardDatabase {
    pub(super) fn new(label: &str) -> Self {
        Self {
            path: std::env::temp_dir().join(format!(
                "board-schema-preparation-{label}-{}.sqlite",
                uuid::Uuid::now_v7()
            )),
        }
    }

    pub(super) fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl Drop for TemporaryBoardDatabase {
    fn drop(&mut self) {
        for suffix in ["", "-journal", "-wal", "-shm"] {
            let sidecar = format!("{}{suffix}", self.path.display());
            let _ = std::fs::remove_file(sidecar);
        }
    }
}

pub(super) struct SubscriptionPrefixFixture {
    pub(super) database: TemporaryBoardDatabase,
    pub(super) project_id: String,
    pub(super) root_id: String,
    pub(super) reader_key: String,
}

#[derive(Debug, Eq, PartialEq)]
pub(super) struct DatabaseSnapshot {
    pub(super) main_database_bytes: Vec<u8>,
    pub(super) migration_history: Vec<(String, String, String, String, String, String)>,
    pub(super) schema_objects: Vec<(String, String, String, Option<String>)>,
    pub(super) user_version: i64,
    pub(super) domain_counts: (i64, i64, i64, i64, i64),
    pub(super) project_rows: Vec<(String, String, String)>,
    pub(super) subscriptions_exist: bool,
    pub(super) subscription_count: Option<i64>,
}

pub(super) async fn create_pre_subscription_prefix(label: &str) -> SubscriptionPrefixFixture {
    let database = TemporaryBoardDatabase::new(label);
    let mut connection = open_writer(database.path(), true).await;
    let subscription_version = crate::board_schema_migrations::THREAD_SUBSCRIPTIONS_VERSION;
    let prefix_migrations = crate::board_schema_migrations::MIGRATOR
        .iter()
        .take_while(|migration| migration.version < subscription_version)
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(
        prefix_migrations.last().map(|migration| migration.version),
        Some(202609160002),
        "fixture must stop immediately before thread-subscription creation"
    );
    let prefix_migrator = Migrator {
        migrations: Cow::Owned(prefix_migrations),
        ..Migrator::DEFAULT
    };
    prefix_migrator
        .run(&mut connection)
        .await
        .expect("actual board migration prefix should apply");

    let project_id = message_board::ProjectId::generate().as_str().to_owned();
    let board_id = message_board::BoardId::generate().as_str().to_owned();
    let topic_id = message_board::TopicId::generate().as_str().to_owned();
    let root_id = message_board::MessageId::generate().as_str().to_owned();
    let session_id = "pre-subscription-reader";
    let service_id = "550e8400-e29b-41d4-a716-446655440000";
    let endpoint_id = "codex-local";
    let reader_key = format!("session:{service_id}:{endpoint_id}:{session_id}");
    sqlx::query(
        "INSERT INTO board_identities(identity_key,kind,service_id,endpoint_id,session_id)
         VALUES(?,'session',?,?,?)",
    )
    .bind(&reader_key)
    .bind(service_id)
    .bind(endpoint_id)
    .bind(session_id)
    .execute(&mut connection)
    .await
    .expect("session identity should seed");
    sqlx::query(
        "INSERT INTO board_projects(project_id,name,description) VALUES(?,'Project','seed')",
    )
    .bind(&project_id)
    .execute(&mut connection)
    .await
    .expect("project row should seed");
    sqlx::query(
        "INSERT INTO project_boards(board_id,project_id,name,description,state)
         VALUES(?,?,'Board','','active')",
    )
    .bind(&board_id)
    .bind(&project_id)
    .execute(&mut connection)
    .await
    .expect("board row should seed");
    sqlx::query(
        "INSERT INTO board_topics(topic_id,board_id,name,description)
         VALUES(?,?,'Topic','')",
    )
    .bind(&topic_id)
    .bind(&board_id)
    .execute(&mut connection)
    .await
    .expect("topic row should seed");
    sqlx::query(
        "INSERT INTO board_messages(message_id,topic_id,board_id,root_id,actor_key,text)
         VALUES(?,?,?,NULL,?,'pre-subscription root')",
    )
    .bind(&root_id)
    .bind(&topic_id)
    .bind(&board_id)
    .bind(&reader_key)
    .execute(&mut connection)
    .await
    .expect("root message row should seed");
    sqlx::query("INSERT INTO board_threads(root_id,state) VALUES(?,'unresolved')")
        .bind(&root_id)
        .execute(&mut connection)
        .await
        .expect("unresolved thread row should seed");
    sqlx::query("UPDATE activity_checkpoint SET last_sequence=1 WHERE singleton=1")
        .execute(&mut connection)
        .await
        .expect("activity checkpoint should track seeded row");
    sqlx::query(
        "INSERT INTO board_activity(activity_sequence,project_id,board_id,topic_id,root_id,kind,actor_key,message_id)
         VALUES(1,?,?,?,NULL,'mainMessageCreated',?,?)",
    )
    .bind(&project_id)
    .bind(&board_id)
    .bind(&topic_id)
    .bind(&reader_key)
    .bind(&root_id)
    .execute(&mut connection)
    .await
    .expect("root activity row should seed");
    sqlx::query(
        "INSERT INTO thread_participants(reader_key,root_id,role,joined_at_activity,last_seen_activity)
         VALUES(?,?,'participant',1,1)",
    )
    .bind(&reader_key)
    .bind(&root_id)
    .execute(&mut connection)
    .await
    .expect("open participant row should seed");
    sqlx::query(
        "INSERT INTO thread_watches(reader_key,root_id,active,starts_after_activity)
         VALUES(?,?,1,1)",
    )
    .bind(&reader_key)
    .bind(&root_id)
    .execute(&mut connection)
    .await
    .expect("active watch row should seed");
    connection
        .close()
        .await
        .expect("prefix fixture connection should close");

    SubscriptionPrefixFixture {
        database,
        project_id,
        root_id,
        reader_key,
    }
}

pub(super) async fn create_full_native_database(label: &str) -> TemporaryBoardDatabase {
    let database = TemporaryBoardDatabase::new(label);
    let mut connection = open_writer(database.path(), true).await;
    crate::board_schema_migrations::MIGRATOR
        .run(&mut connection)
        .await
        .expect("actual full board migration set should apply");
    connection
        .close()
        .await
        .expect("full-schema fixture connection should close");
    database
}

pub(super) async fn create_wal_current_subscription_fixture(
    label: &str,
) -> SubscriptionPrefixFixture {
    let fixture = create_pre_subscription_prefix(label).await;
    let store = crate::BoardStore::open(fixture.database.path())
        .await
        .expect("ordinary writer should finish the current board schema");
    store
        .close()
        .await
        .expect("ordinary board writer should close");
    enable_write_ahead_logging(fixture.database.path()).await;
    fixture
}

pub(super) async fn enable_write_ahead_logging(path: &std::path::Path) {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(false)
        .journal_mode(SqliteJournalMode::Wal);
    let mut connection = SqliteConnection::connect_with(&options)
        .await
        .expect("fixture should enter write-ahead logging mode");
    let journal_mode: String = sqlx::query_scalar("PRAGMA journal_mode")
        .fetch_one(&mut connection)
        .await
        .expect("fixture journal mode should query");
    assert_eq!(journal_mode.to_ascii_lowercase(), "wal");
    connection
        .close()
        .await
        .expect("WAL setup connection should close");
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

pub(super) async fn apply_prefix_defect_database(
    path: &std::path::Path,
    defect: super::HistoryDefect,
) {
    let mut connection = open_writer(path, false).await;
    let first_version = crate::board_schema_migrations::MIGRATOR
        .iter()
        .next()
        .expect("board image should contain a migration")
        .version;
    let newest_version = crate::board_schema_migrations::MIGRATOR
        .iter()
        .last()
        .expect("board image should contain a migration")
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
                 VALUES(202609180001,'unknown migration',1,X'00',1)",
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
    for migration in crate::board_schema_migrations::MIGRATOR.iter() {
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

pub(super) async fn create_legacy_database(label: &str) -> TemporaryBoardDatabase {
    let database = TemporaryBoardDatabase::new(label);
    let mut connection = open_writer(database.path(), true).await;
    sqlx::raw_sql("CREATE TABLE legacy_board_rows(value TEXT NOT NULL); PRAGMA user_version=1;")
        .execute(&mut connection)
        .await
        .expect("legacy database should seed");
    sqlx::query("INSERT INTO legacy_board_rows(value) VALUES('preserve-me')")
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
    let has_history: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='_sqlx_migrations')",
    )
    .fetch_one(&mut connection)
    .await
    .expect("migration table presence should query");
    let migration_history = if has_history {
        sqlx::query_as(
            "SELECT typeof(version), CAST(version AS TEXT), typeof(success),
                    CAST(success AS TEXT), typeof(checksum), hex(checksum)
             FROM _sqlx_migrations ORDER BY version",
        )
        .fetch_all(&mut connection)
        .await
        .expect("raw migration history should query")
    } else {
        Vec::new()
    };
    let schema_objects =
        sqlx::query_as("SELECT type,name,tbl_name,sql FROM sqlite_schema ORDER BY type,name")
            .fetch_all(&mut connection)
            .await
            .expect("schema objects should query");
    let user_version = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(&mut connection)
        .await
        .expect("user version should query");
    let domain_counts = sqlx::query_as(
        "SELECT (SELECT count(*) FROM board_projects),
                (SELECT count(*) FROM board_messages),
                (SELECT count(*) FROM board_activity),
                (SELECT count(*) FROM thread_participants),
                (SELECT count(*) FROM thread_watches)",
    )
    .fetch_one(&mut connection)
    .await
    .expect("domain row counts should query");
    let project_rows = sqlx::query_as(
        "SELECT project_id,name,description FROM board_projects ORDER BY project_id",
    )
    .fetch_all(&mut connection)
    .await
    .expect("project rows should query");
    let subscriptions_exist = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='thread_subscriptions')",
    )
    .fetch_one(&mut connection)
    .await
    .expect("subscription table presence should query");
    let subscription_count = if subscriptions_exist {
        Some(
            sqlx::query_scalar("SELECT count(*) FROM thread_subscriptions")
                .fetch_one(&mut connection)
                .await
                .expect("subscription count should query"),
        )
    } else {
        None
    };
    connection
        .close()
        .await
        .expect("snapshot connection should close");
    DatabaseSnapshot {
        main_database_bytes,
        migration_history,
        schema_objects,
        user_version,
        domain_counts,
        project_rows,
        subscriptions_exist,
        subscription_count,
    }
}

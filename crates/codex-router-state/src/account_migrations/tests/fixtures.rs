use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::Stdio;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;

use rusqlite::Connection;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::sqlite::SqlitePoolOptions;

use super::super::BASELINE_SQL;

pub(super) fn create_conflicting_v13_database(database_path: &Path) {
    let connection = Connection::open(database_path).expect("fixture should open");
    connection
        .execute_batch(
            "CREATE TABLE accounts (
                account_id TEXT PRIMARY KEY NOT NULL,
                label BLOB NOT NULL,
                status TEXT NOT NULL,
                active_credential_generation INTEGER
             );
             INSERT INTO accounts VALUES ('preserved', 'before-failure', 'enabled', 7);
             PRAGMA user_version = 13;",
        )
        .expect("fixture should initialize");
}

pub(super) async fn open_test_pool(database_path: &Path) -> sqlx::SqlitePool {
    let options = SqliteConnectOptions::new()
        .filename(database_path)
        .create_if_missing(false);
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .expect("test pool should open")
}

pub(super) fn create_legacy_v13_database(database_path: &Path, account_label: &str) {
    let connection = Connection::open(database_path).expect("legacy fixture should open");
    connection
        .execute_batch(BASELINE_SQL)
        .expect("legacy v13 schema should initialize");
    connection
        .execute(
            "INSERT INTO accounts VALUES ('preserved-account', ?1, 'disabled', 41)",
            [account_label],
        )
        .expect("legacy account should seed");
    connection
        .execute(
            "INSERT INTO affinity_pins VALUES ('preserved-pin', 'preserved-account')",
            [],
        )
        .expect("legacy pin should seed");
    connection
        .execute(
            "INSERT INTO previous_response_affinity_owners VALUES (
                'preserved-hash', 'responses', 'preserved-account', 41, 'websocket', 1234
             )",
            [],
        )
        .expect("legacy owner should seed");
    connection
        .execute(
            "INSERT INTO account_routing_policies VALUES ('preserved-account', 900)",
            [],
        )
        .expect("legacy policy should seed");
    connection
        .execute(
            "INSERT INTO session_account_affinities VALUES (
                'preserved-session', 'preserved-account', 5678
             )",
            [],
        )
        .expect("legacy session affinity should seed");
}

pub(super) fn create_legacy_v11_database(database_path: &Path) {
    create_legacy_v13_database(database_path, "v11-policy");
    let connection = Connection::open(database_path).expect("v11 fixture should reopen");
    connection
        .execute_batch(
            "DROP TABLE session_account_affinities;
             ALTER TABLE account_routing_policies RENAME TO account_routing_policies_current;
             CREATE TABLE account_routing_policies (
                account_id TEXT PRIMARY KEY NOT NULL,
                weekly_quota_floor_basis_points INTEGER NOT NULL
                    CHECK (
                        weekly_quota_floor_basis_points BETWEEN 100 AND 1000
                        AND weekly_quota_floor_basis_points % 100 = 0
                    )
             );
             INSERT INTO account_routing_policies
                SELECT * FROM account_routing_policies_current;
             DROP TABLE account_routing_policies_current;
             PRAGMA user_version = 11;",
        )
        .expect("v11 fixture should initialize");
}

pub(super) fn create_legacy_v7_database(database_path: &Path) {
    create_legacy_v13_database(database_path, "v7-lease");
    let connection = Connection::open(database_path).expect("v7 fixture should reopen");
    connection
        .execute_batch(
            "DROP TABLE session_account_affinities;
             DROP TABLE account_routing_policies;
             DROP TABLE active_client_leases;
             CREATE TABLE active_client_leases (
                route_band TEXT NOT NULL,
                reservation_id TEXT NOT NULL,
                account_id TEXT NOT NULL,
                acquired_unix_seconds INTEGER NOT NULL,
                PRIMARY KEY (route_band, reservation_id)
             );
             INSERT INTO active_client_leases VALUES (
                'responses', 'legacy-reservation', 'preserved-account', 123
             );
             PRAGMA user_version = 7;",
        )
        .expect("v7 fixture should initialize");
}

pub(super) fn create_legacy_v7_current_lease_database(database_path: &Path) {
    create_legacy_v13_database(database_path, "v7-current-lease");
    let connection = Connection::open(database_path).expect("v7 fixture should reopen");
    connection
        .execute_batch(
            "DROP TABLE session_account_affinities;
             DROP TABLE account_routing_policies;
             INSERT INTO active_client_leases VALUES (
                'responses', 'current-process', 'current-reservation',
                'preserved-account', 123, 5
             );
             PRAGMA user_version = 7;",
        )
        .expect("v7 current lease fixture should initialize");
}

pub(super) fn create_legacy_v7_without_lease_table(database_path: &Path) {
    create_legacy_v13_database(database_path, "v7-absent-lease");
    let connection = Connection::open(database_path).expect("v7 fixture should reopen");
    connection
        .execute_batch(
            "DROP TABLE session_account_affinities;
             DROP TABLE account_routing_policies;
             DROP TABLE active_client_leases;
             PRAGMA user_version = 7;",
        )
        .expect("v7 absent lease fixture should initialize");
}

pub(super) fn create_legacy_v9_history_shape(database_path: &Path, optional_field_mask: u8) {
    create_legacy_v13_database(database_path, "v9-history");
    let connection = Connection::open(database_path).expect("v9 fixture should reopen");
    let mut event_columns = Vec::new();
    let mut event_column_names = Vec::new();
    let mut event_values = Vec::new();
    for (bit, definition, name, value) in [
        (
            1,
            "logical_session_id TEXT NOT NULL",
            "logical_session_id",
            "'logical-v9'",
        ),
        (
            2,
            "session_started_unix_seconds INTEGER NOT NULL",
            "session_started_unix_seconds",
            "88",
        ),
        (
            4,
            "session_ended_unix_seconds INTEGER",
            "session_ended_unix_seconds",
            "99",
        ),
        (
            8,
            "transport_kind TEXT NOT NULL",
            "transport_kind",
            "'websocket'",
        ),
    ] {
        if optional_field_mask & bit != 0 {
            event_columns.push(definition);
            event_column_names.push(name);
            event_values.push(value);
        }
    }
    let mut rollup_columns = Vec::new();
    let mut rollup_column_names = Vec::new();
    let mut rollup_values = Vec::new();
    for (bit, definition, name, value) in [
        (
            16,
            "completed_sessions INTEGER NOT NULL",
            "completed_sessions",
            "3",
        ),
        (
            32,
            "stale_purged_sessions INTEGER NOT NULL",
            "stale_purged_sessions",
            "4",
        ),
    ] {
        if optional_field_mask & bit != 0 {
            rollup_columns.push(definition);
            rollup_column_names.push(name);
            rollup_values.push(value);
        }
    }
    let event_optional_definitions = event_columns
        .iter()
        .map(|definition| format!(", {definition}"))
        .collect::<String>();
    let event_optional_names = event_column_names
        .iter()
        .map(|name| format!(", {name}"))
        .collect::<String>();
    let event_optional_values = event_values
        .iter()
        .map(|value| format!(", {value}"))
        .collect::<String>();
    let rollup_optional_definitions = rollup_columns
        .iter()
        .map(|definition| format!(", {definition}"))
        .collect::<String>();
    let rollup_optional_names = rollup_column_names
        .iter()
        .map(|name| format!(", {name}"))
        .collect::<String>();
    let rollup_optional_values = rollup_values
        .iter()
        .map(|value| format!(", {value}"))
        .collect::<String>();
    connection
        .execute_batch(&format!(
            "DROP TABLE session_account_affinities;
             DROP TABLE account_routing_policies;
             DROP TABLE active_session_events;
             CREATE TABLE active_session_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                account_id TEXT NOT NULL,
                route_band TEXT NOT NULL,
                process_run_id TEXT NOT NULL,
                reservation_id TEXT NOT NULL,
                event_kind TEXT NOT NULL,
                event_unix_seconds INTEGER NOT NULL
                {event_optional_definitions}
             );
             CREATE INDEX active_session_events_route_lookup
                ON active_session_events (route_band, account_id, event_unix_seconds);
             INSERT INTO active_session_events (
                id, account_id, route_band, process_run_id,
                reservation_id, event_kind, event_unix_seconds
                {event_optional_names}
             ) VALUES (
                41, 'preserved-account', 'responses', 'process-v9',
                'reservation-v9', 'acquired', 123
                {event_optional_values}
             );
             DROP TABLE active_session_rollups;
             CREATE TABLE active_session_rollups (
                account_id TEXT NOT NULL,
                route_band TEXT NOT NULL,
                bucket_start_unix_seconds INTEGER NOT NULL,
                bucket_end_unix_seconds INTEGER NOT NULL,
                active_session_seconds INTEGER NOT NULL,
                max_concurrent_sessions INTEGER NOT NULL
                {rollup_optional_definitions},
                PRIMARY KEY (
                    account_id, route_band, bucket_start_unix_seconds, bucket_end_unix_seconds
                )
             );
             INSERT INTO active_session_rollups (
                account_id, route_band, bucket_start_unix_seconds,
                bucket_end_unix_seconds, active_session_seconds,
                max_concurrent_sessions {rollup_optional_names}
             ) VALUES (
                'preserved-account', 'responses', 0, 300, 77, 2
                {rollup_optional_values}
             );
             PRAGMA user_version = 9;"
        ))
        .expect("v9 history fixture should initialize");
}

pub(super) const LEGACY_V0_TABLE_STATEMENTS: &[&str] = &[
    "CREATE TABLE accounts (
        account_id TEXT PRIMARY KEY NOT NULL,
        label TEXT NOT NULL,
        status TEXT NOT NULL,
        active_credential_generation INTEGER
    );",
    "CREATE TABLE quota_snapshots (
        account_id TEXT NOT NULL,
        source TEXT NOT NULL,
        observed_unix_seconds INTEGER NOT NULL,
        route_band TEXT NOT NULL,
        remaining_headroom INTEGER NOT NULL,
        reset_unix_seconds INTEGER,
        reset_credits_available INTEGER,
        stale_penalty INTEGER NOT NULL,
        PRIMARY KEY (account_id, route_band)
    );",
    "CREATE TABLE affinity_pins (
        affinity_key TEXT PRIMARY KEY NOT NULL,
        account_id TEXT NOT NULL
    );",
    "CREATE TABLE selector_quota_windows (
        account_id TEXT NOT NULL,
        route_band TEXT NOT NULL,
        limit_window_seconds INTEGER NOT NULL,
        status TEXT NOT NULL,
        remaining_headroom INTEGER NOT NULL,
        reset_unix_seconds INTEGER,
        effective INTEGER NOT NULL,
        observed_unix_seconds INTEGER NOT NULL,
        PRIMARY KEY (account_id, route_band, limit_window_seconds)
    );",
    "CREATE TABLE quota_refresh_status (
        account_id TEXT NOT NULL,
        route_band TEXT NOT NULL,
        last_success_unix_seconds INTEGER,
        last_attempt_unix_seconds INTEGER,
        last_error_class TEXT,
        stale_after_unix_seconds INTEGER,
        PRIMARY KEY (account_id, route_band)
    );",
    "CREATE TABLE previous_response_affinity_owners (
        affinity_key_hash TEXT NOT NULL,
        route_band TEXT NOT NULL,
        account_id TEXT NOT NULL,
        credential_generation INTEGER NOT NULL,
        source_transport TEXT NOT NULL,
        created_unix_seconds INTEGER NOT NULL,
        PRIMARY KEY (affinity_key_hash, route_band, account_id)
    );",
];

pub(super) fn assert_legacy_database_unchanged(database_path: &Path, account_label: &str) {
    let connection = Connection::open(database_path).expect("legacy database should inspect");
    let history_exists: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = '_sqlx_migrations')",
            [],
            |row| row.get(0),
        )
        .expect("history presence should query");
    assert!(!history_exists);
    let account: (String, String, i64) = connection
        .query_row(
            "SELECT label, status, active_credential_generation
               FROM accounts WHERE account_id = 'preserved-account'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("preserved account should query");
    assert_eq!(
        account,
        (account_label.to_owned(), "disabled".to_owned(), 41)
    );
}

pub(super) fn assert_native_history_present(database_path: &Path) {
    let connection = Connection::open(database_path).expect("native database should inspect");
    let history_rows: i64 = connection
        .query_row("SELECT COUNT(*) FROM _sqlx_migrations", [], |row| {
            row.get(0)
        })
        .expect("native history should query");
    assert_eq!(history_rows, 1);
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .expect("legacy marker should query");
    assert_eq!(version, 13);
}

pub(super) fn migration_history_bytes(database_path: &Path) -> Vec<(i64, bool, Vec<u8>)> {
    let connection = Connection::open(database_path).expect("history should inspect");
    let mut statement = connection
        .prepare("SELECT version, success, checksum FROM _sqlx_migrations ORDER BY version")
        .expect("history query should prepare");
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .expect("history should query")
        .collect::<Result<Vec<_>, _>>()
        .expect("history should collect")
}

pub(super) fn run_migration_child_to_checkpoint(
    database_path: &Path,
    commit_before_checkpoint: bool,
) {
    let socket_sequence = TEMPORARY_DATABASE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let checkpoint_path = database_path.with_extension(format!("{socket_sequence}.checkpoint"));
    let release_path = database_path.with_extension(format!("{socket_sequence}.release"));
    let mut command =
        Command::new(std::env::current_exe().expect("test executable should resolve"));
    command
        .arg("--exact")
        .arg("account_migrations::tests::migration_process_helper")
        .arg("--nocapture")
        .env(
            "CODEX_ROUTER_ACCOUNT_MIGRATION_TEST_DATABASE",
            database_path,
        )
        .env(
            "CODEX_ROUTER_ACCOUNT_MIGRATION_TEST_CHECKPOINT",
            &checkpoint_path,
        )
        .env("CODEX_ROUTER_ACCOUNT_MIGRATION_TEST_RELEASE", &release_path)
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    if commit_before_checkpoint {
        command.env(
            "CODEX_ROUTER_ACCOUNT_MIGRATION_TEST_COMMIT_BEFORE_CHECKPOINT",
            "1",
        );
    }
    let mut child = command.spawn().expect("migration child should spawn");
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !checkpoint_path.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "migration child should reach checkpoint"
        );
        std::thread::yield_now();
    }
    assert_eq!(
        std::fs::read(&checkpoint_path).expect("checkpoint should read"),
        b"checkpoint\n"
    );
    child.kill().expect("migration child should terminate");
    let status = child.wait().expect("migration child should reap");
    assert!(!status.success(), "killed child must not report success");
    let _ = std::fs::remove_file(checkpoint_path);
    let _ = std::fs::remove_file(release_path);
}

static TEMPORARY_DATABASE_COUNTER: AtomicUsize = AtomicUsize::new(0);

pub(super) struct TemporaryDatabase {
    directory_path: PathBuf,
    database_path: PathBuf,
}

impl TemporaryDatabase {
    pub(super) fn new(label: &str) -> Self {
        let sequence = TEMPORARY_DATABASE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let directory_path = std::env::temp_dir().join(format!(
            "codex-router-state-account-migration-{label}-{}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory_path)
            .expect("temporary database directory should be created");
        let database_path = directory_path.join("state.sqlite");
        Self {
            directory_path,
            database_path,
        }
    }

    pub(super) fn path(&self) -> &Path {
        &self.database_path
    }
}

impl Drop for TemporaryDatabase {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory_path);
    }
}

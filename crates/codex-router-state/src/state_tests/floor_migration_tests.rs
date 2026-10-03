use super::*;

#[tokio::test]
async fn v10_v11_v12_native_upgrades_create_credit_tables_and_preserve_account_policy() {
    type LegacyFixtureConverter = fn(&Path);
    let legacy_fixtures: [(&str, LegacyFixtureConverter); 3] = [
        ("v10", convert_current_fixture_to_v10),
        ("v11", convert_current_fixture_to_v11),
        ("v12", convert_current_fixture_to_v12),
    ];

    for (version, convert_fixture) in legacy_fixtures {
        let temp_dir = TestTempDir::new(&format!("credit_tables_{version}_upgrade"));
        let database_path = temp_dir.path().join("state.sqlite");
        let account_id = account_id(&format!("acct_credit_tables_{version}"));
        let account = AccountRecord::new(
            Provider::Openai,
            account_id.clone(),
            format!("credit-tables-{version}"),
            AccountStatus::Enabled,
        )
        .with_active_credential_generation(1);
        let current = AsyncSqliteStateStore::open(&database_path)
            .await
            .unwrap_or_else(|error| panic!("current fixture should open for {version}: {error}"));
        current
            .upsert_account(&account)
            .await
            .unwrap_or_else(|error| panic!("{version} account should persist: {error}"));
        current
            .close()
            .await
            .unwrap_or_else(|error| panic!("current fixture should close for {version}: {error}"));

        let preserved_floor = WeeklyQuotaFloorBasisPoints::new(800)
            .expect("800 basis points should be a valid floor");
        if version != "v10" {
            let mutation = AsyncWeeklyQuotaFloorMutationStore::open(&database_path)
                .await
                .unwrap_or_else(|error| {
                    panic!("current floor mutation store should open: {error}")
                });
            mutation
                .set_weekly_quota_floor_by_account_id(&account_id, Some(preserved_floor))
                .await
                .unwrap_or_else(|error| panic!("{version} floor should persist: {error}"));
            mutation.close().await;
        }

        convert_fixture(&database_path);
        assert_credit_migration_tables(&database_path, false);

        let migrated = AsyncSqliteStateStore::open(&database_path)
            .await
            .unwrap_or_else(|error| panic!("{version} native database should upgrade: {error}"));
        assert_eq!(migrated.schema_version().await, Ok(13));
        assert_eq!(migrated.list_accounts().await, Ok(vec![account]));
        let expected_policies = if version == "v10" {
            Vec::new()
        } else {
            vec![AccountRoutingPolicy::new(account_id, preserved_floor)]
        };
        assert_eq!(
            migrated.list_account_routing_policies().await,
            Ok(expected_policies),
            "{version} account policy should survive its native upgrade"
        );
        migrated
            .close()
            .await
            .unwrap_or_else(|error| panic!("upgraded {version} database should close: {error}"));
        assert_credit_migration_tables(&database_path, true);
    }
}

#[test]
fn weekly_floor_accepts_only_integer_percent_basis_points() {
    for basis_points in [100_u16, 200, 500, 1_000, 1_500] {
        assert_eq!(
            WeeklyQuotaFloorBasisPoints::new(basis_points)
                .map(WeeklyQuotaFloorBasisPoints::basis_points),
            Ok(basis_points)
        );
    }
    for basis_points in [0_u16, 1, 99, 101, 999, 1_501, 1_600] {
        assert!(WeeklyQuotaFloorBasisPoints::new(basis_points).is_err());
    }
}

#[tokio::test]
async fn v10_migrates_through_v11_to_v12_and_exposes_policy_read_only() {
    let temp_dir = TestTempDir::new("v10_to_v12_policy_migration");
    let database_path = temp_dir.path().join("state.sqlite");
    let v10 = SqliteStateStore::open(&database_path)
        .unwrap_or_else(|error| panic!("v10 fixture should open: {error}"));
    let account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_v11_preserved"),
        "preserved",
        AccountStatus::Enabled,
    );
    v10.upsert_account(&account)
        .unwrap_or_else(|error| panic!("v10 account should persist: {error}"));
    drop(v10);
    let raw = Connection::open(&database_path).expect("fixture should reopen for seeding");
    raw.execute_batch(
        "INSERT INTO quota_snapshots VALUES (
                'acct_v11_preserved', 'mock_endpoint', 100, 'responses', 5000,
                1000, NULL, 0
             );
             INSERT INTO selector_quota_windows VALUES (
                'acct_v11_preserved', 'responses', 604800, 'eligible', 5000,
                1000, 1, 100
             );
             INSERT INTO active_client_leases VALUES (
                'responses', 'process', 'reservation', 'acct_v11_preserved', 100, 0
             );
             INSERT INTO active_session_events (
                account_id, route_band, process_run_id, logical_session_id,
                reservation_id, event_kind, event_unix_seconds,
                session_started_unix_seconds, session_ended_unix_seconds,
                transport_kind
             ) VALUES (
                'acct_v11_preserved', 'responses', 'process', 'session',
                'reservation', 'acquired', 100, 100, NULL, 'websocket'
             );
             INSERT INTO active_session_rollups VALUES (
                'acct_v11_preserved', 'responses', 0, 300, 100, 1, 0, 0
             );
             CREATE TABLE unrelated_v10_sentinel (value TEXT NOT NULL);
             INSERT INTO unrelated_v10_sentinel VALUES ('preserved');",
    )
    .expect("canonical v10 rows should seed");
    drop(raw);
    convert_current_fixture_to_v10(&database_path);

    let migrated = AsyncSqliteStateStore::open(&database_path)
        .await
        .unwrap_or_else(|error| panic!("v10 database should migrate: {error}"));
    assert_eq!(migrated.schema_version().await, Ok(13));
    assert_eq!(migrated.list_accounts().await, Ok(vec![account]));
    assert_eq!(
        migrated.list_account_routing_policies().await,
        Ok(Vec::new())
    );
    migrated.close().await.expect("migrated store should close");

    let raw = Connection::open(&database_path).expect("migrated database should inspect");
    for table_name in [
        "quota_snapshots",
        "selector_quota_windows",
        "active_client_leases",
        "active_session_events",
        "active_session_rollups",
        "unrelated_v10_sentinel",
    ] {
        let count: i64 = raw
            .query_row(&format!("SELECT COUNT(*) FROM {table_name}"), [], |row| {
                row.get(0)
            })
            .unwrap_or_else(|error| panic!("{table_name} should survive: {error}"));
        assert_eq!(count, 1, "{table_name} row should survive migration");
    }
    let integrity: String = raw
        .pragma_query_value(None, "integrity_check", |row| row.get(0))
        .expect("integrity check should run");
    assert_eq!(integrity, "ok");
    assert!(
        raw.execute(
            "INSERT INTO account_routing_policies VALUES ('acct_v11_preserved', 50)",
            [],
        )
        .is_err()
    );
    assert!(
        raw.execute(
            "INSERT INTO account_routing_policies VALUES ('acct_v11_preserved', 1501)",
            [],
        )
        .is_err()
    );
    drop(raw);

    let read_only = AsyncSqliteStateStore::open_read_only(&database_path)
        .await
        .unwrap_or_else(|error| panic!("v12 should reopen read-only: {error}"));
    assert_eq!(
        read_only.list_account_routing_policies().await,
        Ok(Vec::new())
    );
}

#[tokio::test]
async fn v12_migration_preserves_v11_policy_and_expands_integer_percent_check() {
    let temp_dir = TestTempDir::new("v11_to_v12_policy_migration");
    let database_path = temp_dir.path().join("state.sqlite");
    let store = AsyncSqliteStateStore::open(&database_path)
        .await
        .expect("current fixture should open");
    let account_id = account_id("acct_v12_preserved");
    store
        .upsert_account(&AccountRecord::new(
            codex_router_core::provider::Provider::Openai,
            account_id.clone(),
            "v12-preserved",
            AccountStatus::Enabled,
        ))
        .await
        .expect("account should persist");
    store.close().await.expect("fixture should close");
    let mutation = AsyncWeeklyQuotaFloorMutationStore::open(&database_path)
        .await
        .expect("current mutation store should open");
    let preserved_floor = WeeklyQuotaFloorBasisPoints::new(1_000).expect("valid v11 floor");
    mutation
        .set_weekly_quota_floor_by_label("v12-preserved", Some(preserved_floor))
        .await
        .expect("v11-compatible floor should persist");
    mutation.close().await;
    convert_current_fixture_to_v11(&database_path);

    let migrated = AsyncSqliteStateStore::open(&database_path)
        .await
        .expect("v11 database should migrate to v12");
    assert_eq!(migrated.schema_version().await, Ok(13));
    assert_eq!(
        migrated.list_account_routing_policies().await,
        Ok(vec![AccountRoutingPolicy::new(
            account_id.clone(),
            preserved_floor
        )])
    );
    migrated.close().await.expect("migrated store should close");

    let raw = Connection::open(&database_path).expect("v12 database should inspect");
    let table_sql: String = raw
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'account_routing_policies'",
                [],
                |row| row.get(0),
            )
            .expect("policy table SQL should read");
    assert!(table_sql.contains("BETWEEN 100 AND 1500"));
    assert_eq!(
        raw.execute(
            "INSERT INTO account_routing_policies VALUES ('acct_v12_fifteen', 1500)",
            [],
        ),
        Ok(1)
    );
    for invalid_basis_points in [1_501, 1_550, 1_600] {
        assert!(
            raw.execute(
                "INSERT INTO account_routing_policies VALUES (?1, ?2)",
                (
                    format!("acct_invalid_{invalid_basis_points}"),
                    invalid_basis_points,
                ),
            )
            .is_err(),
            "{invalid_basis_points} basis points must be rejected"
        );
    }
    let integrity: String = raw
        .pragma_query_value(None, "integrity_check", |row| row.get(0))
        .expect("integrity check should run");
    assert_eq!(integrity, "ok");
}

#[test]
fn v10_to_v11_sync_migration_rolls_back_atomically_and_retries_to_v12() {
    let temp_dir = TestTempDir::new("v11_sync_rollback_retry");
    let database_path = temp_dir.path().join("state.sqlite");
    let store = SqliteStateStore::open(&database_path).expect("fixture should open");
    drop(store);
    convert_current_fixture_to_v10(&database_path);

    SqliteStateStore::inject_v11_migration_rollback_for_test(&database_path)
        .expect_err("sync migration failure should be injected");
    let connection = Connection::open(&database_path).expect("fixture should reopen");
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .expect("version should read");
    assert_eq!(version, 10);
    assert!(
        connection
            .prepare("SELECT 1 FROM account_routing_policies")
            .is_err()
    );
    drop(connection);

    let migrated =
        SqliteStateStore::open(&database_path).expect("sync migration retry should succeed");
    assert_eq!(migrated.schema_version(), 13);
}

#[test]
fn v12_sync_migration_rolls_back_to_intact_v11_and_retries() {
    let temp_dir = TestTempDir::new("v12_sync_rollback_retry");
    let database_path = temp_dir.path().join("state.sqlite");
    let store = SqliteStateStore::open(&database_path).expect("current fixture should open");
    let account_id = account_id("acct_v12_sync_rollback");
    store
        .upsert_account(&AccountRecord::new(
            codex_router_core::provider::Provider::Openai,
            account_id.clone(),
            "v12-sync-rollback",
            AccountStatus::Enabled,
        ))
        .expect("account should persist");
    drop(store);
    let raw = Connection::open(&database_path).expect("fixture should reopen");
    raw.execute(
        "INSERT INTO account_routing_policies VALUES (?1, 900)",
        [account_id.as_str()],
    )
    .expect("policy should persist");
    drop(raw);
    convert_current_fixture_to_v11(&database_path);

    SqliteStateStore::inject_v12_migration_rollback_for_test(&database_path)
        .expect_err("v12 migration failure should be injected");
    assert_intact_v11_policy_database(&database_path, account_id.as_str(), 900);

    let migrated =
        SqliteStateStore::open(&database_path).expect("sync v12 migration retry should succeed");
    assert_eq!(migrated.schema_version(), 13);
    drop(migrated);
    let raw = Connection::open(&database_path).expect("migrated database should reopen");
    let preserved_floor: i64 = raw
            .query_row(
                "SELECT weekly_quota_floor_basis_points FROM account_routing_policies WHERE account_id = ?1",
                [account_id.as_str()],
                |row| row.get(0),
            )
            .expect("sync migration should preserve policy");
    assert_eq!(preserved_floor, 900);
}

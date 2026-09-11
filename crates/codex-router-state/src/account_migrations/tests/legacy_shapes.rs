use super::*;

#[tokio::test]
async fn v7_current_lease_shape_preserves_identity_pressure_and_empty_history() {
    let temporary_database = TemporaryDatabase::new("v7_current_lease");
    create_legacy_v7_current_lease_database(temporary_database.path()).await;

    let migrated = AsyncSqliteStateStore::open(temporary_database.path())
        .await
        .expect("v7 current lease fixture should migrate");
    let counts = migrated
        .active_client_counts_for_route_band_read_only("responses", 200, 300)
        .await
        .expect("preserved current lease should count");
    assert_eq!(
        counts,
        vec![crate::sqlite::ActiveClientCount::new(
            codex_router_core::ids::AccountId::new("preserved-account").expect("valid account"),
            1,
            5,
        )]
    );
    assert!(
        migrated
            .active_session_events_for_route_band("responses")
            .await
            .expect("session history should load")
            .is_empty(),
        "adoption must not fabricate history from a current lease"
    );
    migrated.close().await.expect("migrated store should close");

    let mut connection = open_test_connection(temporary_database.path(), false).await;
    let lease: (String, String, String, i64) = sqlx::query_as(
        "SELECT process_run_id, reservation_id, account_id, active_pressure
               FROM active_client_leases",
    )
    .fetch_one(&mut connection)
    .await
    .expect("current lease should remain exact");
    assert_eq!(
        lease,
        (
            "current-process".to_owned(),
            "current-reservation".to_owned(),
            "preserved-account".to_owned(),
            5,
        )
    );
    connection.close().await.expect("inspection should close");
    assert_native_history_present(temporary_database.path()).await;
}

#[tokio::test]
async fn v7_absent_lease_table_creates_empty_table_without_fabricated_history() {
    let temporary_database = TemporaryDatabase::new("v7_absent_lease");
    create_legacy_v7_without_lease_table(temporary_database.path()).await;

    let migrated = AsyncSqliteStateStore::open(temporary_database.path())
        .await
        .expect("v7 absent lease fixture should migrate");
    assert!(
        migrated
            .active_client_counts_for_route_band_read_only("responses", 200, 300)
            .await
            .expect("empty lease table should read")
            .is_empty()
    );
    assert!(
        migrated
            .active_session_events_for_route_band("responses")
            .await
            .expect("session history should load")
            .is_empty(),
        "adoption must not fabricate history for an absent lease table"
    );
    migrated.close().await.expect("migrated store should close");

    let mut connection = open_test_connection(temporary_database.path(), false).await;
    let lease_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM active_client_leases")
        .fetch_one(&mut connection)
        .await
        .expect("created lease table should query");
    assert_eq!(lease_rows, 0);
    connection.close().await.expect("inspection should close");
    assert_native_history_present(temporary_database.path()).await;
}

#[tokio::test]
async fn every_v0_initialization_prefix_completes_without_losing_existing_rows() {
    for prefix_length in 0..=LEGACY_V0_TABLE_STATEMENTS.len() {
        let temporary_database = TemporaryDatabase::new(&format!("v0_prefix_{prefix_length}"));
        let mut connection = open_test_connection(temporary_database.path(), true).await;
        for statement in &LEGACY_V0_TABLE_STATEMENTS[..prefix_length] {
            sqlx::raw_sql(*statement)
                .execute(&mut connection)
                .await
                .expect("v0 prefix statement should execute");
        }
        if prefix_length != 0 {
            sqlx::query("INSERT INTO accounts VALUES ('v0-account', 'prefix', 'disabled', 17)")
                .execute(&mut connection)
                .await
                .expect("v0 account should seed");
        }
        connection.close().await.expect("v0 fixture should close");

        let migrated = AsyncSqliteStateStore::open(temporary_database.path())
            .await
            .unwrap_or_else(|error| panic!("v0 prefix {prefix_length} should migrate: {error}"));
        if prefix_length != 0 {
            let account = migrated
                .load_account(
                    &codex_router_core::ids::AccountId::new("v0-account").expect("valid id"),
                )
                .await
                .expect("v0 account should load")
                .expect("v0 account should remain");
            assert_eq!(account.label(), "prefix");
            assert_eq!(account.status(), crate::account::AccountStatus::Disabled);
            assert_eq!(account.active_credential_generation(), Some(17));
        }
        migrated
            .close()
            .await
            .expect("v0 migrated store should close");
        assert_native_history_present(temporary_database.path()).await;
    }
}

#[tokio::test]
async fn v9_missing_history_fields_adopts_exact_defaults_and_preserves_rows() {
    for optional_field_mask in 0_u8..64 {
        let temporary_database =
            TemporaryDatabase::new(&format!("v9_history_fields_{optional_field_mask}"));
        create_legacy_v9_history_shape(temporary_database.path(), optional_field_mask).await;

        let migrated = AsyncSqliteStateStore::open(temporary_database.path())
            .await
            .unwrap_or_else(|error| {
                panic!("v9 history mask {optional_field_mask} should migrate: {error}")
            });
        migrated
            .close()
            .await
            .expect("v9 migrated store should close");

        let mut connection = open_test_connection(temporary_database.path(), false).await;
        let event: (String, i64, Option<i64>, String) = sqlx::query_as(
            "SELECT logical_session_id, session_started_unix_seconds,
                        session_ended_unix_seconds, transport_kind
                   FROM active_session_events WHERE id = 41",
        )
        .fetch_one(&mut connection)
        .await
        .expect("v9 event should remain");
        assert_eq!(
            event,
            (
                if optional_field_mask & 1 != 0 {
                    "logical-v9".to_owned()
                } else {
                    String::new()
                },
                if optional_field_mask & 2 != 0 { 88 } else { 0 },
                (optional_field_mask & 4 != 0).then_some(99),
                if optional_field_mask & 8 != 0 {
                    "websocket".to_owned()
                } else {
                    "unknown".to_owned()
                },
            ),
            "event values should match mask {optional_field_mask}"
        );
        let rollup: (i64, i64, i64) = sqlx::query_as(
            "SELECT active_session_seconds, completed_sessions, stale_purged_sessions
                   FROM active_session_rollups WHERE account_id = 'preserved-account'",
        )
        .fetch_one(&mut connection)
        .await
        .expect("v9 rollup should remain");
        assert_eq!(
            rollup,
            (
                77,
                if optional_field_mask & 16 != 0 { 3 } else { 0 },
                if optional_field_mask & 32 != 0 { 4 } else { 0 },
            ),
            "rollup values should match mask {optional_field_mask}"
        );
        connection.close().await.expect("inspection should close");
        assert_native_history_present(temporary_database.path()).await;
    }
}

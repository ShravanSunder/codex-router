use super::*;

#[tokio::test]
async fn provider_migration_preserves_copied_prechange_rows_and_rejects_unknown_values() {
    let original_database = TemporaryDatabase::new("prechange_provider_source");
    create_legacy_v13_database(original_database.path(), "first-label").await;
    let mut original_connection = open_test_connection(original_database.path(), false).await;
    sqlx::query("INSERT INTO accounts VALUES ('second-account', 'second-label', 'enabled', 7)")
        .execute(&mut original_connection)
        .await
        .expect("second legacy account should seed");
    sqlx::query(
        "INSERT INTO session_account_affinities VALUES (
            'second-session', 'second-account', 6789
         )",
    )
    .execute(&mut original_connection)
    .await
    .expect("second legacy pin should seed");
    original_connection
        .close()
        .await
        .expect("prechange source should close");

    let original_bytes =
        std::fs::read(original_database.path()).expect("prechange source bytes should read");
    let copied_database = TemporaryDatabase::new("copied_prechange_provider");
    std::fs::copy(original_database.path(), copied_database.path())
        .expect("prechange fixture should copy");
    let copied_bytes =
        std::fs::read(copied_database.path()).expect("copied prechange bytes should read");

    let migrated = AsyncSqliteStateStore::open(copied_database.path())
        .await
        .expect("copied prechange database should migrate");
    let accounts = migrated
        .list_accounts()
        .await
        .expect("migrated accounts should decode");
    assert_eq!(accounts.len(), 2);
    assert!(
        accounts
            .iter()
            .all(|account| account.provider() == codex_router_core::provider::Provider::Openai)
    );
    let mut migrated_connection = open_test_connection(copied_database.path(), false).await;
    let account_rows: Vec<(String, String, String, Option<i64>, String)> = sqlx::query_as(
        "SELECT account_id, label, status, active_credential_generation, provider
           FROM accounts ORDER BY account_id",
    )
    .fetch_all(&mut migrated_connection)
    .await
    .expect("provider accounts should read");
    assert_eq!(
        account_rows,
        [
            (
                "preserved-account".to_owned(),
                "first-label".to_owned(),
                "disabled".to_owned(),
                Some(41),
                "openai".to_owned(),
            ),
            (
                "second-account".to_owned(),
                "second-label".to_owned(),
                "enabled".to_owned(),
                Some(7),
                "openai".to_owned(),
            ),
        ]
    );
    let pin_rows: Vec<(String, String, String, i64, i64)> = sqlx::query_as(
        "SELECT provider, session_id, account_id, last_seen_unix_seconds, pin_version
           FROM session_account_affinities ORDER BY session_id",
    )
    .fetch_all(&mut migrated_connection)
    .await
    .expect("migrated pins should read");
    assert_eq!(
        pin_rows,
        [
            (
                "openai".to_owned(),
                "preserved-session".to_owned(),
                "preserved-account".to_owned(),
                5678,
                0,
            ),
            (
                "openai".to_owned(),
                "second-session".to_owned(),
                "second-account".to_owned(),
                6789,
                0,
            ),
        ]
    );
    migrated_connection
        .close()
        .await
        .expect("migration inspection should close");
    migrated.close().await.expect("migrated state should close");
    assert_eq!(
        std::fs::read(original_database.path()).expect("original source should remain"),
        original_bytes,
        "migration must operate on the copied database"
    );
    assert_eq!(
        copied_bytes, original_bytes,
        "fixture copy must begin byte-identical to its source"
    );

    let mut corrupt_connection = open_test_connection(copied_database.path(), false).await;
    sqlx::query(
        "UPDATE accounts SET provider = 'unrecognized' WHERE account_id = 'second-account'",
    )
    .execute(&mut corrupt_connection)
    .await
    .expect("invalid provider fixture should write");
    corrupt_connection
        .close()
        .await
        .expect("invalid provider fixture should close");
    let reopened = AsyncSqliteStateStore::open(copied_database.path())
        .await
        .expect("current database should reopen");
    let error = reopened
        .list_accounts()
        .await
        .expect_err("unknown provider must fail closed during row decoding");
    assert!(matches!(
        error,
        crate::sqlite::StateStoreError::CorruptAccount {
            field: "provider",
            ..
        }
    ));
    reopened.close().await.expect("reopened state should close");
}

#[tokio::test]
async fn provider_migration_preserves_copied_native_upgrade_rows_and_recreates_pin_index() {
    const CREDENTIAL_MAINTENANCE_VERSION: i64 = 202609250001;

    let native_source = TemporaryDatabase::new("native_provider_source");
    let initial_connection = open_test_connection(native_source.path(), true).await;
    initial_connection
        .close()
        .await
        .expect("native source file should close");
    let native_pool = open_test_pool(native_source.path()).await;
    MIGRATOR
        .run_to(CREDENTIAL_MAINTENANCE_VERSION, &native_pool)
        .await
        .expect("native schema should migrate through credential maintenance");
    native_pool.close().await;

    let mut native_connection = open_test_connection(native_source.path(), false).await;
    sqlx::query(
        "INSERT INTO accounts (account_id, label, status, active_credential_generation)
         VALUES ('native-first', 'native-first-label', 'disabled', 41),
                ('native-second', 'native-second-label', 'enabled', NULL)",
    )
    .execute(&mut native_connection)
    .await
    .expect("native accounts should seed");
    sqlx::query(
        "INSERT INTO session_account_affinities (
            session_id, account_id, last_seen_unix_seconds
         ) VALUES ('native-session', 'native-first', 5678)",
    )
    .execute(&mut native_connection)
    .await
    .expect("native pin should seed");
    sqlx::query(
        "INSERT INTO account_routing_policies (account_id, weekly_quota_floor_basis_points)
         VALUES ('native-first', 900)",
    )
    .execute(&mut native_connection)
    .await
    .expect("native routing policy should seed");
    sqlx::query(
        "INSERT INTO previous_response_affinity_owners (
            affinity_key_hash, route_band, account_id, credential_generation,
            source_transport, created_unix_seconds
         ) VALUES ('native-hash', 'responses', 'native-first', 41, 'websocket', 1234)",
    )
    .execute(&mut native_connection)
    .await
    .expect("native previous-response owner should seed");
    native_connection
        .close()
        .await
        .expect("seeded native source should close");

    assert_eq!(migration_history_bytes(native_source.path()).await.len(), 2);
    let native_source_bytes =
        std::fs::read(native_source.path()).expect("native source bytes should read");
    let copied_native_database = TemporaryDatabase::new("copied_native_provider_source");
    std::fs::copy(native_source.path(), copied_native_database.path())
        .expect("native upgrade source should copy");
    assert_eq!(
        std::fs::read(copied_native_database.path()).expect("native copy bytes should read"),
        native_source_bytes,
        "native migration must start from a byte-identical copy"
    );

    let migrated = AsyncSqliteStateStore::open(copied_native_database.path())
        .await
        .expect("copied native database should migrate to provider schema");
    let accounts = migrated
        .list_accounts()
        .await
        .expect("native migrated accounts should decode");
    assert_eq!(accounts.len(), 2);
    assert!(
        accounts
            .iter()
            .all(|account| { account.provider() == codex_router_core::provider::Provider::Openai })
    );

    let mut migrated_connection = open_test_connection(copied_native_database.path(), false).await;
    let account_rows: Vec<(String, String, String, Option<i64>, String)> = sqlx::query_as(
        "SELECT account_id, label, status, active_credential_generation, provider
           FROM accounts ORDER BY account_id",
    )
    .fetch_all(&mut migrated_connection)
    .await
    .expect("native migrated accounts should read");
    assert_eq!(
        account_rows,
        [
            (
                "native-first".to_owned(),
                "native-first-label".to_owned(),
                "disabled".to_owned(),
                Some(41),
                "openai".to_owned(),
            ),
            (
                "native-second".to_owned(),
                "native-second-label".to_owned(),
                "enabled".to_owned(),
                None,
                "openai".to_owned(),
            ),
        ]
    );

    let pin_rows: Vec<(String, String, Option<String>, i64, i64)> = sqlx::query_as(
        "SELECT provider, session_id, account_id, last_seen_unix_seconds, pin_version
           FROM session_account_affinities ORDER BY session_id",
    )
    .fetch_all(&mut migrated_connection)
    .await
    .expect("native migrated pin should read");
    assert_eq!(
        pin_rows,
        [(
            "openai".to_owned(),
            "native-session".to_owned(),
            Some("native-first".to_owned()),
            5678,
            0,
        )]
    );

    let policy_rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT account_id, weekly_quota_floor_basis_points
           FROM account_routing_policies ORDER BY account_id",
    )
    .fetch_all(&mut migrated_connection)
    .await
    .expect("native routing policy should read");
    assert_eq!(policy_rows, [("native-first".to_owned(), 900)]);

    let owner_rows: Vec<(String, String, String, i64, String, i64)> = sqlx::query_as(
        "SELECT affinity_key_hash, route_band, account_id, credential_generation,
                source_transport, created_unix_seconds
           FROM previous_response_affinity_owners ORDER BY affinity_key_hash",
    )
    .fetch_all(&mut migrated_connection)
    .await
    .expect("native previous-response owner should read");
    assert_eq!(
        owner_rows,
        [(
            "native-hash".to_owned(),
            "responses".to_owned(),
            "native-first".to_owned(),
            41,
            "websocket".to_owned(),
            1234,
        )]
    );

    let recreated_index: Option<(String, String)> = sqlx::query_as(
        "SELECT tbl_name, sql FROM sqlite_master
           WHERE type = 'index' AND name = 'session_account_affinities_last_seen_lookup'",
    )
    .fetch_optional(&mut migrated_connection)
    .await
    .expect("migrated pin index should be inspected");
    let (index_table, index_sql) = recreated_index.expect("pin lookup index should be recreated");
    assert_eq!(index_table, "session_account_affinities");
    assert!(index_sql.contains("last_seen_unix_seconds"));

    let history_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations")
        .fetch_one(&mut migrated_connection)
        .await
        .expect("native migration history should read");
    assert_eq!(history_rows, 5);
    migrated_connection
        .close()
        .await
        .expect("native migration inspection should close");
    migrated
        .close()
        .await
        .expect("native migrated state should close");
}

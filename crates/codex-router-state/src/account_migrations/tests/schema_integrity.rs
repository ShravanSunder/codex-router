use super::*;

#[tokio::test]
async fn legacy_owned_table_trigger_rejects_without_mutation() {
    let temporary_database = TemporaryDatabase::new("legacy_owned_trigger");
    create_legacy_v13_database(temporary_database.path(), "trigger-preserved").await;
    install_sql(
        temporary_database.path(),
        "CREATE TRIGGER accounts_insert_rewriter
         AFTER INSERT ON accounts
         BEGIN
            DELETE FROM accounts;
         END;",
    )
    .await;

    assert_incompatible_schema(temporary_database.path()).await;
    let mut connection = open_test_connection(temporary_database.path(), false).await;
    assert_eq!(legacy_version(&mut connection).await, 13);
    assert_eq!(account_label(&mut connection).await, "trigger-preserved");
    assert!(schema_object_exists(&mut connection, "trigger", "accounts_insert_rewriter").await);
    assert!(!schema_object_exists(&mut connection, "table", "_sqlx_migrations").await);
    connection.close().await.expect("inspection should close");
}

#[tokio::test]
async fn native_owned_table_trigger_rejects_with_history_unchanged() {
    let temporary_database = TemporaryDatabase::new("native_owned_trigger");
    let store = AsyncSqliteStateStore::open(temporary_database.path())
        .await
        .expect("native database should initialize");
    store.close().await.expect("native store should close");
    install_sql(
        temporary_database.path(),
        "CREATE TRIGGER accounts_insert_rewriter
         AFTER INSERT ON accounts
         BEGIN
            DELETE FROM accounts;
         END;",
    )
    .await;
    let history_before = migration_history_bytes(temporary_database.path()).await;

    assert_incompatible_schema(temporary_database.path()).await;
    assert_eq!(
        migration_history_bytes(temporary_database.path()).await,
        history_before
    );
    let mut connection = open_test_connection(temporary_database.path(), false).await;
    assert!(schema_object_exists(&mut connection, "trigger", "accounts_insert_rewriter").await);
    connection.close().await.expect("inspection should close");
}

#[tokio::test]
async fn extra_virtual_and_stored_generated_columns_reject_without_adoption() {
    for generated_kind in ["VIRTUAL", "STORED"] {
        let temporary_database = TemporaryDatabase::new(&format!(
            "extra_generated_{}",
            generated_kind.to_ascii_lowercase()
        ));
        create_legacy_v13_database(temporary_database.path(), "generated-preserved").await;
        let sql = format!(
            "ALTER TABLE accounts RENAME TO accounts_original;
             CREATE TABLE accounts (
                account_id TEXT PRIMARY KEY NOT NULL,
                label TEXT NOT NULL,
                status TEXT NOT NULL,
                active_credential_generation INTEGER,
                derived_label TEXT GENERATED ALWAYS AS (label || '-derived') {generated_kind}
             );
             INSERT INTO accounts (account_id, label, status, active_credential_generation)
                SELECT account_id, label, status, active_credential_generation
                  FROM accounts_original;
             DROP TABLE accounts_original;"
        );
        install_dynamic_sql(temporary_database.path(), sql).await;

        assert_incompatible_schema(temporary_database.path()).await;
        let mut connection = open_test_connection(temporary_database.path(), false).await;
        assert_eq!(account_label(&mut connection).await, "generated-preserved");
        let hidden: i64 = sqlx::query_scalar(
            "SELECT hidden FROM pragma_table_xinfo('accounts') WHERE name = 'derived_label'",
        )
        .fetch_one(&mut connection)
        .await
        .expect("generated column should remain");
        assert_ne!(hidden, 0);
        assert!(!schema_object_exists(&mut connection, "table", "_sqlx_migrations").await);
        connection.close().await.expect("inspection should close");
    }
}

#[tokio::test]
async fn canonical_name_generated_replacement_rejects() {
    let temporary_database = TemporaryDatabase::new("generated_canonical_name");
    create_legacy_v13_database(temporary_database.path(), "original-label").await;
    install_sql(
        temporary_database.path(),
        "ALTER TABLE accounts RENAME TO accounts_original;
         CREATE TABLE accounts (
            account_id TEXT PRIMARY KEY NOT NULL,
            label TEXT NOT NULL GENERATED ALWAYS AS ('derived-label') VIRTUAL,
            status TEXT NOT NULL,
            active_credential_generation INTEGER
         );
         INSERT INTO accounts (account_id, status, active_credential_generation)
            SELECT account_id, status, active_credential_generation FROM accounts_original;
         DROP TABLE accounts_original;",
    )
    .await;

    assert_incompatible_schema(temporary_database.path()).await;
    let mut connection = open_test_connection(temporary_database.path(), false).await;
    let row: (String, String, i64) =
        sqlx::query_as("SELECT label, status, active_credential_generation FROM accounts")
            .fetch_one(&mut connection)
            .await
            .expect("generated account should remain");
    assert_eq!(row, ("derived-label".to_owned(), "disabled".to_owned(), 41));
    let hidden: i64 = sqlx::query_scalar(
        "SELECT hidden FROM pragma_table_xinfo('accounts') WHERE name = 'label'",
    )
    .fetch_one(&mut connection)
    .await
    .expect("generated label should remain");
    assert_ne!(hidden, 0);
    assert!(!schema_object_exists(&mut connection, "table", "_sqlx_migrations").await);
    connection.close().await.expect("inspection should close");
}

#[tokio::test]
async fn v11_dependent_view_rejects_and_rolls_back_during_policy_rebuild() {
    let temporary_database = TemporaryDatabase::new("v11_dependent_view");
    create_legacy_v11_database(temporary_database.path()).await;
    install_sql(
        temporary_database.path(),
        "CREATE VIEW policy_view AS
         SELECT account_id, weekly_quota_floor_basis_points
           FROM account_routing_policies;",
    )
    .await;

    let error = AsyncSqliteStateStore::open(temporary_database.path())
        .await
        .expect_err("dependent view must reject");
    assert!(matches!(
        error,
        crate::sqlite::StateStoreError::Sqlite { .. }
    ));
    let mut connection = open_test_connection(temporary_database.path(), false).await;
    assert_eq!(legacy_version(&mut connection).await, 11);
    let policy: i64 = sqlx::query_scalar(
        "SELECT weekly_quota_floor_basis_points FROM account_routing_policies
          WHERE account_id = 'preserved-account'",
    )
    .fetch_one(&mut connection)
    .await
    .expect("policy should remain");
    assert_eq!(policy, 900);
    assert!(schema_object_exists(&mut connection, "view", "policy_view").await);
    let view_policy: i64 = sqlx::query_scalar(
        "SELECT weekly_quota_floor_basis_points FROM policy_view
          WHERE account_id = 'preserved-account'",
    )
    .fetch_one(&mut connection)
    .await
    .expect("dependent view should remain usable after rollback");
    assert_eq!(view_policy, 900);
    assert!(!schema_object_exists(&mut connection, "table", "_sqlx_migrations").await);
    connection.close().await.expect("inspection should close");
}

#[tokio::test]
async fn unrelated_table_trigger_survives_adoption() {
    let temporary_database = TemporaryDatabase::new("unrelated_trigger");
    create_legacy_v13_database(temporary_database.path(), "unrelated-trigger").await;
    install_sql(
        temporary_database.path(),
        "CREATE TABLE unrelated_log (value TEXT NOT NULL);
         CREATE TRIGGER unrelated_logger
         AFTER INSERT ON unrelated_log
         BEGIN
            UPDATE unrelated_log SET value = upper(value);
         END;",
    )
    .await;

    let migrated = AsyncSqliteStateStore::open(temporary_database.path())
        .await
        .expect("unrelated trigger should not block adoption");
    migrated.close().await.expect("migrated store should close");
    let mut connection = open_test_connection(temporary_database.path(), false).await;
    assert!(schema_object_exists(&mut connection, "trigger", "unrelated_logger").await);
    sqlx::query("INSERT INTO unrelated_log VALUES ('preserved')")
        .execute(&mut connection)
        .await
        .expect("unrelated trigger should execute");
    let value: String = sqlx::query_scalar("SELECT value FROM unrelated_log")
        .fetch_one(&mut connection)
        .await
        .expect("unrelated trigger result should read");
    assert_eq!(value, "PRESERVED");
    connection.close().await.expect("inspection should close");
}

async fn assert_incompatible_schema(database_path: &std::path::Path) {
    assert_eq!(
        AsyncSqliteStateStore::open(database_path)
            .await
            .expect_err("incompatible schema must reject"),
        crate::sqlite::StateStoreError::Sqlite {
            message: "incompatible account database schema".to_owned()
        }
    );
}

async fn install_sql(database_path: &std::path::Path, sql: &'static str) {
    let mut connection = open_test_connection(database_path, false).await;
    sqlx::raw_sql(sql)
        .execute(&mut connection)
        .await
        .expect("schema fixture should install");
    connection.close().await.expect("fixture should close");
}

async fn install_dynamic_sql(database_path: &std::path::Path, sql: String) {
    let mut connection = open_test_connection(database_path, false).await;
    sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
        .execute(&mut connection)
        .await
        .expect("generated schema fixture should install");
    connection.close().await.expect("fixture should close");
}

async fn legacy_version(connection: &mut sqlx::SqliteConnection) -> i64 {
    sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(connection)
        .await
        .expect("legacy version should read")
}

async fn account_label(connection: &mut sqlx::SqliteConnection) -> String {
    sqlx::query_scalar("SELECT label FROM accounts WHERE account_id = 'preserved-account'")
        .fetch_one(connection)
        .await
        .expect("account label should read")
}

async fn schema_object_exists(
    connection: &mut sqlx::SqliteConnection,
    object_type: &'static str,
    object_name: &'static str,
) -> bool {
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = ?1 AND name = ?2)")
        .bind(object_type)
        .bind(object_name)
        .fetch_one(connection)
        .await
        .expect("schema object presence should query")
}

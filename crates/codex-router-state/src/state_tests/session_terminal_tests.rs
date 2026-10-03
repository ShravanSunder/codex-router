use super::*;

#[tokio::test]
async fn sqlx_active_session_history_schema_matches_spec_terminal_counters() {
    let temp_dir = TestTempDir::new("async_active_session_schema_spec");
    let database_path = temp_dir.path().join("state.sqlite");
    let store = match AsyncSqliteStateStore::open(&database_path).await {
        Ok(store) => store,
        Err(error) => panic!("async state store should open and migrate: {error}"),
    };
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&format!("sqlite://{}", database_path.display()))
        .await
        .unwrap_or_else(|error| panic!("schema check pool should open: {error}"));

    let event_columns = sqlx::query("PRAGMA table_info(active_session_events)")
        .fetch_all(&pool)
        .await
        .unwrap_or_else(|error| panic!("active_session_events schema should load: {error}"))
        .into_iter()
        .map(|row| row.get::<String, _>("name"))
        .collect::<Vec<_>>();
    for required_column in [
        "logical_session_id",
        "session_started_unix_seconds",
        "session_ended_unix_seconds",
        "transport_kind",
    ] {
        assert!(
            event_columns.iter().any(|column| column == required_column),
            "active_session_events must include {required_column}: {event_columns:?}"
        );
    }

    let rollup_columns = sqlx::query("PRAGMA table_info(active_session_rollups)")
        .fetch_all(&pool)
        .await
        .unwrap_or_else(|error| panic!("active_session_rollups schema should load: {error}"))
        .into_iter()
        .map(|row| row.get::<String, _>("name"))
        .collect::<Vec<_>>();
    for required_column in ["completed_sessions", "stale_purged_sessions"] {
        assert!(
            rollup_columns
                .iter()
                .any(|column| column == required_column),
            "active_session_rollups must include {required_column}: {rollup_columns:?}"
        );
    }

    let account = account_id("acct_schema_spec");
    let released = ReservationId::new("reservation_schema_released");
    let stale = ReservationId::new("reservation_schema_stale");
    store
        .record_active_client_acquired("responses", "process-a", &released, &account, 10, 8)
        .await
        .unwrap_or_else(|error| panic!("released acquire should persist: {error}"));
    store
        .record_active_client_released("responses", "process-a", &released, 40)
        .await
        .unwrap_or_else(|error| panic!("released terminal event should persist: {error}"));
    store
        .record_active_client_acquired("responses", "process-a", &stale, &account, 50, 8)
        .await
        .unwrap_or_else(|error| panic!("stale acquire should persist: {error}"));
    store
        .active_client_counts_for_route_band("responses", 500, 300)
        .await
        .unwrap_or_else(|error| panic!("stale prune should persist terminal event: {error}"));
    store
        .refresh_active_session_rollups_for_interval("responses", 0, 600, 300)
        .await
        .unwrap_or_else(|error| panic!("rollups should refresh: {error}"));

    let rollups = store
        .active_session_rollups_for_route_band("responses", 0, 600)
        .await
        .unwrap_or_else(|error| panic!("rollups should load: {error}"));
    assert_eq!(
        rollups
            .iter()
            .map(crate::sqlite::ActiveSessionRollup::completed_sessions)
            .sum::<u32>(),
        1
    );
    assert_eq!(
        rollups
            .iter()
            .map(crate::sqlite::ActiveSessionRollup::stale_purged_sessions)
            .sum::<u32>(),
        1
    );
}

#[tokio::test]
async fn sqlx_active_session_stale_prune_records_terminal_event_and_rollup() {
    let temp_dir = TestTempDir::new("async_active_session_stale_prune");
    let database_path = temp_dir.path().join("state.sqlite");
    let store = match AsyncSqliteStateStore::open(&database_path).await {
        Ok(store) => store,
        Err(error) => panic!("async state store should open and migrate: {error}"),
    };
    let account = account_id("acct_stale_session");
    let reservation_id = ReservationId::new("reservation_stale_1");

    store
        .record_active_client_acquired(
            "responses",
            "process-stale",
            &reservation_id,
            &account,
            100,
            8,
        )
        .await
        .unwrap_or_else(|error| panic!("session acquire should persist: {error}"));

    let counts = store
        .active_client_counts_for_route_band("responses", 1_000, 300)
        .await
        .unwrap_or_else(|error| panic!("active counts should prune stale leases: {error}"));
    assert!(counts.is_empty(), "stale lease should no longer be active");

    let events = store
        .active_session_events_for_route_band("responses")
        .await
        .unwrap_or_else(|error| panic!("active session events should load: {error}"));
    assert_eq!(
        events,
        vec![
            crate::sqlite::ActiveSessionEvent::new(
                account.clone(),
                "responses",
                "process-stale",
                reservation_id.clone(),
                crate::sqlite::ActiveSessionEventKind::Acquired,
                100,
            ),
            crate::sqlite::ActiveSessionEvent::new(
                account.clone(),
                "responses",
                "process-stale",
                reservation_id,
                crate::sqlite::ActiveSessionEventKind::StalePurged,
                1_000,
            )
            .with_session_interval(100, Some(1_000), "unknown"),
        ]
    );

    store
        .refresh_active_session_rollups_for_interval("responses", 0, 1_200, 300)
        .await
        .unwrap_or_else(|error| panic!("rollups should refresh: {error}"));
    let rollups = store
        .active_session_rollups_for_route_band("responses", 0, 1_200)
        .await
        .unwrap_or_else(|error| panic!("rollups should load: {error}"));
    assert_eq!(
        rollups,
        vec![
            crate::sqlite::ActiveSessionRollup::new(account.clone(), "responses", 0, 300, 200, 1,),
            crate::sqlite::ActiveSessionRollup::new(account.clone(), "responses", 300, 600, 300, 1,),
            crate::sqlite::ActiveSessionRollup::new(account.clone(), "responses", 600, 900, 300, 1,),
            crate::sqlite::ActiveSessionRollup::new(account, "responses", 900, 1_200, 100, 1)
                .with_terminal_counts(0, 1),
        ]
    );
}

#[tokio::test]
async fn writable_selection_projection_terminalizes_stale_active_leases() {
    let temp_dir = TestTempDir::new("selection_projection_stale_lease_cleanup");
    let database_path = temp_dir.path().join("state.sqlite");
    let store = match AsyncSqliteStateStore::open(&database_path).await {
        Ok(store) => store,
        Err(error) => panic!("async state store should open and migrate: {error}"),
    };
    let account = account_id("acct_projection_stale_cleanup");
    let reservation_id = ReservationId::new("reservation_projection_stale_cleanup");
    let account_record = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account.clone(),
        "projection-stale",
        AccountStatus::Enabled,
    )
    .with_active_credential_generation(1);
    store
        .upsert_account(&account_record)
        .await
        .unwrap_or_else(|error| panic!("account should persist: {error}"));
    store
        .upsert_selector_quota_window(
            &PersistedSelectorQuotaWindow::new(
                account.clone(),
                "responses",
                V1_WEEKLY_WINDOW_SECONDS,
                SelectorQuotaWindowStatus::Eligible,
            )
            .with_remaining_headroom(90)
            .with_reset_unix_seconds(100_000)
            .with_effective(true)
            .with_observed_unix_seconds(100),
        )
        .await
        .unwrap_or_else(|error| panic!("selector window should persist: {error}"));
    store
        .record_active_client_acquired(
            "responses",
            "process-projection-stale",
            &reservation_id,
            &account,
            100,
            8,
        )
        .await
        .unwrap_or_else(|error| panic!("stale lease should acquire: {error}"));

    let projection = project_route_band_selection_inputs(&store, "responses", 1_000, 300)
        .await
        .unwrap_or_else(|error| panic!("writable projection should load: {error}"));

    assert_eq!(projection.accounts().len(), 1);
    assert_eq!(projection.accounts()[0].current_active_sessions(), 0);
    let events = store
        .active_session_events_for_route_band("responses")
        .await
        .unwrap_or_else(|error| panic!("active session events should load: {error}"));
    assert!(
        events.iter().any(|event| {
            event.event_kind() == crate::sqlite::ActiveSessionEventKind::StalePurged
                && event.reservation_id() == &reservation_id
        }),
        "writable projection should terminalize stale persisted leases"
    );
}

#[tokio::test]
async fn sqlx_v8_migration_preserves_current_leases_without_synthetic_session_history() {
    let temp_dir = TestTempDir::new("async_v8_active_session_migration");
    let database_path = temp_dir.path().join("state.sqlite");
    create_v8_database_with_current_active_lease(&database_path);

    let store = match AsyncSqliteStateStore::open(&database_path).await {
        Ok(store) => store,
        Err(error) => panic!("async v8 state store should migrate to v9: {error}"),
    };

    assert_eq!(
        store
            .schema_version()
            .await
            .unwrap_or_else(|error| panic!("schema version should load: {error}")),
        13
    );
    assert_eq!(
        store
            .active_client_counts_for_route_band("responses", 150, 300)
            .await
            .unwrap_or_else(|error| panic!("active lease should survive migration: {error}")),
        vec![crate::sqlite::ActiveClientCount::new(
            account_id("acct_v8_active_lease"),
            1,
            8,
        )]
    );
    assert!(
        store
            .active_session_events_for_route_band("responses")
            .await
            .unwrap_or_else(|error| panic!("session history should load: {error}"))
            .is_empty(),
        "v9 migration must not synthesize completed session history from current leases"
    );
}

#[tokio::test]
async fn sqlx_current_schema_repairs_partial_v10_active_session_columns() {
    let temp_dir = TestTempDir::new("async_partial_v10_active_session_repair");
    let database_path = temp_dir.path().join("state.sqlite");
    create_partial_v10_database_missing_active_session_columns(&database_path);

    let store = match AsyncSqliteStateStore::open(&database_path).await {
        Ok(store) => store,
        Err(error) => panic!("partial v10 state store should open and repair: {error}"),
    };
    let account = account_id("acct_partial_v10");
    let reservation = ReservationId::new("reservation-partial-v10");

    store
        .record_active_client_acquired("models", "process-partial", &reservation, &account, 100, 1)
        .await
        .unwrap_or_else(|error| {
            panic!("active client mirror should write after partial v10 repair: {error}")
        });

    assert_eq!(
        store
            .active_client_counts_for_route_band("models", 120, 300)
            .await
            .unwrap_or_else(|error| panic!("active client counts should load: {error}")),
        vec![crate::sqlite::ActiveClientCount::new(account, 1, 1)]
    );
}

#[tokio::test]
async fn sqlx_active_session_rollup_retention_purges_old_buckets() {
    let temp_dir = TestTempDir::new("async_active_session_rollup_retention");
    let database_path = temp_dir.path().join("state.sqlite");
    let store = match AsyncSqliteStateStore::open(&database_path).await {
        Ok(store) => store,
        Err(error) => panic!("async state store should open and migrate: {error}"),
    };
    let account = account_id("acct_rollup_retention");
    let reservation_id = ReservationId::new("reservation_retention_1");

    store
        .record_active_client_acquired(
            "responses",
            "process-retention",
            &reservation_id,
            &account,
            0,
            8,
        )
        .await
        .unwrap_or_else(|error| panic!("session acquire should persist: {error}"));
    store
        .record_active_client_released("responses", "process-retention", &reservation_id, 700)
        .await
        .unwrap_or_else(|error| panic!("session release should persist: {error}"));
    store
        .refresh_active_session_rollups_for_interval("responses", 0, 900, 300)
        .await
        .unwrap_or_else(|error| panic!("rollups should refresh: {error}"));

    store
        .purge_active_session_rollups_before(600)
        .await
        .unwrap_or_else(|error| panic!("old rollups should purge: {error}"));
    let rollups = store
        .active_session_rollups_for_route_band("responses", 0, 900)
        .await
        .unwrap_or_else(|error| panic!("rollups should load after purge: {error}"));
    assert_eq!(
        rollups,
        vec![
            crate::sqlite::ActiveSessionRollup::new(account.clone(), "responses", 300, 600, 300, 1,),
            crate::sqlite::ActiveSessionRollup::new(account, "responses", 600, 900, 100, 1)
                .with_terminal_counts(1, 0),
        ]
    );
}

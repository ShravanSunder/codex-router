use super::*;

#[tokio::test]
async fn sqlx_active_session_events_retain_completed_sessions_after_release() {
    let temp_dir = TestTempDir::new("async_active_session_events");
    let database_path = temp_dir.path().join("state.sqlite");
    let store = match AsyncSqliteStateStore::open(&database_path).await {
        Ok(store) => store,
        Err(error) => panic!("async state store should open and migrate: {error}"),
    };
    let account = account_id("acct_completed_session");
    let reservation_id = ReservationId::new("reservation_completed_1");

    store
        .record_active_client_acquired(
            "responses",
            "process-a",
            &reservation_id,
            &account,
            1_000,
            8,
        )
        .await
        .unwrap_or_else(|error| panic!("session acquire should persist: {error}"));
    store
        .record_active_client_released("responses", "process-a", &reservation_id, 1_100)
        .await
        .unwrap_or_else(|error| panic!("session release should persist: {error}"));

    let counts = store
        .active_client_counts_for_route_band("responses", 1_100, 300)
        .await
        .unwrap_or_else(|error| panic!("active counts should load: {error}"));
    assert!(
        counts.is_empty(),
        "released sessions should not remain active"
    );

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
                "process-a",
                reservation_id.clone(),
                crate::sqlite::ActiveSessionEventKind::Acquired,
                1_000,
            ),
            crate::sqlite::ActiveSessionEvent::new(
                account,
                "responses",
                "process-a",
                reservation_id,
                crate::sqlite::ActiveSessionEventKind::Released,
                1_100,
            )
            .with_session_interval(1_000, Some(1_100), "unknown"),
        ]
    );
}

#[tokio::test]
async fn sqlx_active_session_event_compaction_purges_only_completed_old_sessions() {
    let temp_dir = TestTempDir::new("async_active_session_event_compaction");
    let database_path = temp_dir.path().join("state.sqlite");
    let store = match AsyncSqliteStateStore::open(&database_path).await {
        Ok(store) => store,
        Err(error) => panic!("async state store should open and migrate: {error}"),
    };
    let account = account_id("acct_event_compaction");
    let old_completed = ReservationId::new("reservation-old-completed");
    let exact_cutoff_completed = ReservationId::new("reservation-exact-cutoff-completed");
    let old_open = ReservationId::new("reservation-old-open");
    let after_cutoff_completed = ReservationId::new("reservation-after-cutoff-completed");
    let other_route = ReservationId::new("reservation-other-route");

    for (
        route_band,
        process_run_id,
        reservation_id,
        acquired_unix_seconds,
        released_unix_seconds,
    ) in [
        ("responses", "process-old", &old_completed, 100, Some(200)),
        ("responses", "process-open", &old_open, 100, None),
        (
            "responses",
            "process-exact-cutoff",
            &exact_cutoff_completed,
            300,
            Some(500),
        ),
        (
            "responses",
            "process-after-cutoff",
            &after_cutoff_completed,
            400,
            Some(501),
        ),
        (
            "models",
            "process-other-route",
            &other_route,
            100,
            Some(200),
        ),
    ] {
        store
            .record_active_client_acquired(
                route_band,
                process_run_id,
                reservation_id,
                &account,
                acquired_unix_seconds,
                1,
            )
            .await
            .unwrap_or_else(|error| panic!("session acquire should persist: {error}"));
        if let Some(released_unix_seconds) = released_unix_seconds {
            store
                .record_active_client_released(
                    route_band,
                    process_run_id,
                    reservation_id,
                    released_unix_seconds,
                )
                .await
                .unwrap_or_else(|error| panic!("session release should persist: {error}"));
        }
    }

    store
        .compact_completed_active_session_events_before("responses", 500)
        .await
        .unwrap_or_else(|error| panic!("completed old events should compact: {error}"));

    let response_events = store
        .active_session_events_for_route_band("responses")
        .await
        .unwrap_or_else(|error| panic!("response events should load: {error}"));
    assert_eq!(
        response_events,
        vec![
            crate::sqlite::ActiveSessionEvent::new(
                account.clone(),
                "responses",
                "process-open",
                old_open,
                crate::sqlite::ActiveSessionEventKind::Acquired,
                100,
            ),
            crate::sqlite::ActiveSessionEvent::new(
                account.clone(),
                "responses",
                "process-exact-cutoff",
                exact_cutoff_completed.clone(),
                crate::sqlite::ActiveSessionEventKind::Acquired,
                300,
            ),
            crate::sqlite::ActiveSessionEvent::new(
                account.clone(),
                "responses",
                "process-after-cutoff",
                after_cutoff_completed.clone(),
                crate::sqlite::ActiveSessionEventKind::Acquired,
                400,
            ),
            crate::sqlite::ActiveSessionEvent::new(
                account.clone(),
                "responses",
                "process-exact-cutoff",
                exact_cutoff_completed,
                crate::sqlite::ActiveSessionEventKind::Released,
                500,
            )
            .with_session_interval(300, Some(500), "unknown"),
            crate::sqlite::ActiveSessionEvent::new(
                account,
                "responses",
                "process-after-cutoff",
                after_cutoff_completed,
                crate::sqlite::ActiveSessionEventKind::Released,
                501,
            )
            .with_session_interval(400, Some(501), "unknown"),
        ]
    );

    assert_eq!(
        store
            .active_session_events_for_route_band("models")
            .await
            .unwrap_or_else(|error| panic!("other-route events should load: {error}"))
            .len(),
        2,
        "compaction must stay within its route band"
    );
}

#[tokio::test]
async fn sqlx_active_session_release_timestamp_bounds_rollup_interval() {
    let temp_dir = TestTempDir::new("async_active_session_release_timestamp");
    let database_path = temp_dir.path().join("state.sqlite");
    let store = match AsyncSqliteStateStore::open(&database_path).await {
        Ok(store) => store,
        Err(error) => panic!("async state store should open and migrate: {error}"),
    };
    let account = account_id("acct_release_timestamp");
    let reservation_id = ReservationId::new("reservation_release_timestamp");

    store
        .record_active_client_acquired(
            "responses",
            "process-release",
            &reservation_id,
            &account,
            1_000,
            8,
        )
        .await
        .unwrap_or_else(|error| panic!("session acquire should persist: {error}"));
    store
        .record_active_client_released("responses", "process-release", &reservation_id, 1_100)
        .await
        .unwrap_or_else(|error| panic!("session release should persist: {error}"));
    store
        .refresh_active_session_rollups_for_interval("responses", 1_000, 1_300, 300)
        .await
        .unwrap_or_else(|error| panic!("rollups should refresh: {error}"));

    let rollups = store
        .active_session_rollups_for_route_band("responses", 1_000, 1_300)
        .await
        .unwrap_or_else(|error| panic!("rollups should load: {error}"));
    let active_session_seconds = rollups
        .iter()
        .map(crate::sqlite::ActiveSessionRollup::active_session_seconds)
        .sum::<u64>();
    assert_eq!(active_session_seconds, 100);
}

#[tokio::test]
async fn sqlx_active_session_rollups_clip_partial_buckets_and_overlap() {
    let temp_dir = TestTempDir::new("async_active_session_rollups");
    let database_path = temp_dir.path().join("state.sqlite");
    let store = match AsyncSqliteStateStore::open(&database_path).await {
        Ok(store) => store,
        Err(error) => panic!("async state store should open and migrate: {error}"),
    };
    let account = account_id("acct_rollup_session");
    let first = ReservationId::new("reservation_rollup_1");
    let second = ReservationId::new("reservation_rollup_2");

    store
        .record_active_client_acquired("responses", "process-a", &first, &account, 100, 8)
        .await
        .unwrap_or_else(|error| panic!("first acquire should persist: {error}"));
    store
        .record_active_client_acquired("responses", "process-a", &second, &account, 160, 8)
        .await
        .unwrap_or_else(|error| panic!("second acquire should persist: {error}"));
    store
        .record_active_client_released("responses", "process-a", &first, 220)
        .await
        .unwrap_or_else(|error| panic!("first release should persist: {error}"));
    store
        .record_active_client_released("responses", "process-a", &second, 260)
        .await
        .unwrap_or_else(|error| panic!("second release should persist: {error}"));

    store
        .refresh_active_session_rollups_for_interval("responses", 120, 240, 300)
        .await
        .unwrap_or_else(|error| panic!("rollups should refresh: {error}"));
    let rollups = store
        .active_session_rollups_for_route_band("responses", 120, 240)
        .await
        .unwrap_or_else(|error| panic!("rollups should load: {error}"));

    assert_eq!(
        rollups,
        vec![
            crate::sqlite::ActiveSessionRollup::new(account, "responses", 0, 300, 180, 2)
                .with_terminal_counts(1, 0)
        ]
    );
}

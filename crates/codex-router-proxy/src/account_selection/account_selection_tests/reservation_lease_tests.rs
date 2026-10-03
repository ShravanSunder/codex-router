use super::*;

#[test]
fn active_reservation_rereserve_keeps_external_lease_reporter() {
    let account_id = AccountId::new("acct_selected")
        .unwrap_or_else(|error| panic!("test account id should parse: {error}"));
    let active_reservations = RouteBandReservationBooks::default();
    let reservation_handle = {
        let mut reservations = active_reservations
            .lock()
            .unwrap_or_else(|error| panic!("reservations lock should be available: {error}"));
        reservations
            .entry("responses".to_owned())
            .or_insert_with(ReservationBook::default)
            .reserve_next_at(account_id, super::ACTIVE_SESSION_RESERVATION_UNITS, 1)
    };
    let reporter = Arc::new(RecordingLeaseReporter::default());
    let guard = ActiveReservationGuard::new_with_active_client_leases(
        Arc::clone(&active_reservations),
        "responses".to_owned(),
        reservation_handle,
        Some(reporter.clone()),
    );

    let rereserved = guard
        .reserve_again_at(2)
        .unwrap_or_else(|| panic!("re-reservation should succeed"));
    drop(rereserved);

    assert_eq!(
        reporter.events(),
        vec![
            LeaseEvent::Acquired {
                route_band: "responses".to_owned(),
                reservation_id: "reservation_2".to_owned(),
                account_id: "acct_selected".to_owned(),
                acquired_unix_seconds: 2,
                active_pressure: super::ACTIVE_SESSION_RESERVATION_UNITS,
            },
            LeaseEvent::Released {
                route_band: "responses".to_owned(),
                reservation_id: "reservation_2".to_owned(),
            },
        ]
    );
}

#[tokio::test]
async fn sqlite_active_client_lease_reporter_records_release_clock_timestamp() {
    let database_path = proxy_test_database_path("sqlite_active_client_release_clock");
    let store = AsyncSqliteStateStore::open(&database_path)
        .await
        .unwrap_or_else(|error| panic!("async state store should open and migrate: {error}"));
    let db_write_actor =
        DbWriteActor::start(Arc::new(SqliteDbWriteRepository::new(store.clone())), 4);
    let reporter = SqliteActiveClientLeaseReporter::new(db_write_actor.clone(), Arc::new(|| 1_100));
    let account_id = account_id("acct_release_clock");
    let mut reservation_book = ReservationBook::default();
    let reservation_handle = reservation_book.reserve_next_at(account_id, 1, 1_000);

    reporter.record_acquired("responses", &reservation_handle, 1_000, 1);
    wait_for_active_client_count(&store, "responses", 1, 1_050).await;
    reporter.record_released("responses", &reservation_handle);
    wait_for_active_client_count(&store, "responses", 0, 1_150).await;
    db_write_actor.shutdown().await;

    store
        .refresh_active_session_rollups_for_interval("responses", 1_000, 1_300, 300)
        .await
        .unwrap_or_else(|error| panic!("rollups should refresh: {error}"));
    let active_session_seconds = store
        .active_session_rollups_for_route_band("responses", 1_000, 1_300)
        .await
        .unwrap_or_else(|error| panic!("rollups should load: {error}"))
        .iter()
        .map(|rollup| rollup.active_session_seconds())
        .sum::<u64>();

    assert_eq!(
        active_session_seconds, 100,
        "release must use the reporter clock timestamp instead of epoch zero"
    );
}

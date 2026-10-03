use super::*;

#[tokio::test]
async fn sqlx_active_client_leases_count_release_and_prune() {
    let temp_dir = TestTempDir::new("async_active_client_leases");
    let database_path = temp_dir.path().join("state.sqlite");
    let store = match AsyncSqliteStateStore::open(&database_path).await {
        Ok(store) => store,
        Err(error) => panic!("async state store should open and migrate: {error}"),
    };
    let alpha = account_id("acct_active_alpha");
    let beta = account_id("acct_active_beta");

    if let Err(error) = store
        .record_active_client_acquired(
            "responses",
            "process-a",
            &ReservationId::new("reservation_alpha_1"),
            &alpha,
            1_000,
            2,
        )
        .await
    {
        panic!("alpha first lease should persist: {error}");
    }
    if let Err(error) = store
        .record_active_client_acquired(
            "responses",
            "process-a",
            &ReservationId::new("reservation_alpha_2"),
            &alpha,
            1_005,
            8,
        )
        .await
    {
        panic!("alpha second lease should persist: {error}");
    }
    if let Err(error) = store
        .record_active_client_acquired(
            "responses",
            "process-a",
            &ReservationId::new("reservation_beta_1"),
            &beta,
            1_006,
            8,
        )
        .await
    {
        panic!("beta lease should persist: {error}");
    }

    let counts = match store
        .active_client_counts_for_route_band("responses", 1_010, 100)
        .await
    {
        Ok(counts) => counts,
        Err(error) => panic!("active client counts should load: {error}"),
    };
    assert_eq!(
        counts,
        vec![
            crate::sqlite::ActiveClientCount::new(alpha.clone(), 2, 10),
            crate::sqlite::ActiveClientCount::new(beta.clone(), 1, 8),
        ]
    );

    if let Err(error) = store
        .record_active_client_released(
            "responses",
            "process-a",
            &ReservationId::new("reservation_alpha_1"),
            1_010,
        )
        .await
    {
        panic!("alpha lease should release: {error}");
    }
    let counts = match store
        .active_client_counts_for_route_band("responses", 1_010, 100)
        .await
    {
        Ok(counts) => counts,
        Err(error) => panic!("active client counts should reload: {error}"),
    };
    assert_eq!(
        counts,
        vec![
            crate::sqlite::ActiveClientCount::new(alpha, 1, 8),
            crate::sqlite::ActiveClientCount::new(beta, 1, 8),
        ]
    );

    let counts = match store
        .active_client_counts_for_route_band("responses", 1_200, 100)
        .await
    {
        Ok(counts) => counts,
        Err(error) => panic!("stale active client counts should prune: {error}"),
    };
    assert!(counts.is_empty());
}

#[tokio::test]
async fn sqlx_active_client_leases_do_not_collide_across_process_runs() {
    let temp_dir = TestTempDir::new("async_active_client_process_collision");
    let database_path = temp_dir.path().join("state.sqlite");
    let store = match AsyncSqliteStateStore::open(&database_path).await {
        Ok(store) => store,
        Err(error) => panic!("async state store should open and migrate: {error}"),
    };
    let account = account_id("acct_active_shared");
    let reservation_id = ReservationId::new("reservation_1");

    if let Err(error) = store
        .record_active_client_acquired(
            "responses",
            "process-a",
            &reservation_id,
            &account,
            1_000,
            2,
        )
        .await
    {
        panic!("process-a lease should persist: {error}");
    }
    if let Err(error) = store
        .record_active_client_acquired(
            "responses",
            "process-b",
            &reservation_id,
            &account,
            1_001,
            8,
        )
        .await
    {
        panic!("process-b lease should persist without overwriting process-a: {error}");
    }

    let counts = match store
        .active_client_counts_for_route_band("responses", 1_010, 100)
        .await
    {
        Ok(counts) => counts,
        Err(error) => panic!("active client counts should load: {error}"),
    };
    assert_eq!(
        counts,
        vec![crate::sqlite::ActiveClientCount::new(
            account.clone(),
            2,
            10
        )]
    );

    if let Err(error) = store
        .record_active_client_released("responses", "process-a", &reservation_id, 1_010)
        .await
    {
        panic!("process-a release should not delete process-b lease: {error}");
    }
    let counts = match store
        .active_client_counts_for_route_band("responses", 1_010, 100)
        .await
    {
        Ok(counts) => counts,
        Err(error) => panic!("active client counts should reload: {error}"),
    };
    assert_eq!(
        counts,
        vec![crate::sqlite::ActiveClientCount::new(account, 1, 8)]
    );
}

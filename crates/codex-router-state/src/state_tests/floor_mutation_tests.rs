use super::*;

#[tokio::test]
async fn policy_bulk_read_is_independent_and_unknown_account_is_redacted() {
    let temp_dir = TestTempDir::new("weekly_floor_bulk_read");
    let database_path = temp_dir.path().join("state.sqlite");
    let store = AsyncSqliteStateStore::open(&database_path)
        .await
        .unwrap_or_else(|error| panic!("state should open: {error}"));
    let account_a = account_id("acct_policy_a");
    let account_b = account_id("acct_policy_b");
    for (account_id, label) in [(&account_a, "policy-a"), (&account_b, "policy-b")] {
        store
            .upsert_account(&AccountRecord::new(
                codex_router_core::provider::Provider::Openai,
                account_id.clone(),
                label,
                AccountStatus::Enabled,
            ))
            .await
            .unwrap_or_else(|error| panic!("account should persist: {error}"));
    }
    store.close().await.expect("state should close");

    let mutation = AsyncWeeklyQuotaFloorMutationStore::open(&database_path)
        .await
        .unwrap_or_else(|error| panic!("mutation store should open: {error}"));
    let floor_a = WeeklyQuotaFloorBasisPoints::new(300).expect("valid floor");
    let floor_b = WeeklyQuotaFloorBasisPoints::new(900).expect("valid floor");
    mutation
        .set_weekly_quota_floor_by_label("policy-a", Some(floor_a))
        .await
        .expect("policy A should persist");
    mutation
        .set_weekly_quota_floor_by_label("policy-b", Some(floor_b))
        .await
        .expect("policy B should persist");
    let missing_label = "sensitive-missing-label";
    let error = mutation
        .set_weekly_quota_floor_by_label(missing_label, Some(floor_a))
        .await
        .expect_err("unknown account must fail");
    assert_eq!(error, StateStoreError::WeeklyQuotaFloorAccountNotFound);
    assert!(!error.to_string().contains(missing_label));
    mutation.close().await;

    let read_only = AsyncSqliteStateStore::open_read_only(&database_path)
        .await
        .expect("state should reopen read-only");
    assert_eq!(
        read_only.list_account_routing_policies().await,
        Ok(vec![
            AccountRoutingPolicy::new(account_a, floor_a),
            AccountRoutingPolicy::new(account_b, floor_b),
        ])
    );
}

#[tokio::test]
async fn weekly_floor_label_resolution_rejects_duplicate_exact_labels_without_mutation() {
    let temp_dir = TestTempDir::new("weekly_floor_duplicate_label");
    let database_path = temp_dir.path().join("state.sqlite");
    let state = AsyncSqliteStateStore::open(&database_path)
        .await
        .expect("state should open");
    for account_id_value in ["acct_duplicate_a", "acct_duplicate_b"] {
        state
            .upsert_account(&AccountRecord::new(
                codex_router_core::provider::Provider::Openai,
                account_id(account_id_value),
                "duplicate-label",
                AccountStatus::Enabled,
            ))
            .await
            .expect("duplicate-label account should persist");
    }
    let mutation = AsyncWeeklyQuotaFloorMutationStore::open(&database_path)
        .await
        .expect("mutation store should open");
    let error = mutation
        .set_weekly_quota_floor_by_label(
            "duplicate-label",
            Some(WeeklyQuotaFloorBasisPoints::new(500).expect("valid floor")),
        )
        .await
        .expect_err("duplicate exact labels must fail");
    assert_eq!(
        error,
        StateStoreError::WeeklyQuotaFloorAccountLabelAmbiguous
    );
    assert!(!error.to_string().contains("duplicate-label"));
    assert_eq!(state.list_account_routing_policies().await, Ok(Vec::new()));
    let target_account_id = account_id("acct_duplicate_b");
    let target_floor = WeeklyQuotaFloorBasisPoints::new(500).expect("valid floor");
    assert_eq!(
        mutation
            .set_weekly_quota_floor_by_account_id(&target_account_id, Some(target_floor))
            .await,
        Ok(WeeklyQuotaFloorMutationResult::Enabled(
            AccountRoutingPolicy::new(target_account_id.clone(), target_floor)
        ))
    );
    assert_eq!(
        state.list_account_routing_policies().await,
        Ok(vec![AccountRoutingPolicy::new(
            target_account_id,
            target_floor
        )])
    );
    mutation.close().await;
}

#[tokio::test]
async fn weekly_floor_transaction_resolves_current_account_label_metadata() {
    let temp_dir = TestTempDir::new("weekly_floor_current_label");
    let database_path = temp_dir.path().join("state.sqlite");
    let state = AsyncSqliteStateStore::open(&database_path)
        .await
        .expect("state should open");
    let account_id = account_id("acct_current_label");
    state
        .upsert_account(&AccountRecord::new(
            codex_router_core::provider::Provider::Openai,
            account_id.clone(),
            "old-label",
            AccountStatus::Enabled,
        ))
        .await
        .expect("old label should persist");
    let mutation = AsyncWeeklyQuotaFloorMutationStore::open(&database_path)
        .await
        .expect("mutation store should open");

    state
        .upsert_account(&AccountRecord::new(
            codex_router_core::provider::Provider::Openai,
            account_id.clone(),
            "new-label",
            AccountStatus::Enabled,
        ))
        .await
        .expect("new label should replace old metadata");
    let floor = WeeklyQuotaFloorBasisPoints::new(800).expect("valid floor");
    assert_eq!(
        mutation
            .set_weekly_quota_floor_by_label("old-label", Some(floor))
            .await,
        Err(StateStoreError::WeeklyQuotaFloorAccountNotFound)
    );
    assert_eq!(
        mutation
            .set_weekly_quota_floor_by_label("new-label", Some(floor))
            .await,
        Ok(WeeklyQuotaFloorMutationResult::Enabled(
            AccountRoutingPolicy::new(account_id, floor)
        ))
    );
    mutation.close().await;
}

#[tokio::test]
async fn weekly_floor_mutation_busy_retry_is_bounded_and_preserves_value() {
    let temp_dir = TestTempDir::new("weekly_floor_busy");
    let database_path = temp_dir.path().join("state.sqlite");
    let store = AsyncSqliteStateStore::open(&database_path)
        .await
        .expect("state should open");
    let account_id = account_id("acct_policy_busy");
    store
        .upsert_account(&AccountRecord::new(
            codex_router_core::provider::Provider::Openai,
            account_id.clone(),
            "busy-policy",
            AccountStatus::Enabled,
        ))
        .await
        .expect("account should persist");
    store.close().await.expect("state should close");

    let mutation = AsyncWeeklyQuotaFloorMutationStore::open(&database_path)
        .await
        .expect("mutation store should open");
    let old_floor = WeeklyQuotaFloorBasisPoints::new(400).expect("valid old floor");
    mutation
        .set_weekly_quota_floor_by_label("busy-policy", Some(old_floor))
        .await
        .expect("old floor should persist");

    let lock = Connection::open(&database_path).expect("lock connection should open");
    lock.execute_batch("PRAGMA busy_timeout = 0; BEGIN IMMEDIATE;")
        .expect("writer lock should be held");
    let concurrent_reader = AsyncSqliteStateStore::open_read_only(&database_path)
        .await
        .expect("WAL reader should remain responsive during a held writer");
    assert_eq!(
        concurrent_reader.list_account_routing_policies().await,
        Ok(vec![AccountRoutingPolicy::new(
            account_id.clone(),
            old_floor
        )])
    );
    concurrent_reader
        .close()
        .await
        .expect("reader should close");
    let started_at = std::time::Instant::now();
    let error = mutation
        .set_weekly_quota_floor_by_account_id(
            &account_id,
            Some(WeeklyQuotaFloorBasisPoints::new(700).expect("valid new floor")),
        )
        .await
        .expect_err("held writer must exhaust retries");
    let elapsed = started_at.elapsed();
    assert_eq!(error, StateStoreError::WeeklyQuotaFloorDatabaseBusy);
    assert!(elapsed >= std::time::Duration::from_millis(150));
    assert!(elapsed < std::time::Duration::from_millis(250));
    lock.execute_batch("ROLLBACK;")
        .expect("lock should release");
    mutation.close().await;

    let read_only = AsyncSqliteStateStore::open_read_only(&database_path)
        .await
        .expect("state should reopen read-only");
    assert_eq!(
        read_only.list_account_routing_policies().await,
        Ok(vec![AccountRoutingPolicy::new(account_id, old_floor)])
    );
}

#[tokio::test]
async fn weekly_floor_mutation_commits_after_held_writer_releases() {
    let temp_dir = TestTempDir::new("weekly_floor_busy_then_release");
    let database_path = temp_dir.path().join("state.sqlite");
    let store = AsyncSqliteStateStore::open(&database_path)
        .await
        .expect("state should open");
    let account_id = account_id("acct_policy_busy_then_release");
    store
        .upsert_account(&AccountRecord::new(
            codex_router_core::provider::Provider::Openai,
            account_id.clone(),
            "busy-then-release",
            AccountStatus::Enabled,
        ))
        .await
        .expect("account should persist");
    store.close().await.expect("state should close");

    let lock = Connection::open(&database_path).expect("lock connection should open");
    lock.execute_batch("PRAGMA busy_timeout = 0; BEGIN IMMEDIATE;")
        .expect("writer lock should be held");
    let mutation = Arc::new(
        AsyncWeeklyQuotaFloorMutationStore::open(&database_path)
            .await
            .expect("mutation store should open"),
    );
    let committed_floor = WeeklyQuotaFloorBasisPoints::new(600).expect("valid floor");
    let (started_sender, started_receiver) = tokio::sync::oneshot::channel();
    let mutation_task = {
        let mutation = Arc::clone(&mutation);
        tokio::spawn(async move {
            started_sender
                .send(())
                .expect("test should observe mutation start");
            mutation
                .set_weekly_quota_floor_by_label("busy-then-release", Some(committed_floor))
                .await
        })
    };
    started_receiver
        .await
        .expect("mutation task should establish its first attempt");
    tokio::task::yield_now().await;
    assert!(
        !mutation_task.is_finished(),
        "held writer should leave the setter pending for a retry"
    );
    lock.execute_batch("ROLLBACK;")
        .expect("lock should release");

    assert_eq!(
        mutation_task.await.expect("mutation task should join"),
        Ok(WeeklyQuotaFloorMutationResult::Enabled(
            AccountRoutingPolicy::new(account_id.clone(), committed_floor)
        ))
    );
    let mutation = Arc::into_inner(mutation).expect("mutation task should release store");
    mutation.close().await;

    let read_only = AsyncSqliteStateStore::open_read_only(&database_path)
        .await
        .expect("state should reopen read-only");
    assert_eq!(
        read_only.list_account_routing_policies().await,
        Ok(vec![AccountRoutingPolicy::new(account_id, committed_floor)])
    );
}

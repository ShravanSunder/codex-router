use super::*;

#[tokio::test]
async fn async_quota_history_appends_queries_and_purges_old_observations() {
    let temp_dir = TestTempDir::new("async_quota_history");
    let database_path = temp_dir.path().join("state.sqlite");
    let store = match AsyncSqliteStateStore::open(&database_path).await {
        Ok(store) => store,
        Err(error) => panic!("async state store should open and migrate: {error}"),
    };
    let account_id = account_id("acct_history");
    let before_cutoff_observation = quota_history_observation(
        account_id.clone(),
        "responses",
        18_000,
        999,
        91,
        Some(18_100),
    );
    let exact_cutoff_observation = quota_history_observation(
        account_id.clone(),
        "responses",
        18_000,
        1_000,
        90,
        Some(18_100),
    );
    let after_cutoff_observation = quota_history_observation(
        account_id.clone(),
        "responses",
        18_000,
        1_001,
        89,
        Some(18_100),
    );
    let first_observation = quota_history_observation(
        account_id.clone(),
        "responses",
        18_000,
        10_000,
        88,
        Some(28_000),
    )
    .with_effective(true)
    .with_reset_credits_available(1);
    let second_observation = quota_history_observation(
        account_id.clone(),
        "responses",
        18_000,
        10_900,
        76,
        Some(28_000),
    )
    .with_refresh_outcome(QuotaHistoryRefreshOutcome::Failure {
        error_class: QuotaRefreshErrorClass::RateLimited,
    });
    let other_window = quota_history_observation(
        account_id.clone(),
        "responses",
        604_800,
        10_900,
        50,
        Some(615_700),
    );

    for observation in [
        before_cutoff_observation,
        exact_cutoff_observation.clone(),
        after_cutoff_observation.clone(),
        first_observation.clone(),
        second_observation.clone(),
        other_window.clone(),
    ] {
        if let Err(error) =
            AsyncQuotaHistoryRepository::append_quota_history_observation(&store, &observation)
                .await
        {
            panic!("quota history observation should append: {error}");
        }
    }
    if let Err(error) = AsyncQuotaHistoryRepository::purge_quota_history_before(&store, 1_000).await
    {
        panic!("old quota history should purge: {error}");
    }

    let observations = match AsyncQuotaHistoryRepository::quota_history_observations_for_window(
        &store,
        &account_id,
        "responses",
        18_000,
        0,
        11_000,
    )
    .await
    {
        Ok(observations) => observations,
        Err(error) => panic!("quota history observations should load: {error}"),
    };

    assert_eq!(
        observations,
        vec![
            exact_cutoff_observation,
            after_cutoff_observation,
            first_observation,
            second_observation,
        ]
    );
    assert_eq!(
        AsyncQuotaHistoryRepository::quota_history_observations_for_window(
            &store,
            &account_id,
            "responses",
            604_800,
            0,
            11_000,
        )
        .await,
        Ok(vec![other_window])
    );
}

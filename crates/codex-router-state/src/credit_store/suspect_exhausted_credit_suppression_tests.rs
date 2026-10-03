use super::*;

#[tokio::test]
async fn suspect_exhausted_overlay_suppresses_positive_credit_authority_without_erasing_display() {
    let temporary_directory = CreditStoreTempDir::new();
    let database_path = temporary_directory.database_path();
    let (state, account_id) = openai_account(&database_path, "suspect_exhausted", 1).await;
    state
        .save_account_credit_usage_policy(&account_id, CreditUsagePolicy::Allow)
        .await
        .expect("explicit credit policy should persist");

    let attempt = state
        .begin_credit_refresh_attempt(&account_id, 1)
        .await
        .expect("Responses attempt should bind the active generation");
    state
        .record_responses_refresh_success(ResponsesRefreshSuccessCommit {
            attempt: &attempt,
            selector_windows: &[selector_window(&account_id, 0, 100)],
            observed_unix_seconds: 100,
            stale_after_unix_seconds: 460,
            provider_observation: &provider_observation(
                "4.25",
                CreditSpendControl::Clear,
                Some(CreditProviderLimitReason::RateLimitReached),
            ),
            history_observations: &[successful_history_observation(&account_id, 0, 100)],
            snapshot: &quota_snapshot(&account_id, 0, 100),
        })
        .await
        .expect("positive credit facts and exhausted quota should commit together");

    let before_overlay = state
        .load_account_credit_observation(&account_id)
        .await
        .expect("stored credit observation should load")
        .expect("positive credit facts should remain stored");
    assert!(before_overlay.authorizes_credit_usage(Some(1), 101));

    state
        .mark_route_band_quota_exhausted(&account_id, "responses", 110)
        .await
        .expect("SQLite should persist the suspect-exhausted overlay");
    let selector_inputs = state
        .selector_inputs_for_route_band("responses", 111)
        .await
        .expect("coherent selector inputs should load from SQLite");
    let selector_input = selector_inputs
        .iter()
        .find(|input| input.account_id() == &account_id)
        .expect("the account should remain visible to selector consumers");
    let displayed_observation = selector_input
        .credit_observation()
        .expect("the provider observation should remain displayable");

    assert_eq!(
        selector_input.credit_usage_policy(),
        CreditUsagePolicy::Allow
    );
    assert!(matches!(
        displayed_observation.provider_observation().availability(),
        CreditAvailability::Available {
            balance: Some(balance)
        } if balance.as_str() == "4.25"
    ));
    assert!(displayed_observation.authorizes_credit_usage(Some(1), 111));
    assert!(
        !selector_input.has_current_credit_authority(111),
        "the real SQLite suspect-exhausted overlay must suppress otherwise-current positive credits"
    );
}

use super::*;

#[test]
fn quota_status_selection_uses_projected_run_rate_like_runtime_selector() {
    let fast_burning_account = account("acct_fast", "fast");
    let slow_burning_account = account("acct_slow", "slow");
    let fast_burning_input = burn_down_input_from_display_windows(
        &fast_burning_account,
        &[
            display_window(
                V1_SHORT_WINDOW_SECONDS,
                50,
                NOW + V1_SHORT_WINDOW_SECONDS,
                QuotaRunRateEstimate::with_rate(QuotaRunRateConfidence::Normal, 80, 50),
            ),
            display_window(
                V1_WEEKLY_WINDOW_SECONDS,
                80,
                NOW + V1_WEEKLY_WINDOW_SECONDS,
                QuotaRunRateEstimate::unknown(),
            ),
        ],
        NOW,
    );
    let slow_burning_input = burn_down_input_from_display_windows(
        &slow_burning_account,
        &[
            display_window(
                V1_SHORT_WINDOW_SECONDS,
                50,
                NOW + V1_SHORT_WINDOW_SECONDS,
                QuotaRunRateEstimate::with_rate(QuotaRunRateConfidence::Normal, 1, 50),
            ),
            display_window(
                V1_WEEKLY_WINDOW_SECONDS,
                80,
                NOW + V1_WEEKLY_WINDOW_SECONDS,
                QuotaRunRateEstimate::unknown(),
            ),
        ],
        NOW,
    );

    let assessment = assess_route_band(BurnDownRouteBandAssessmentInput::new(
        RouteBand::Responses,
        NOW,
        vec![fast_burning_input, slow_burning_input],
    ));

    assert_eq!(
        assessment.preferred_next().map(AccountId::as_str),
        Some("acct_slow")
    );
    let Some(slow_burning_assessment) = assessment
        .accounts()
        .iter()
        .find(|account| account.account_id().as_str() == "acct_slow")
    else {
        panic!("slow-burning account should be assessed");
    };
    assert!(matches!(
        slow_burning_assessment.routing_reason(),
        RoutingReason::PreferredProjectedBurn | RoutingReason::PreferredSafestQuota
    ));
}

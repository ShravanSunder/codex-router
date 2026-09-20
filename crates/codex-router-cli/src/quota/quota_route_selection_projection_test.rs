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

#[test]
fn quota_status_selection_exposes_initial_admission_for_fresh_idle_quota() {
    let near_reset_account = account("acct_near_reset", "near-reset");
    let reserve_account = account("acct_reserve", "reserve");
    let near_reset_input = burn_down_input_from_display_windows(
        &near_reset_account,
        &[
            display_window(
                V1_SHORT_WINDOW_SECONDS,
                100,
                NOW + 4 * 3_600,
                QuotaRunRateEstimate::unknown(),
            ),
            display_window(
                V1_WEEKLY_WINDOW_SECONDS,
                20,
                NOW + 24 * 3_600,
                QuotaRunRateEstimate::unknown(),
            ),
        ],
        NOW,
    );
    let reserve_input = burn_down_input_from_display_windows(
        &reserve_account,
        &[
            display_window(
                V1_SHORT_WINDOW_SECONDS,
                100,
                NOW + 4 * 3_600,
                QuotaRunRateEstimate::with_rate(QuotaRunRateConfidence::Normal, 20, 100),
            ),
            display_window(
                V1_WEEKLY_WINDOW_SECONDS,
                80,
                NOW + 5 * 86_400,
                QuotaRunRateEstimate::with_rate(QuotaRunRateConfidence::Normal, 20, 80),
            ),
        ],
        NOW,
    );

    let assessment = assess_route_band(BurnDownRouteBandAssessmentInput::new(
        RouteBand::Responses,
        NOW,
        vec![near_reset_input, reserve_input],
    ));
    let selected = assessment
        .accounts()
        .iter()
        .find(|account| account.preferred_next())
        .unwrap_or_else(|| panic!("quota status projection should select an account"));

    assert_eq!(selected.account_id().as_str(), "acct_near_reset");
    assert_eq!(
        selected.routing_reason(),
        RoutingReason::PreferredNearResetInitialAdmission
    );
    assert_eq!(
        selected.weekly_burn_rate_confidence(),
        QuotaRunRateConfidence::Unknown
    );
    assert_eq!(selected.weekly_projected_exhaustion_unix_seconds(), None);
}

//! OpenAI account authentication boundaries for codex-router.

pub mod live_quota;
pub mod oauth;
pub mod quota_client;
pub mod refresh_worker;
pub mod router_credentials;

/// Returns this crate's package name.
#[must_use]
pub const fn package_name() -> &'static str {
    "codex-router-auth"
}

#[cfg(test)]
mod tests {
    use super::package_name;
    use crate::live_quota::UsageWindow;
    use crate::live_quota::UsageWindowBurnDown;
    use crate::live_quota::WindowPair;
    use crate::live_quota::quota_pair_bottleneck_window;
    use crate::live_quota::quota_pair_remaining_headroom;
    use crate::live_quota::quota_window_burn_down;
    use crate::oauth::OAuthRefreshClassification;
    use crate::oauth::OAuthTokenStatus;
    use crate::oauth::TokenClock;
    use crate::oauth::classify_refresh_response;
    use crate::quota_client::AuthenticatedQuotaClient;
    use crate::quota_client::AuthenticatedQuotaError;
    use crate::quota_client::QuotaFetchRequest;
    use crate::quota_client::QuotaFetchResponse;
    use crate::refresh_worker::AccountRefreshInput;
    use crate::refresh_worker::RefreshWorkDecision;
    use crate::refresh_worker::RefreshWorker;

    #[test]
    fn reports_package_name() {
        assert_eq!(package_name(), "codex-router-auth");
    }

    #[test]
    fn token_status_classifies_refresh_window_and_expiry() {
        let clock = TokenClock::new(1_000);

        assert_eq!(
            clock.classify_token(1_400, 120),
            OAuthTokenStatus::Valid {
                refresh_after_unix_seconds: 1_280
            }
        );
        assert_eq!(
            clock.classify_token(1_060, 120),
            OAuthTokenStatus::RefreshNeeded
        );
        assert_eq!(clock.classify_token(999, 120), OAuthTokenStatus::Expired);
    }

    #[test]
    fn refresh_response_classification_is_openai_oauth_specific() {
        assert_eq!(
            classify_refresh_response(200, None),
            OAuthRefreshClassification::Succeeded
        );
        assert_eq!(
            classify_refresh_response(400, Some("invalid_grant")),
            OAuthRefreshClassification::RefreshTokenRejected
        );
        assert_eq!(
            classify_refresh_response(429, None),
            OAuthRefreshClassification::RateLimited
        );
        assert_eq!(
            classify_refresh_response(503, None),
            OAuthRefreshClassification::TransientProviderFailure
        );
        assert_eq!(
            classify_refresh_response(418, Some("teapot")),
            OAuthRefreshClassification::UnexpectedProviderResponse { status: 418 }
        );
    }

    #[test]
    fn refresh_worker_selects_only_accounts_that_need_background_refresh() {
        let worker = RefreshWorker::new(TokenClock::new(1_000), 120);
        let decisions = worker.plan_refreshes(&[
            AccountRefreshInput::new("acct_valid", 1_400),
            AccountRefreshInput::new("acct_refresh", 1_050),
            AccountRefreshInput::new("acct_expired", 900),
        ]);

        assert_eq!(
            decisions,
            vec![
                RefreshWorkDecision::Skip {
                    account_label: "acct_valid".to_owned(),
                    token_status: OAuthTokenStatus::Valid {
                        refresh_after_unix_seconds: 1_280
                    }
                },
                RefreshWorkDecision::Refresh {
                    account_label: "acct_refresh".to_owned(),
                    token_status: OAuthTokenStatus::RefreshNeeded
                },
                RefreshWorkDecision::Refresh {
                    account_label: "acct_expired".to_owned(),
                    token_status: OAuthTokenStatus::Expired
                }
            ]
        );
    }

    #[test]
    fn authenticated_quota_client_contract_maps_mock_quota_response() {
        let client = FakeAuthenticatedQuotaClient;
        let response = match client.fetch_quota(QuotaFetchRequest::new("acct_primary", "responses"))
        {
            Ok(response) => response,
            Err(error) => panic!("quota fetch should succeed: {error}"),
        };

        assert_eq!(response.remaining_headroom(), 77);
        assert_eq!(response.route_name(), "responses");
    }

    #[test]
    fn live_quota_headroom_uses_daily_weekly_bottleneck() {
        let quota = WindowPair {
            primary_window: Some(UsageWindow {
                used_percent: Some(25),
                reset_at: Some(2_000),
                limit_window_seconds: Some(18_000),
            }),
            secondary_window: Some(UsageWindow {
                used_percent: Some(80),
                reset_at: Some(9_000),
                limit_window_seconds: Some(604_800),
            }),
        };

        assert_eq!(quota_pair_remaining_headroom(Some(&quota)), Some(20));
        assert_eq!(
            quota_pair_bottleneck_window(Some(&quota)).and_then(|window| window.reset_at),
            Some(9_000)
        );
    }

    #[test]
    fn live_quota_window_burn_down_reports_pace_and_projected_runout() {
        let window = UsageWindow {
            used_percent: Some(25),
            reset_at: Some(1_800),
            limit_window_seconds: Some(1_000),
        };

        assert_eq!(
            quota_window_burn_down(Some(&window), 1_300),
            Some(UsageWindowBurnDown {
                expected_used_percent: 50,
                pace_delta_percent: -25,
                runout_unix_seconds: Some(2_800),
                runout_before_reset: false,
            })
        );
    }

    struct FakeAuthenticatedQuotaClient;

    impl AuthenticatedQuotaClient for FakeAuthenticatedQuotaClient {
        fn fetch_quota(
            &self,
            request: QuotaFetchRequest,
        ) -> Result<QuotaFetchResponse, AuthenticatedQuotaError> {
            assert_eq!(request.account_label(), "acct_primary");
            assert_eq!(request.route_name(), "responses");
            Ok(QuotaFetchResponse::new(request.route_name(), 77))
        }
    }
}

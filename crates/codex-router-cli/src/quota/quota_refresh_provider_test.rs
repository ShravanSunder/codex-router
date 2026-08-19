use super::*;

#[test]
fn quota_refresh_selector_window_stale_after_uses_plan_freshness_ceiling() {
    assert_eq!(
        stale_after_unix_seconds(1_000),
        1_360,
        "selector-window last-known-good freshness must outlive the default refresh cadence"
    );
}

#[test]
fn quota_response_ignores_independently_metered_additional_window() {
    let usage: UsageResponse = serde_json::from_str(
        r#"{
            "rate_limit": {
                "primary_window": null,
                "secondary_window": {
                    "used_percent": 72,
                    "reset_at": 9000,
                    "limit_window_seconds": 604800
                }
            },
            "additional_rate_limits": [{
                "limit_name": "codex_other",
                "metered_feature": "codex_other",
                "rate_limit": {
                    "primary_window": {
                        "used_percent": 28,
                        "reset_at": 2000,
                        "limit_window_seconds": 18000
                    },
                    "secondary_window": null
                }
            }]
        }"#,
    )
    .expect("latest Codex usage payload should deserialize");

    let response = quota_response_for_route_band(&usage, "responses")
        .expect("top-level weekly quota should remain usable");

    assert_eq!(
        response
            .windows
            .iter()
            .map(|window| window.limit_window_seconds)
            .collect::<Vec<_>>(),
        vec![V1_WEEKLY_WINDOW_SECONDS]
    );
    assert_eq!(
        response
            .windows
            .iter()
            .find(|window| window.limit_window_seconds == V1_WEEKLY_WINDOW_SECONDS)
            .map(|window| window.effective),
        Some(true)
    );
}

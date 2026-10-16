//! Opt-in real quota screen capture. No production account or native Host is used.

#![cfg(unix)]

#[path = "quota_storyboard_capture_test/mod.rs"]
mod capture_support;
#[path = "quota_reset_pty_test/isolated_fixture_test.rs"]
#[expect(
    dead_code,
    reason = "Read-only shared fixture also exposes cached-credit seeding, outside this journey"
)]
mod isolated_fixture_test;
#[path = "quota_reset_pty_test/loopback_provider_test.rs"]
#[expect(
    dead_code,
    reason = "Read-only shared provider also exposes fault/held-response scenarios, outside this journey"
)]
mod loopback_provider_test;

use isolated_fixture_test::TestResult;

#[tokio::test(flavor = "current_thread")]
#[ignore = "requires explicitly nominated isolated VHS/ttyd tools and output directory"]
async fn compiled_quota_storyboard_drives_real_ui_and_exports_screens() -> TestResult<()> {
    capture_support::run_storyboard(120, 36, false).await?;
    capture_support::run_storyboard(76, 30, false).await
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "requires explicitly nominated isolated VHS/ttyd tools and output directory"]
async fn compiled_quota_storyboard_rejects_an_impossible_screen() -> TestResult<()> {
    capture_support::run_storyboard(120, 36, true).await
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "requires explicitly nominated isolated VHS/ttyd tools and output directory"]
async fn compiled_quota_storyboard_cleans_an_interrupted_live_capture() -> TestResult<()> {
    capture_support::run_interrupted_capture_cleanup().await
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "requires nominated private capture tools and fresh owned output directory"]
async fn compiled_provider_filter_storyboards_export_both_real_tuis() -> TestResult<()> {
    for (columns, rows) in [(120, 36), (76, 30), (48, 24)] {
        for journey in [
            capture_support::ProviderFilterJourney::QuotaMixed,
            capture_support::ProviderFilterJourney::Sessions,
            capture_support::ProviderFilterJourney::QuotaEmpty,
        ] {
            capture_support::run_provider_filter_storyboard(columns, rows, journey).await?;
        }
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "requires admitted private current-source tools, target and fresh output directory"]
async fn compiled_compact_current_storyboard_exports_both_real_tuis() -> TestResult<()> {
    for (columns, rows) in [(120, 36), (76, 30), (48, 24)] {
        for journey in [
            capture_support::ProviderFilterJourney::CompactSessions,
            capture_support::ProviderFilterJourney::CompactQuota,
        ] {
            capture_support::run_provider_filter_storyboard(columns, rows, journey).await?;
        }
    }
    Ok(())
}

use codex_router_test_support::installed_codex::run_all_weekly_exhausted_terminal;
use codex_router_test_support::installed_codex::run_capacity_retry_limit_terminal;
use codex_router_test_support::installed_codex::run_model_capacity_reconnect;
use codex_router_test_support::installed_codex::run_three_account_short_quota_reconnect;

#[test]
#[ignore = "installed Codex integration; run with cargo test -p codex-router-test-support --test codex_retry -- --ignored --test-threads=1"]
fn three_account_5h_reset_reconnects_and_completes() -> Result<(), String> {
    run_three_account_short_quota_reconnect()
}

#[test]
#[ignore = "installed Codex integration; run with cargo test -p codex-router-test-support --test codex_retry -- --ignored --test-threads=1"]
fn all_accounts_weekly_exhausted_stops() -> Result<(), String> {
    run_all_weekly_exhausted_terminal()
}

#[test]
#[ignore = "installed Codex integration; run with cargo test -p codex-router-test-support --test codex_retry -- --ignored --test-threads=1"]
fn model_capacity_reconnects_and_completes() -> Result<(), String> {
    run_model_capacity_reconnect()
}

#[test]
#[ignore = "installed Codex integration; run with cargo test -p codex-router-test-support --test codex_retry -- --ignored --test-threads=1"]
fn capacity_retry_limit_forwards_terminal_error() -> Result<(), String> {
    run_capacity_retry_limit_terminal()
}

use std::ffi::OsString;
use std::path::Path;

use super::SEMANTIC_WAIT;
use super::ensure;
use super::isolated_fixture_test::QuotaResetFixture;
use super::isolated_fixture_test::TestResult;
use super::loopback_provider_test::HeldLoopbackProvider;
use super::stage;
use super::terminal_interaction_test::TerminalDriver;
use codex_router_core::ids::AccountId;
use codex_router_state::sqlite::AsyncSqliteStateStore;

#[tokio::test(flavor = "current_thread")]
async fn compiled_credit_refresh_reports_focused_responses_failure_despite_partial_success()
-> TestResult<()> {
    let fixture = QuotaResetFixture::create().await?;
    let focused_account_id = AccountId::new("acct_pty_alpha")?;
    let peer_account_id = AccountId::new("acct_pty_beta")?;
    fixture
        .seed_cached_credit_observation(&focused_account_id, 7)
        .await?;
    let before_observation = load_credit_observation(&fixture, &focused_account_id).await?;
    let mut provider =
        HeldLoopbackProvider::bind_failing_usage_for_account("routing-pty-alpha", 2, 429)?;
    let mut terminal = spawn_fixture_terminal(&fixture, provider.address())?;

    if let Err(error) = terminal.wait_for_text("ctrl-r account options", SEMANTIC_WAIT) {
        let diagnostics = terminal.safe_semantic_diagnostics(0);
        let startup_request = provider
            .wait_for_request_count(1, std::time::Duration::ZERO)
            .ok()
            .and_then(|requests| requests.first())
            .map(|request| format!("{} {}", request.method, request.path));
        provider.release_get_responses()?;
        return Err(std::io::Error::other(format!(
            "partial-refresh initial browse: {error}; {diagnostics}; startup_request={startup_request:?}"
        ))
        .into());
    }
    let options_start = terminal.transcript_len();
    terminal.send(&[0x12])?;
    stage(
        terminal.wait_for_text_after("Reset credits", options_start, SEMANTIC_WAIT),
        "partial-refresh Reset tab",
    )?;
    stage(
        provider.wait_for_request_count(2, SEMANTIC_WAIT),
        "partial-refresh inspection requests",
    )?;
    let credits_tab_start = terminal.transcript_len();
    terminal.send(b"\t")?;
    stage(
        terminal.wait_for_text_after("[ Credits ]", credits_tab_start, SEMANTIC_WAIT),
        "partial-refresh tab cancellation acknowledgement",
    )?;
    provider.release_get_responses()?;
    let refresh_start = terminal.transcript_len();
    terminal.send(b"r")?;
    stage(
        provider.wait_for_request_count(9, SEMANTIC_WAIT),
        "partial-refresh whole-pool provider requests",
    )?;
    if let Err(error) = terminal.wait_for_text_after(
        "Credit refresh failed; cached observation retained.",
        refresh_start,
        SEMANTIC_WAIT,
    ) {
        if terminal
            .wait_for_text("Credit balance refreshed.", std::time::Duration::ZERO)
            .is_ok()
        {
            return Err(std::io::Error::other(
                "focused Responses failure was reported as a successful credit refresh",
            )
            .into());
        }
        let diagnostics = terminal.safe_semantic_diagnostics(0);
        return Err(std::io::Error::other(format!(
            "focused Responses failure feedback despite Models and peer success: {error}; {diagnostics}"
        ))
        .into());
    }
    let browse_frame_start = terminal.transcript_len();
    terminal.send(b"\x1b")?;
    stage(
        terminal.wait_for_text_after("ctrl-r account options", browse_frame_start, SEMANTIC_WAIT),
        "partial-refresh account-options close",
    )?;
    terminal.send(b"q")?;
    let transcript = stage(terminal.finish(SEMANTIC_WAIT), "partial-refresh child exit")?;
    let requests = stage(provider.finish(), "partial-refresh provider shutdown")?;

    ensure(
        requests.len() == 9 && requests.iter().all(|request| request.method == "GET"),
        "partial credit refresh must issue only the expected loopback GETs",
    )?;
    ensure(
        requests
            .iter()
            .filter(|request| {
                request.response_status == 429
                    && request.path.ends_with("/usage")
                    && request.routing_account.as_deref() == Some("routing-pty-alpha")
            })
            .count()
            == 1,
        "focused Responses provider failure should be the only synthetic 429",
    )?;
    ensure(
        requests.iter().any(|request| {
            request.path.ends_with("/usage")
                && request.routing_account.as_deref() == Some("routing-pty-beta")
        }),
        "peer quota success should accompany the focused Responses failure",
    )?;
    ensure(
        !String::from_utf8_lossy(&transcript).contains("Credit balance refreshed."),
        "partial refresh must not report a new focused balance",
    )?;

    let state = AsyncSqliteStateStore::open_read_only(&fixture.root().join("state.sqlite")).await?;
    let after_observation = state
        .load_account_credit_observation(&focused_account_id)
        .await?
        .ok_or_else(|| std::io::Error::other("focused cached credit observation disappeared"))?;
    ensure(
        before_observation.observed_unix_seconds() == after_observation.observed_unix_seconds(),
        "failed Responses refresh must preserve the focused cached observation age",
    )?;
    ensure(
        before_observation.provider_observation() == after_observation.provider_observation(),
        "failed Responses refresh must preserve the focused cached credit facts",
    )?;
    ensure(
        state
            .load_quota_snapshot_for_route_band(&focused_account_id, "models")
            .await?
            .is_some(),
        "focused Models refresh should still commit while Responses fails",
    )?;
    ensure(
        state
            .load_account_credit_observation(&peer_account_id)
            .await?
            .is_some(),
        "peer Responses observation should commit during partial success",
    )?;
    state.close().await?;
    fixture.assert_secrets_unchanged()?;
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn compiled_pending_refresh_tab_can_return_to_browse_and_exit_before_provider_release()
-> TestResult<()> {
    let fixture = QuotaResetFixture::create().await?;
    let mut provider = HeldLoopbackProvider::bind_releasing_first_gets(2)?;
    let mut terminal = spawn_fixture_terminal(&fixture, provider.address())?;

    stage(
        terminal.wait_for_text("ctrl-r account options", SEMANTIC_WAIT),
        "pending-refresh initial browse",
    )?;
    let inspection_start = terminal.transcript_len();
    terminal.send(&[0x12])?;
    stage(
        terminal.wait_for_text_after("Reset credits", inspection_start, SEMANTIC_WAIT),
        "pending-refresh Resets tab",
    )?;
    stage(
        provider.wait_for_request_count(2, SEMANTIC_WAIT),
        "initial reset inspection requests",
    )?;
    let credits_tab_start = terminal.transcript_len();
    terminal.send(b"\t")?;
    stage(
        terminal.wait_for_text_after("[ Credits ]", credits_tab_start, SEMANTIC_WAIT),
        "initial switch to Credits",
    )?;

    let refresh_start = terminal.transcript_len();
    terminal.send(b"r")?;
    stage(
        provider.wait_for_request_count(3, SEMANTIC_WAIT),
        "whole-pool refresh starts and blocks on loopback response",
    )?;
    stage(
        terminal.wait_for_text_after("Refreshing credit balance", refresh_start, SEMANTIC_WAIT),
        "refresh progress while provider is held",
    )?;
    let pending_switch_start = terminal.transcript_len();
    terminal.send(b"\t")?;
    stage(
        terminal.wait_for_text_after(
            "tab change pending  esc back  q/ctrl-c exit",
            pending_switch_start,
            SEMANTIC_WAIT,
        ),
        "pending navigation renders its available cancellation and exit actions",
    )?;

    let browse_start = terminal.transcript_len();
    terminal.send(b"\x1b")?;
    if let Err(error) =
        terminal.wait_for_text_after("ctrl-r account options", browse_start, SEMANTIC_WAIT)
    {
        let diagnostics = terminal.safe_semantic_diagnostics(browse_start);
        provider.release_get_responses()?;
        return Err(std::io::Error::other(format!(
            "Escape should close a pending tab transition without awaiting the whole-pool refresh: {error}; {diagnostics}"
        ))
        .into());
    }

    terminal.send(b"q")?;
    let transcript = stage(
        terminal.finish(SEMANTIC_WAIT),
        "exit while the loopback refresh is still held",
    )?;
    provider.release_get_responses()?;
    let requests = provider.finish()?;
    ensure(
        requests.len() == 3 && requests.iter().all(|request| request.method == "GET"),
        "cancelled pending navigation should exit before the remaining refresh requests and never POST",
    )?;
    ensure(
        !String::from_utf8_lossy(&transcript).contains("chatgpt.com")
            && !String::from_utf8_lossy(&transcript).contains("api.openai.com"),
        "pending-refresh exit transcript must not mention a production origin",
    )?;
    fixture.assert_secrets_unchanged()?;
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn compiled_disabled_account_refresh_reports_unavailable_without_provider_io()
-> TestResult<()> {
    let fixture = QuotaResetFixture::create().await?;
    let peer_account_id = AccountId::new("acct_pty_alpha")?;
    let focused_account_id = AccountId::new("acct_pty_beta")?;
    fixture
        .seed_cached_credit_observation(&focused_account_id, 11)
        .await?;
    let before_observation = load_credit_observation(&fixture, &focused_account_id).await?;
    let state = AsyncSqliteStateStore::open(&fixture.root().join("state.sqlite")).await?;
    ensure(
        state
            .disable_account_if_credential_generation_current(&focused_account_id, 11)
            .await?,
        "focused account should be disabled in the isolated fixture",
    )?;
    state.close().await?;
    let state = AsyncSqliteStateStore::open_read_only(&fixture.root().join("state.sqlite")).await?;
    let accounts = state.list_accounts().await?;
    ensure(
        accounts.iter().any(|account| {
            account.account_id() == &peer_account_id
                && account.status() == codex_router_state::account::AccountStatus::Enabled
                && account.active_credential_generation() == Some(7)
        }),
        "the unrelated enabled peer should remain available",
    )?;
    state.close().await?;

    let provider = HeldLoopbackProvider::bind()?;
    let mut terminal = spawn_fixture_terminal(&fixture, provider.address())?;
    stage(
        terminal.wait_for_text("ctrl-r account options", SEMANTIC_WAIT),
        "ineligible-refresh initial browse",
    )?;
    let disabled_options_start = terminal.transcript_len();
    terminal.send(b"\x1b[B")?;
    terminal.send(&[0x12])?;
    stage(
        terminal.wait_for_text_after(
            "Resets are unavailable while this account is disabled.",
            disabled_options_start,
            SEMANTIC_WAIT,
        ),
        "disabled focused account options",
    )?;
    let credits_tab_start = terminal.transcript_len();
    terminal.send(b"\t")?;
    stage(
        terminal.wait_for_text_after("[ Credits ]", credits_tab_start, SEMANTIC_WAIT),
        "disabled-account tab switch acknowledgement",
    )?;
    let refresh_start = terminal.transcript_len();
    terminal.send(b"r")?;
    stage(
        terminal.wait_for_text_after(
            "Credit refresh unavailable: account disabled.",
            refresh_start,
            SEMANTIC_WAIT,
        ),
        "disabled account refresh availability feedback",
    )?;
    let close_options_start = terminal.transcript_len();
    terminal.send(b"\x1b")?;
    stage(
        terminal.wait_for_text_after("ctrl-r account options", close_options_start, SEMANTIC_WAIT),
        "ineligible-refresh account-options close",
    )?;
    terminal.send(b"q")?;
    let transcript = stage(
        terminal.finish(SEMANTIC_WAIT),
        "ineligible-refresh child exit",
    )?;
    let requests = stage(provider.finish(), "ineligible-refresh provider shutdown")?;

    ensure(
        requests.is_empty(),
        "an unavailable focused account must not trigger a whole-pool refresh",
    )?;
    ensure(
        !String::from_utf8_lossy(&transcript)
            .contains("Credit refresh failed; cached observation retained."),
        "an unavailable refresh must not be reported as a failed provider refresh",
    )?;

    let state = AsyncSqliteStateStore::open_read_only(&fixture.root().join("state.sqlite")).await?;
    let after_observation = state
        .load_account_credit_observation(&focused_account_id)
        .await?
        .ok_or_else(|| std::io::Error::other("disabled cached credit observation disappeared"))?;
    ensure(
        before_observation.observed_unix_seconds() == after_observation.observed_unix_seconds(),
        "an unavailable refresh must preserve the disabled account's observation age",
    )?;
    ensure(
        before_observation.provider_observation() == after_observation.provider_observation(),
        "an unavailable refresh must preserve the disabled account's credit facts",
    )?;
    ensure(
        state
            .load_account_credit_observation(&peer_account_id)
            .await?
            .is_none(),
        "an unavailable focused refresh must not refresh an unrelated enabled peer",
    )?;
    state.close().await?;
    fixture.assert_secrets_unchanged()?;
    Ok(())
}

async fn load_credit_observation(
    fixture: &QuotaResetFixture,
    account_id: &AccountId,
) -> TestResult<codex_router_state::credit_store::CreditUsageObservation> {
    let state = AsyncSqliteStateStore::open_read_only(&fixture.root().join("state.sqlite")).await?;
    let observation = state
        .load_account_credit_observation(account_id)
        .await?
        .ok_or_else(|| std::io::Error::other("seeded credit observation should exist"))?;
    state.close().await?;
    Ok(observation)
}

fn spawn_fixture_terminal(
    fixture: &QuotaResetFixture,
    provider_address: std::net::SocketAddr,
) -> TestResult<TerminalDriver> {
    let arguments = [
        OsString::from("--router-root"),
        fixture.root().as_os_str().to_owned(),
        OsString::from("--fixture-capability"),
        OsString::from(fixture.capability()),
        OsString::from("--provider-listener"),
        OsString::from(provider_address.to_string()),
    ];
    TerminalDriver::spawn(
        Path::new(env!("CARGO_BIN_EXE_codex-router-quota-reset-test-harness")),
        arguments,
        Path::new(env!("CARGO_MANIFEST_DIR")),
    )
}

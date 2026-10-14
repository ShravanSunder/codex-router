//! Default-CI real process proof; no browser tools or live providers are needed.
use super::isolated_fixture_test::ProviderFilterFixtureScenario;
use super::*;

#[tokio::test(flavor = "current_thread")]
async fn compiled_quota_filters_real_stored_providers_and_disabled_intersections() -> TestResult<()>
{
    for (scenario, count, result) in [
        (
            ProviderFilterFixtureScenario::MixedAccounts,
            "1/4 accounts",
            "Claude Gamma",
        ),
        (
            ProviderFilterFixtureScenario::ClaudeAccountsDisabled,
            "0/4 accounts",
            "No accounts match these filters",
        ),
    ] {
        let fixture = QuotaResetFixture::create_provider_filters(scenario).await?;
        let provider = HeldLoopbackProvider::bind()?;
        let arguments = [
            OsString::from("--router-root"),
            fixture.root().as_os_str().to_owned(),
            OsString::from("--fixture-capability"),
            OsString::from(fixture.capability()),
            OsString::from("--provider-listener"),
            OsString::from(provider.address().to_string()),
        ];
        let mut terminal = TerminalDriver::spawn(
            Path::new(env!("CARGO_BIN_EXE_codex-router-quota-reset-test-harness")),
            arguments,
            Path::new(env!("CARGO_MANIFEST_DIR")),
        )?;
        terminal.resize(36, 120)?;
        terminal.wait_for_text("4/4 accounts", SEMANTIC_WAIT)?;
        let chooser_start = terminal.transcript_len();
        terminal.send(&[0x06])?;
        terminal.wait_for_text_after("Account provider", chooser_start, SEMANTIC_WAIT)?;
        terminal.send(b"\x1b[B\x1b[B")?;
        let apply_start = terminal.transcript_len();
        terminal.send(b"\r")?;
        terminal.wait_for_text_after("Provider: Claude", apply_start, SEMANTIC_WAIT)?;
        terminal.wait_for_text_after("2/4 accounts", apply_start, SEMANTIC_WAIT)?;
        let toggle_start = terminal.transcript_len();
        terminal.send(&[0x06])?;
        terminal.wait_for_text_after("Account provider", toggle_start, SEMANTIC_WAIT)?;
        terminal.send(b"\t ")?;
        let intersection_start = terminal.transcript_len();
        terminal.send(b"\r")?;
        terminal.wait_for_text_after("Disabled: Hidden", intersection_start, SEMANTIC_WAIT)?;
        terminal.wait_for_text_after(count, intersection_start, SEMANTIC_WAIT)?;
        terminal.wait_for_text_after(result, intersection_start, SEMANTIC_WAIT)?;
        terminal.send(b"q")?;
        let transcript = terminal.finish(SEMANTIC_WAIT)?;
        ensure(
            provider.finish()?.is_empty(),
            "filter-only PTY made provider requests",
        )?;
        fixture.assert_read_only()?;
        assert_no_printable_output_after_terminal_restoration(&transcript)?;
        let text = String::from_utf8_lossy(&transcript);
        assert_forbidden_terminal_canaries_absent(&text)?;
        ensure(!text.contains("panicked at"), "filter PTY child panicked")?;
    }
    Ok(())
}

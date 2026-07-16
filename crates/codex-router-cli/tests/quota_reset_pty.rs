mod quota_reset_pty {
    mod fixture;
    mod loopback_server;
    mod terminal_driver;

    use std::collections::BTreeSet;
    use std::ffi::OsString;
    use std::net::TcpStream;
    use std::path::Path;
    use std::process::Command;
    use std::time::Duration;

    use fixture::ACCESS_TOKEN_CANARY;
    use fixture::QuotaResetFixture;
    use fixture::TestResult;
    use fixture::ensure;
    use loopback_server::HeldLoopbackProvider;
    use terminal_driver::TerminalDriver;

    const SEMANTIC_WAIT: Duration = Duration::from_secs(8);

    #[tokio::test(flavor = "current_thread")]
    async fn compiled_quota_tui_inspects_loopback_and_cancels_with_zero_posts() -> TestResult<()> {
        let fixture = QuotaResetFixture::create().await?;
        let mut provider = HeldLoopbackProvider::bind()?;
        let arguments = [
            OsString::from("--router-root"),
            fixture.root().as_os_str().to_owned(),
            OsString::from("--provider-listener"),
            OsString::from(provider.address().to_string()),
        ];
        let mut terminal = TerminalDriver::spawn(
            Path::new(env!("CARGO_BIN_EXE_codex-router-quota-reset-test-harness")),
            arguments,
            Path::new(env!("CARGO_MANIFEST_DIR")),
        )?;

        terminal.wait_for_text("ctrl-r inspect reset credits", SEMANTIC_WAIT)?;
        terminal.send(b"\x1b[B")?;
        terminal.send(&[0x12])?;
        terminal.wait_for_text("inspect usage      loading", SEMANTIC_WAIT)?;
        terminal.wait_for_text("inspect credits    loading", SEMANTIC_WAIT)?;

        let requests = provider.wait_for_request_count(2, SEMANTIC_WAIT)?;
        ensure(
            requests
                .iter()
                .map(|request| request.method.as_str())
                .collect::<Vec<_>>()
                == vec!["GET", "GET"],
            "inspection must issue exactly two GET requests",
        )?;
        ensure(
            requests
                .iter()
                .map(|request| request.path.as_str())
                .collect::<BTreeSet<_>>()
                == BTreeSet::from(["/api/codex/rate-limit-reset-credits", "/api/codex/usage"]),
            "inspection paths did not match usage and reset-credit GETs",
        )?;
        let routing_accounts = requests
            .iter()
            .filter_map(|request| request.routing_account.as_deref())
            .collect::<BTreeSet<_>>();
        ensure(
            routing_accounts.len() == 1,
            "inspection GETs did not target one routing account",
        )?;

        let resize_start = terminal.transcript_len();
        terminal.resize(48, 170)?;
        terminal.wait_for_text_after("Reset credit", resize_start, SEMANTIC_WAIT)?;
        let cancel_start = terminal.transcript_len();
        terminal.send(&[0x12])?;
        terminal.wait_for_text_after(
            "ctrl-r inspect reset credits",
            cancel_start,
            SEMANTIC_WAIT,
        )?;
        terminal.send(b"q")?;
        let transcript = terminal.finish(SEMANTIC_WAIT)?;
        let request_records = provider.finish()?;

        ensure(
            request_records.len() == 2,
            "provider ledger contained an unexpected request count",
        )?;
        ensure(
            request_records
                .iter()
                .all(|request| request.method == "GET"),
            "provider ledger observed a consume POST",
        )?;
        fixture.assert_read_only()?;
        let transcript = String::from_utf8_lossy(&transcript);
        ensure(
            !transcript.contains(ACCESS_TOKEN_CANARY),
            "terminal transcript exposed the access-token canary",
        )?;
        ensure(
            !transcript.contains("Quota reset moved"),
            "harness used legacy migration guidance instead of integrated quota",
        )?;
        ensure(
            !transcript.contains("test harness composition is not configured"),
            "harness fail-closed stub remained reachable",
        )?;
        ensure(
            !transcript.contains("panicked at"),
            "terminal transcript contained a child panic",
        )?;
        ensure(
            transcript.contains("\u{1b}[?25h") || transcript.contains("\u{1b}[?1049l"),
            "terminal restoration sequence was not observed",
        )?;
        Ok(())
    }

    #[test]
    fn all_feature_installed_binary_rejects_harness_options_without_creating_state()
    -> TestResult<()> {
        let absent_root = std::env::temp_dir().join(format!(
            "codex-router-installed-harness-rejection-{}",
            std::process::id()
        ));
        ensure(
            !absent_root.exists(),
            "installed-binary rejection sentinel already exists",
        )?;
        let output = Command::new(env!("CARGO_BIN_EXE_codex-router"))
            .env_clear()
            .args([
                "quota",
                "--provider-listener",
                "127.0.0.1:9",
                "--router-root",
            ])
            .arg(&absent_root)
            .output()?;

        ensure(
            output.status.code() == Some(2),
            "installed binary did not return parser exit 2",
        )?;
        ensure(
            output.stdout.is_empty(),
            "installed binary wrote stdout for a rejected harness option",
        )?;
        ensure(
            String::from_utf8_lossy(&output.stderr).contains("unknown option"),
            "installed binary did not reject the harness option as unknown",
        )?;
        ensure(
            !absent_root.exists(),
            "installed binary touched state before rejecting harness option",
        )?;
        Ok(())
    }

    #[tokio::test(flavor = "current_thread")]
    async fn fixture_and_listener_guards_cleanup_on_early_return() -> TestResult<()> {
        let fixture = QuotaResetFixture::create().await?;
        let fixture_root = fixture.root().to_path_buf();
        let provider = HeldLoopbackProvider::bind()?;
        let address = provider.address();

        drop(provider);
        drop(fixture);

        ensure(
            !fixture_root.exists(),
            "fixture guard did not remove its root",
        )?;
        ensure(
            TcpStream::connect(address).is_err(),
            "loopback listener remained reachable after guard drop",
        )?;
        Ok(())
    }
}

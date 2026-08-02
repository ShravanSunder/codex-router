mod quota_reset_pty_test {
    mod isolated_fixture_test;
    mod loopback_provider_test;
    mod terminal_interaction_test;

    use std::collections::BTreeSet;
    use std::ffi::OsString;
    use std::fs;
    use std::net::TcpListener;
    use std::net::TcpStream;
    use std::path::Path;
    use std::process::Command;
    use std::time::Duration;
    use std::time::Instant;

    use isolated_fixture_test::FORBIDDEN_TERMINAL_CANARIES;
    use isolated_fixture_test::QuotaResetFixture;
    use isolated_fixture_test::TestResult;
    use isolated_fixture_test::ensure;
    use loopback_provider_test::HeldLoopbackProvider;
    use terminal_interaction_test::TerminalDriver;

    const SEMANTIC_WAIT: Duration = Duration::from_secs(8);

    #[tokio::test(flavor = "current_thread")]
    async fn compiled_quota_tui_inspects_loopback_and_cancels_with_zero_posts() -> TestResult<()> {
        let fixture = QuotaResetFixture::create().await?;
        let mut provider = HeldLoopbackProvider::bind()?;
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

        stage(
            terminal.wait_for_text("ctrl-r reset credits", SEMANTIC_WAIT),
            "initial browse",
        )?;
        terminal.send(b"\x1b[B")?;
        terminal.send(&[0x12])?;
        terminal.wait_for_text("Weekly usage", SEMANTIC_WAIT)?;
        terminal.wait_for_text("Reset credits", SEMANTIC_WAIT)?;

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
        terminal.wait_for_text_after("ctrl-r reset credits", cancel_start, SEMANTIC_WAIT)?;
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
        assert_forbidden_terminal_canaries_absent(&transcript)?;
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
            transcript.contains("\u{1b}[?1049h")
                && transcript.contains("\u{1b}[?1003h")
                && transcript.contains("\u{1b}[?1006h"),
            "fullscreen quota TUI did not enable alternate-screen mouse capture",
        )?;
        ensure(
            transcript.contains("\u{1b}[?1006l")
                && transcript.contains("\u{1b}[?1003l")
                && transcript.contains("\u{1b}[?1049l"),
            "fullscreen quota TUI did not restore mouse and alternate-screen modes",
        )?;
        ensure(
            transcript.contains("\u{1b}[?25h") || transcript.contains("\u{1b}[?1049l"),
            "terminal restoration sequence was not observed",
        )?;
        assert_no_printable_output_after_terminal_restoration(transcript.as_bytes())?;
        Ok(())
    }

    #[tokio::test(flavor = "current_thread")]
    async fn compiled_quota_tui_allows_nine_percent_and_commits_one_owned_post() -> TestResult<()> {
        let fixture = QuotaResetFixture::create().await?;
        let mut provider = HeldLoopbackProvider::bind()?;
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

        stage(
            terminal.wait_for_text("ctrl-r reset credits", SEMANTIC_WAIT),
            "committed path initial browse",
        )?;
        terminal.send(b"\x1b[B")?;
        terminal.send(&[0x12])?;
        stage(
            provider
                .wait_for_request_count(2, SEMANTIC_WAIT)
                .map(|_| ()),
            "inspection requests",
        )?;
        provider.release_get_responses()?;
        stage(
            terminal.wait_for_text("Live eligibility", SEMANTIC_WAIT),
            "inspection completion",
        )?;
        stage(
            terminal.wait_for_text("9% · eligible", SEMANTIC_WAIT),
            "below-ten weekly eligibility",
        )?;
        terminal.send(b"\r")?;
        stage(
            terminal.wait_for_text("Confirm reset credit", SEMANTIC_WAIT),
            "confirmation screen",
        )?;
        terminal.send(b"\x1b[C")?;
        stage(
            terminal.wait_for_text("[Yes]", SEMANTIC_WAIT),
            "enabled yes selection",
        )?;
        terminal.send(b"\r")?;

        let requests = provider
            .wait_for_request_count(5, SEMANTIC_WAIT)
            .map_err(|error| std::io::Error::other(format!("commit request ledger: {error}")))?;
        ensure(
            requests
                .iter()
                .filter(|request| request.method == "GET")
                .count()
                == 4,
            "commit path must perform two inspection and two revalidation GETs",
        )?;
        ensure(
            requests
                .iter()
                .filter(|request| request.method == "POST")
                .count()
                == 1,
            "commit path must invoke exactly one POST",
        )?;
        ensure(
            requests.last().is_some_and(|request| {
                request.method == "POST"
                    && request.path == "/api/codex/rate-limit-reset-credits/consume"
            }),
            "consume POST method or path did not match the production protocol",
        )?;
        stage(
            terminal.wait_for_text("Reset request sent", SEMANTIC_WAIT),
            "committing screen",
        )?;
        ensure(
            terminal.child_is_running()?,
            "command exited while the committed POST response was held",
        )?;

        provider.release_post_response()?;
        stage(
            terminal.wait_for_text("Success — reset completed", SEMANTIC_WAIT),
            "known reset result",
        )?;
        let browse_restoration_start = terminal.transcript_len();
        terminal.send(b"\r")?;
        stage(
            terminal.wait_for_text_after(
                "ctrl-r reset credits",
                browse_restoration_start,
                SEMANTIC_WAIT,
            ),
            "browse restoration",
        )?;
        terminal.send(b"q")?;
        let transcript = stage(terminal.finish(SEMANTIC_WAIT), "committed path child exit")?;
        let request_records = provider.finish()?;

        ensure(
            request_records.len() == 5,
            "provider ledger contained an unexpected committed-path request count",
        )?;
        fixture.assert_read_only()?;
        let transcript = String::from_utf8_lossy(&transcript);
        assert_forbidden_terminal_canaries_absent(&transcript)?;
        ensure(
            !transcript.contains("panicked at"),
            "terminal transcript contained a child panic",
        )?;
        assert_no_printable_output_after_terminal_restoration(transcript.as_bytes())?;
        Ok(())
    }

    #[tokio::test(flavor = "current_thread")]
    async fn compiled_quota_tui_floor_arrow_handles_real_mouse_click() -> TestResult<()> {
        let fixture = QuotaResetFixture::create().await?;
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

        stage(
            terminal.wait_for_text("ctrl-e edit floor", SEMANTIC_WAIT),
            "initial floor browse",
        )?;
        terminal.send(&[0x05])?;
        stage(
            terminal.wait_for_text("Weekly floor", SEMANTIC_WAIT),
            "weekly-floor editor",
        )?;
        let click_start = terminal.transcript_len();
        terminal.send_sgr_mouse_left_down(22, 19)?;
        stage(
            terminal.wait_for_text_after("1%", click_start, SEMANTIC_WAIT),
            "pointer-incremented floor",
        )?;
        terminal.send(b"\x1b")?;
        terminal.wait_for_text("ctrl-e edit floor", SEMANTIC_WAIT)?;
        terminal.send(b"q")?;

        let transcript = terminal.finish(SEMANTIC_WAIT)?;
        let request_records = provider.finish()?;
        ensure(
            request_records.is_empty(),
            "weekly-floor pointer editing unexpectedly contacted the provider",
        )?;
        fixture.assert_read_only()?;
        let transcript = String::from_utf8_lossy(&transcript);
        ensure(
            transcript.contains("\u{1b}[?1003h") && transcript.contains("\u{1b}[?1006h"),
            "floor-button pointer test did not run with mouse capture enabled",
        )?;
        assert_no_printable_output_after_terminal_restoration(transcript.as_bytes())?;
        Ok(())
    }

    #[test]
    fn compiled_sessions_tui_click_focuses_preview_and_enter_resumes() -> TestResult<()> {
        let arguments = [OsString::from("--sessions-picker")];
        let mut terminal = TerminalDriver::spawn(
            Path::new(env!("CARGO_BIN_EXE_codex-router-quota-reset-test-harness")),
            arguments,
            Path::new(env!("CARGO_MANIFEST_DIR")),
        )?;
        terminal.resize(24, 160)?;

        stage(
            terminal.wait_for_text("Start new session", SEMANTIC_WAIT),
            "initial sessions picker",
        )?;
        let pointer_focus_start = terminal.transcript_len();
        terminal.send_sgr_mouse_left_down(10, 14)?;
        stage(
            terminal.wait_for_text_after("BETA_PREVIEW_ACTIVE", pointer_focus_start, SEMANTIC_WAIT),
            "pointer-focused existing session preview",
        )?;
        ensure(
            terminal.child_is_running()?,
            "existing-session click activated instead of changing preview focus",
        )?;
        terminal.send(b"\r")?;

        let transcript = terminal.finish(SEMANTIC_WAIT)?;
        let transcript = String::from_utf8_lossy(&transcript);
        ensure(
            transcript.contains("SESSION_PICKER_OUTCOME resume:thread-b"),
            "Enter did not resume the pointer-focused existing session",
        )?;
        ensure(
            transcript.contains("\u{1b}[?1049h")
                && transcript.contains("\u{1b}[?1003h")
                && transcript.contains("\u{1b}[?1006h"),
            "fullscreen sessions TUI did not enable alternate-screen mouse capture",
        )?;
        ensure(
            transcript.contains("\u{1b}[?1006l")
                && transcript.contains("\u{1b}[?1003l")
                && transcript.contains("\u{1b}[?1049l"),
            "fullscreen sessions TUI did not restore mouse and alternate-screen modes",
        )?;
        Ok(())
    }

    #[test]
    fn compiled_sessions_tui_start_new_click_activates_immediately() -> TestResult<()> {
        let arguments = [OsString::from("--sessions-picker")];
        let mut terminal = TerminalDriver::spawn(
            Path::new(env!("CARGO_BIN_EXE_codex-router-quota-reset-test-harness")),
            arguments,
            Path::new(env!("CARGO_MANIFEST_DIR")),
        )?;
        terminal.resize(24, 160)?;

        stage(
            terminal.wait_for_text("Start new session", SEMANTIC_WAIT),
            "initial sessions picker",
        )?;
        terminal.send_sgr_mouse_left_down(10, 7)?;

        let transcript = terminal.finish(SEMANTIC_WAIT)?;
        let transcript = String::from_utf8_lossy(&transcript);
        ensure(
            transcript.contains("SESSION_PICKER_OUTCOME start-new"),
            "Start New click did not activate immediately",
        )?;
        ensure(
            transcript.contains("\u{1b}[?1006l")
                && transcript.contains("\u{1b}[?1003l")
                && transcript.contains("\u{1b}[?1049l"),
            "Start New click did not restore terminal modes",
        )?;
        Ok(())
    }

    #[test]
    fn production_cli_manifest_has_no_harness_install_target_or_pty_dependency() -> TestResult<()> {
        let harness_manifest_root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let cli_manifest =
            fs::read_to_string(harness_manifest_root.join("../codex-router-cli/Cargo.toml"))?;
        let harness_manifest = fs::read_to_string(harness_manifest_root.join("Cargo.toml"))?;
        let workspace_manifest =
            fs::read_to_string(harness_manifest_root.join("../../Cargo.toml"))?;

        ensure(
            !cli_manifest.contains("codex-router-quota-reset-test-harness"),
            "production CLI manifest still owns the harness executable",
        )?;
        ensure(
            !cli_manifest.contains("portable-pty"),
            "production CLI manifest still owns the PTY dependency",
        )?;
        ensure(
            harness_manifest.contains("publish = false"),
            "dedicated harness package is publishable",
        )?;
        ensure(
            workspace_manifest.contains("crates/codex-router-quota-reset-test-harness"),
            "workspace does not own the dedicated harness package",
        )?;
        Ok(())
    }

    #[test]
    fn harness_rejects_unmarked_ordinary_root_before_state_or_secret_access() -> TestResult<()> {
        let ordinary_root = std::env::temp_dir().join(format!(
            "codex-router-ordinary-pty-root-{}",
            std::process::id()
        ));
        fs::create_dir(&ordinary_root)?;
        let output = Command::new(env!("CARGO_BIN_EXE_codex-router-quota-reset-test-harness"))
            .env_clear()
            .env("TMPDIR", std::env::temp_dir())
            .args(["--router-root"])
            .arg(&ordinary_root)
            .args([
                "--fixture-capability",
                "ordinary-root-capability",
                "--provider-listener",
                "127.0.0.1:9",
            ])
            .output()?;

        ensure(
            output.status.code() == Some(2),
            "ordinary root was accepted",
        )?;
        ensure(output.stdout.is_empty(), "root rejection wrote stdout")?;
        ensure(
            String::from_utf8_lossy(&output.stderr).contains("capability marker"),
            "root rejection did not identify the missing capability marker",
        )?;
        ensure(
            !ordinary_root.join("state.sqlite").exists() && !ordinary_root.join("secrets").exists(),
            "root rejection reached state or secret construction",
        )?;
        fs::remove_dir(ordinary_root)?;
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

    #[tokio::test(flavor = "current_thread")]
    async fn semantic_wait_failure_reaps_real_child_listener_tasks_and_fixture() -> TestResult<()> {
        let fixture = QuotaResetFixture::create().await?;
        let fixture_root = fixture.root().to_path_buf();
        let mut provider = HeldLoopbackProvider::bind()?;
        let address = provider.address();
        let arguments = [
            OsString::from("--router-root"),
            fixture.root().as_os_str().to_owned(),
            OsString::from("--fixture-capability"),
            OsString::from(fixture.capability()),
            OsString::from("--provider-listener"),
            OsString::from(address.to_string()),
        ];
        let mut terminal = TerminalDriver::spawn(
            Path::new(env!("CARGO_BIN_EXE_codex-router-quota-reset-test-harness")),
            arguments,
            Path::new(env!("CARGO_MANIFEST_DIR")),
        )?;

        terminal.wait_for_text("ctrl-r reset credits", SEMANTIC_WAIT)?;
        terminal.send(b"\x1b[B")?;
        terminal.send(&[0x12])?;
        provider.wait_for_request_count(2, SEMANTIC_WAIT)?;
        ensure(
            terminal
                .wait_for_text(
                    "semantic-output-that-must-never-exist",
                    Duration::from_millis(100),
                )
                .is_err(),
            "injected semantic wait failure unexpectedly succeeded",
        )?;

        let cleanup_started = Instant::now();
        let transcript = terminal.terminate_and_reap(Duration::from_secs(2))?;
        let records = provider.finish()?;
        fixture.assert_read_only()?;
        drop(fixture);
        ensure(
            cleanup_started.elapsed() < SEMANTIC_WAIT,
            "failure cleanup exceeded its bound",
        )?;
        ensure(
            records.len() == 2,
            "failure cleanup lost provider task records",
        )?;
        ensure(
            TcpListener::bind(address).is_ok(),
            "failure cleanup did not close the loopback listener",
        )?;
        ensure(
            !fixture_root.exists(),
            "failure cleanup retained fixture root",
        )?;
        assert_forbidden_terminal_canaries_absent(&String::from_utf8_lossy(&transcript))?;
        Ok(())
    }

    fn assert_forbidden_terminal_canaries_absent(transcript: &str) -> TestResult<()> {
        let lowercase_transcript = transcript.to_ascii_lowercase();
        for forbidden_canary in FORBIDDEN_TERMINAL_CANARIES {
            ensure(
                !lowercase_transcript.contains(&forbidden_canary.to_ascii_lowercase()),
                "terminal transcript exposed a forbidden canary",
            )?;
        }
        Ok(())
    }

    fn stage<T>(result: TestResult<T>, name: &'static str) -> TestResult<T> {
        result.map_err(|error| std::io::Error::other(format!("{name}: {error}")).into())
    }

    fn assert_no_printable_output_after_terminal_restoration(transcript: &[u8]) -> TestResult<()> {
        const ALTERNATE_SCREEN_RESTORATION: &[u8] = b"\x1b[?1049l";
        const CURSOR_RESTORATION: &[u8] = b"\x1b[?25h";
        let restoration = [ALTERNATE_SCREEN_RESTORATION, CURSOR_RESTORATION]
            .into_iter()
            .filter_map(|sequence| {
                transcript
                    .windows(sequence.len())
                    .rposition(|window| window == sequence)
                    .map(|start| (start, sequence.len()))
            })
            .max_by_key(|(start, _length)| *start)
            .ok_or_else(|| std::io::Error::other("terminal restoration sequence was absent"))?;
        let tail_start = restoration.0 + restoration.1;
        let tail = transcript
            .get(tail_start..)
            .ok_or_else(|| std::io::Error::other("terminal restoration tail was invalid"))?;
        let printable_tail = strip_ansi_control_sequences(tail);
        ensure(
            printable_tail
                .iter()
                .all(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control()),
            "terminal emitted printable output after restoration",
        )
    }

    fn strip_ansi_control_sequences(bytes: &[u8]) -> Vec<u8> {
        let mut plain = Vec::new();
        let mut cursor = 0;
        while let Some(byte) = bytes.get(cursor).copied() {
            if byte != 0x1b {
                plain.push(byte);
                cursor += 1;
                continue;
            }
            cursor += 1;
            if bytes.get(cursor) == Some(&b'[') {
                cursor += 1;
                while let Some(candidate) = bytes.get(cursor).copied() {
                    cursor += 1;
                    if (0x40..=0x7e).contains(&candidate) {
                        break;
                    }
                }
            } else if cursor < bytes.len() {
                cursor += 1;
            }
        }
        plain
    }
}

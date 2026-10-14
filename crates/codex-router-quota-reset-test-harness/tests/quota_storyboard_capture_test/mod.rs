use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use codex_router_core::credit_usage::CreditAvailability;
use codex_router_core::credit_usage::CreditBalance;
use codex_router_core::ids::AccountId;
use codex_router_state::sqlite::AsyncSqliteStateStore;
use sha2::Digest;
use sha2::Sha256;

use super::isolated_fixture_test::FORBIDDEN_TERMINAL_CANARIES;
use super::isolated_fixture_test::QuotaResetFixture;
use super::isolated_fixture_test::TestResult;
use super::isolated_fixture_test::ensure;
use super::loopback_provider_test::HeldLoopbackProvider;

mod capture_receipt;
mod cleanup_test;
mod failure_evidence;
mod owned_capture;
use capture_receipt::write_receipt;
use owned_capture::{OwnedCapture, create_browser_temporary_directory, ensure_owned_child_stopped};
mod provider_filter_capture;
mod provider_filter_tape;
pub(super) use provider_filter_capture::ProviderFilterJourney;
pub(super) use provider_filter_capture::run_provider_filter_storyboard;

#[derive(Clone, Copy)]
enum CapturedTuiSurface {
    Quota,
    Sessions,
}

impl CapturedTuiSurface {
    const fn label(self) -> &'static str {
        match self {
            Self::Quota => "quota",
            Self::Sessions => "sessions",
        }
    }
}

pub(super) async fn run_interrupted_capture_cleanup() -> TestResult<()> {
    cleanup_test::run_interrupted_capture_cleanup().await
}

const IMPOSSIBLE_SCREEN: &str = "STORYBOARD_IMPOSSIBLE_EXPECTED_SCREEN_9A37";
const FONT_FAMILY: &str = "Menlo";
const CHECKPOINTS: [&str; 7] = [
    "browse",
    "resets",
    "credits",
    "disallow-draft",
    "allow-draft",
    "cancelled",
    "returned",
];

pub(super) async fn run_storyboard(
    columns: u16,
    rows: u16,
    expected_timeout: bool,
) -> TestResult<()> {
    let tools = CaptureTools::from_environment()?;
    let output_root =
        PathBuf::from(std::env::var_os("QUOTA_STORYBOARD_OUTPUT").ok_or_else(|| {
            std::io::Error::other(
                "QUOTA_STORYBOARD_OUTPUT must nominate an owned artifact directory",
            )
        })?);
    fs::create_dir_all(&output_root)?;
    let output_root = fs::canonicalize(output_root)?;
    let scene_label = if expected_timeout {
        "wrong-screen"
    } else {
        "journey"
    };
    let output = output_root.join(format!("{scene_label}-{columns}x{rows}"));
    // A pre-existing directory must not supply stale PNGs to a new run.
    fs::create_dir(&output)?;
    fs::set_permissions(&output, fs::Permissions::from_mode(0o700))?;

    let fixture = QuotaResetFixture::create().await?;
    fixture.assert_read_only()?;
    let provider = HeldLoopbackProvider::bind()?;
    provider.release_get_responses()?;
    let binary = fs::canonicalize(env!("CARGO_BIN_EXE_codex-router-quota-reset-test-harness"))?;
    tools.write_fixed_child_adapter(
        &output,
        &binary,
        &fixture,
        provider.address(),
        CapturedTuiSurface::Quota,
    )?;
    let tape = build_tape(columns, rows, expected_timeout);
    fs::write(output.join("quota.tape"), tape)?;
    let mut capture = tools.start_capture(&output, fixture.capability())?;
    let capture_status = capture.wait(Duration::from_secs(90))?;
    capture.release_process_group()?;
    let stderr = fs::read_to_string(output.join("vhs.stderr"))?;
    if expected_timeout {
        ensure(
            !capture_status.success(),
            "impossible screen must fail the capture tool",
        )?;
        ensure(
            stderr.contains(&format!("failed to execute command: timeout waiting for \"Screen {IMPOSSIBLE_SCREEN}\" to match {IMPOSSIBLE_SCREEN}; last value was:")),
            "negative proof must fail for the exact impossible screen, not a launch error",
        )?;
    } else if !capture_status.success() {
        let requests = provider.finish()?;
        failure_evidence::write_failure_evidence(&output, &fixture, &requests).await?;
        return Err(std::io::Error::other(format!(
            "VHS capture exited {capture_status}; inspect owned vhs.stdout/vhs.stderr"
        ))
        .into());
    }

    ensure_owned_child_stopped(&output)?;
    ensure(
        fs::read_to_string(output.join("quota-child.viewport"))? == format!("{columns} {rows}"),
        "actual child PTY must have the requested viewport",
    )?;
    if !expected_timeout {
        ensure(
            fs::read_to_string(output.join("quota-child.exit"))?.trim() == "0",
            "the actual quota child must exit successfully after q",
        )?;
    }
    let requests = provider.finish()?;
    ensure(
        requests.iter().all(|request| request.method == "GET"),
        "storyboard must issue no consume POST",
    )?;
    ensure(
        requests.len() == if expected_timeout { 0 } else { 10 },
        "storyboard request count must match its exact journey",
    )?;
    fixture.assert_secrets_unchanged()?;
    let state = AsyncSqliteStateStore::open_read_only(&fixture.root().join("state.sqlite")).await?;
    for account_name in ["acct_pty_alpha", "acct_pty_beta"] {
        let account_id = AccountId::new(account_name)?;
        ensure(
            !state
                .load_account_credit_usage_policy(&account_id)
                .await?
                .allows_credit_usage(),
            "cancelled Allow draft must leave both saved policies Disallow",
        )?;
    }
    if !expected_timeout {
        let focused = AccountId::new("acct_pty_alpha")?;
        let observation = state
            .load_account_credit_observation(&focused)
            .await?
            .ok_or_else(|| {
                std::io::Error::other("explicit refresh must persist focused credit observation")
            })?;
        ensure(
            observation.provider_observation().availability()
                == &CreditAvailability::Available {
                    balance: Some(CreditBalance::new("42.00")?),
                },
            "focused credit observation must be the sealed provider's 42.00",
        )?;
        let screen_transcript = fs::read_to_string(output.join("screens.txt"))?;
        for canary in FORBIDDEN_TERMINAL_CANARIES {
            ensure(
                !screen_transcript.to_lowercase().contains(canary),
                "terminal leaked a credential canary",
            )?;
        }
        ensure(
            !screen_transcript.contains("panicked at"),
            "compiled TUI panicked",
        )?;
        ensure(
            !screen_transcript.contains("chatgpt.com")
                && !screen_transcript.contains("api.openai.com"),
            "terminal must not mention a production provider origin",
        )?;
        for checkpoint in CHECKPOINTS {
            validate_png(&output.join(format!("{checkpoint}.png")))?;
            tools.verify_image(
                &output,
                checkpoint,
                &output.join(format!("{checkpoint}.png")),
                false,
            )?;
        }
        tools.verify_image(&output, "browse", &output.join("credits.png"), true)?;
        tools.verify_image(
            &output,
            "allow-draft",
            &output.join("disallow-draft.png"),
            true,
        )?;
        tools.verify_image(
            &output,
            "browse",
            Path::new(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/quota_storyboard_capture_test/fixtures/blank-startup.png"
            )),
            true,
        )?;
    }
    state.close().await?;
    write_receipt(
        &output,
        &binary,
        &tools,
        columns,
        rows,
        expected_timeout,
        requests.len(),
    )?;
    let fixture_root = fixture.root().to_path_buf();
    drop(fixture);
    ensure(
        !fixture_root.exists(),
        "owned fixture directory must be removed",
    )?;
    println!("quota storyboard verified: {}", output.display());
    Ok(())
}

fn build_tape(columns: u16, rows: u16, expected_timeout: bool) -> String {
    let mut tape = format!(
        "Set Shell bash\nSet Columns {columns}\nSet Rows {rows}\nSet FontFamily {FONT_FAMILY}\nSet FontSize 16\nSet Padding 0\nSet CursorBlink false\nSet WaitTimeout 8s\nOutput screens.txt\nWait+Screen /ctrl-r account options/\n"
    );
    if expected_timeout {
        tape.push_str(&format!("Wait+Screen@500ms /{IMPOSSIBLE_SCREEN}/\n"));
        return tape;
    }
    // Wait asserts the state; the short interval only lets VHS schedule its PNG
    // frame. PNG content still needs inspection, not an inferred paint guarantee.
    tape.push_str("Sleep 250ms\nScreenshot browse.png\nSleep 250ms\nCtrl+R\nWait+Screen /Reset credits/\nWait+Screen /PTY weekly reset/\nSleep 250ms\nScreenshot resets.png\nSleep 250ms\nTab\nWait+Screen /\\[ Credits \\]/\nWait+Screen /Credit balance/\nType \"r\"\nWait+Screen /Credit balance refreshed/\nWait+Screen /42.00/\nSleep 250ms\nScreenshot credits.png\nSleep 250ms\nEnter\nWait+Screen /› Disallow/\nSleep 250ms\nScreenshot disallow-draft.png\nSleep 250ms\nRight\nWait+Screen /› Allow/\nSleep 250ms\nScreenshot allow-draft.png\nSleep 250ms\nEscape\nWait+Screen /enter edit/\nSleep 250ms\nScreenshot cancelled.png\nSleep 250ms\nEscape\nWait+Screen /ctrl-r account options/\nSleep 250ms\nScreenshot returned.png\nSleep 250ms\nType \"q\"\nWait+Screen /QUOTA_STORYBOARD_CHILD_EXIT=0/\n");
    tape
}

struct CaptureTools {
    vhs: PathBuf,
    ttyd: PathBuf,
    image_verifier: PathBuf,
    dynamic_library_path: String,
}

impl CaptureTools {
    fn from_environment() -> TestResult<Self> {
        Ok(Self {
            vhs: required_executable("QUOTA_STORYBOARD_VHS")?,
            ttyd: required_executable("QUOTA_STORYBOARD_TTYD")?,
            image_verifier: required_executable("QUOTA_STORYBOARD_IMAGE_VERIFIER")?,
            dynamic_library_path: std::env::var("QUOTA_STORYBOARD_DYLD_LIBRARY_PATH")?,
        })
    }

    fn write_fixed_child_adapter(
        &self,
        output: &Path,
        binary: &Path,
        fixture: &QuotaResetFixture,
        provider_address: std::net::SocketAddr,
        surface: CapturedTuiSurface,
    ) -> TestResult<()> {
        let configuration = format!(
            "{{\"binary\":{},\"root\":{},\"capability\":{},\"provider\":{},\"output\":{},\"ttyd\":{},\"temporary_directory\":{},\"dynamic_library_path\":{},\"surface\":{}}}\n",
            encode_json_string(&binary.to_string_lossy()),
            encode_json_string(&fixture.root().to_string_lossy()),
            encode_json_string(fixture.capability()),
            encode_json_string(&provider_address.to_string()),
            encode_json_string(&output.to_string_lossy()),
            encode_json_string(&self.ttyd.to_string_lossy()),
            encode_json_string(&std::env::temp_dir().to_string_lossy()),
            encode_json_string(&self.dynamic_library_path),
            encode_json_string(surface.label()),
        );
        fs::write(output.join("fixture-command.json"), configuration)?;
        // The extracted Homebrew library retains an installed plugin directory.
        // libwebsockets also searches cwd; supply the exact same verified plugin
        // there without installing or rewriting any system library.
        let library_directory = std::env::split_paths(&self.dynamic_library_path)
            .next()
            .ok_or("isolated library directory is missing")?;
        fs::copy(
            library_directory.join("libwebsockets-evlib_uv.dylib"),
            output.join("libwebsockets-evlib_uv.dylib"),
        )?;
        let shim = output.join("shim");
        fs::create_dir(&shim)?;
        fs::write(shim.join("ttyd"), include_str!("fixed_ttyd_adapter.py"))?;
        fs::set_permissions(shim.join("ttyd"), fs::Permissions::from_mode(0o700))?;
        fs::write(
            output.join("fixed_quota_child.py"),
            include_str!("fixed_quota_child.py"),
        )?;
        Ok(())
    }

    fn start_capture(&self, output: &Path, capability: &str) -> TestResult<OwnedCapture> {
        let stdout = fs::File::create(output.join("vhs.stdout"))?;
        let stderr = fs::File::create(output.join("vhs.stderr"))?;
        let browser_temporary_directory = create_browser_temporary_directory(output)?;
        let child = Command::new(&self.vhs)
            .arg("quota.tape")
            .current_dir(output)
            .env_clear()
            .env(
                "PATH",
                format!(
                    "{}:/opt/homebrew/bin:/usr/bin:/bin",
                    output.join("shim").display()
                ),
            )
            .env("HOME", output)
            .env("TMPDIR", &browser_temporary_directory)
            .env("LANG", "C.UTF-8")
            .env("TERM", "xterm-256color")
            .env("DYLD_LIBRARY_PATH", &self.dynamic_library_path)
            .env(
                "QUOTA_STORYBOARD_FIXTURE_COMMAND",
                output.join("fixture-command.json"),
            )
            .stdout(stdout)
            .stderr(stderr)
            .process_group(0)
            .spawn();
        let child = match child {
            Ok(child) => child,
            Err(error) => {
                // No child exists yet to own this directory's cleanup.
                fs::remove_dir_all(&browser_temporary_directory)?;
                return Err(error.into());
            }
        };
        Ok(OwnedCapture {
            child,
            group_released: false,
            output: output.to_path_buf(),
            capability: capability.to_owned(),
            browser_temporary_directory,
        })
    }

    fn verify_image(
        &self,
        output: &Path,
        checkpoint: &str,
        image_path: &Path,
        expect_rejection: bool,
    ) -> TestResult<()> {
        let observation = Command::new(&self.image_verifier)
            .arg(checkpoint)
            .arg(image_path)
            .output()?;
        let label = if expect_rejection {
            format!(
                "reject-{checkpoint}-{}",
                image_path
                    .file_stem()
                    .ok_or("image name missing")?
                    .to_string_lossy()
            )
        } else {
            checkpoint.to_owned()
        };
        fs::write(
            output.join(format!("{label}.image-observation.json")),
            &observation.stdout,
        )?;
        fs::write(
            output.join(format!("{label}.image-observation.stderr")),
            &observation.stderr,
        )?;
        if expect_rejection {
            ensure(
                observation.status.code() == Some(1)
                    && String::from_utf8_lossy(&observation.stdout).contains("missingMarkers"),
                "blank or wrong-scene image must fail its scene oracle, not crash or fail to launch",
            )
        } else {
            ensure(
                observation.status.success(),
                "saved PNG failed its independent visible-scene oracle",
            )
        }
    }
}

fn required_executable(name: &str) -> TestResult<PathBuf> {
    let path = fs::canonicalize(std::env::var_os(name).ok_or_else(|| {
        std::io::Error::other(format!("{name} must nominate the exact isolated tool"))
    })?)?;
    ensure(path.is_file(), "capture tool must be an existing file")?;
    Ok(path)
}

fn validate_png(path: &Path) -> TestResult<()> {
    let bytes = fs::read(path)?;
    ensure(
        bytes.starts_with(b"\x89PNG\r\n\x1a\n") && bytes.len() > 1000,
        "checkpoint must contain an actual PNG",
    )?;
    ensure(
        bytes.get(12..16) == Some(b"IHDR"),
        "PNG must contain its image header",
    )?;
    let width = u32::from_be_bytes(bytes.get(16..20).ok_or("PNG width missing")?.try_into()?);
    let height = u32::from_be_bytes(bytes.get(20..24).ok_or("PNG height missing")?.try_into()?);
    ensure(
        width > 100 && height > 100,
        "checkpoint PNG must contain a viewport",
    )
}

fn digest_file(path: &Path) -> TestResult<String> {
    Ok(format!("{:x}", Sha256::digest(fs::read(path)?)))
}

fn encode_json_string(value: &str) -> String {
    let mut encoded = String::from("\"");
    for character in value.chars() {
        match character {
            '"' => encoded.push_str("\\\""),
            '\\' => encoded.push_str("\\\\"),
            '\n' => encoded.push_str("\\n"),
            '\r' => encoded.push_str("\\r"),
            '\t' => encoded.push_str("\\t"),
            control if control <= '\u{1f}' => {
                encoded.push_str(&format!("\\u{:04x}", u32::from(control)));
            }
            other => encoded.push(other),
        }
    }
    encoded.push('"');
    encoded
}

#[test]
fn fixture_command_strings_round_trip_through_the_actual_python_json_reader() -> TestResult<()> {
    let all_controls: String = (0_u8..=31).map(char::from).collect();
    let old_debug_encoded = format!("{all_controls:?}");
    let old_reader = Command::new("/usr/bin/python3")
        .args(["-c", "import json,sys; json.loads(sys.argv[1])"])
        .arg(old_debug_encoded)
        .output()?;
    ensure(
        !old_reader.status.success()
            && String::from_utf8_lossy(&old_reader.stderr).contains("JSONDecodeError"),
        "old Debug string formatting must fail for JSON control-character escaping",
    )?;
    for expected in [
        "quote\" and backslash\\",
        "Unicode 🦁",
        all_controls.as_str(),
    ] {
        let expected_hex = expected
            .as_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        ensure(
            Command::new("/usr/bin/python3")
                .args(["-c", "import json,sys; assert json.loads(sys.argv[1]).encode('utf-8').hex() == sys.argv[2]"])
                .arg(encode_json_string(expected)).arg(expected_hex).status()?.success(),
            "fixture JSON must round trip through Python's actual reader",
        )?;
    }
    Ok(())
}

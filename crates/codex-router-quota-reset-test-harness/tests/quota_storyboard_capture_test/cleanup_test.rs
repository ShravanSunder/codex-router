use std::fs;
use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;

use super::CaptureTools;
use super::HeldLoopbackProvider;
use super::QuotaResetFixture;
use super::TestResult;
use super::ensure;
use super::ensure_owned_child_stopped;

#[tokio::test(flavor = "current_thread")]
async fn failed_tool_spawn_removes_its_owned_browser_directory() -> TestResult<()> {
    let fixture = QuotaResetFixture::create().await?;
    let tools = CaptureTools {
        vhs: fixture.root().join("absent-vhs-tool"),
        ttyd: PathBuf::new(),
        image_verifier: PathBuf::new(),
        dynamic_library_path: String::new(),
    };
    ensure(
        tools
            .start_capture(fixture.root(), fixture.capability())
            .is_err(),
        "missing capture tool must fail without launching a child",
    )?;
    let directory = fs::read_to_string(fixture.root().join("browser-temp.directory"))?;
    ensure(
        !std::path::Path::new(directory.trim()).exists(),
        "failed tool spawn leaked its owned browser directory",
    )?;
    fixture.assert_read_only()?;
    Ok(())
}

pub(super) async fn run_interrupted_capture_cleanup() -> TestResult<()> {
    let tools = CaptureTools::from_environment()?;
    let output_root = PathBuf::from(
        std::env::var_os("QUOTA_STORYBOARD_OUTPUT").ok_or("capture output directory missing")?,
    );
    fs::create_dir_all(&output_root)?;
    let output = output_root.join("interrupted-capture");
    fs::create_dir(&output)?;
    let output = fs::canonicalize(output)?;
    let fixture = QuotaResetFixture::create().await?;
    let provider = HeldLoopbackProvider::bind()?;
    let binary = fs::canonicalize(env!("CARGO_BIN_EXE_codex-router-quota-reset-test-harness"))?;
    tools.write_fixed_child_adapter(
        &output,
        &binary,
        &fixture,
        provider.address(),
        super::CapturedTuiSurface::Quota,
    )?;
    fs::write(
        output.join("quota.tape"),
        "Set Shell bash\nSet Columns 120\nSet Rows 36\nWait+Screen /ctrl-r account options/\nWait+Screen@30s /INTERRUPTED_CAPTURE_NEVER_MATCHES/\n",
    )?;
    let capture = tools.start_capture(&output, fixture.capability())?;
    let deadline = Instant::now() + Duration::from_secs(10);
    while !output.join("quota-child.pid").exists() {
        ensure(
            Instant::now() < deadline,
            "interruption fixture child did not start",
        )?;
        std::thread::sleep(Duration::from_millis(25));
    }
    ensure(
        super::owned_capture::has_owned_browser(&output)?,
        "interruption must exercise a live owned headless browser",
    )?;
    // Exercise the guard's early-return/drop path while the real child is alive,
    // not just a successfully exited or already-cancelled VHS process.
    drop(capture);
    ensure_owned_child_stopped(&output)?;
    ensure(
        provider.finish()?.is_empty(),
        "interruption must make no provider requests",
    )?;
    fixture.assert_read_only()?;
    let fixture_root = fixture.root().to_path_buf();
    drop(fixture);
    ensure(
        !fixture_root.exists(),
        "interrupted fixture directory survived cleanup",
    )?;
    fs::write(
        output.join("cleanup-verified.txt"),
        "actual child and headless browser started; early drop reaped owned quota/wrapper/ttyd/browser; no provider requests; fixture read-only and removed\n",
    )?;
    Ok(())
}

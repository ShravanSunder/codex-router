//! Same-run compiled UI journeys. Inventory is synthetic; input and rendering are real.
use super::*;
use crate::isolated_fixture_test::ProviderFilterFixtureScenario;

#[derive(Clone, Copy)]
pub(crate) enum ProviderFilterJourney {
    QuotaMixed,
    QuotaEmpty,
    Sessions,
}

pub(crate) async fn run_provider_filter_storyboard(
    columns: u16,
    rows: u16,
    journey: ProviderFilterJourney,
) -> TestResult<()> {
    let sessions = matches!(journey, ProviderFilterJourney::Sessions);
    let empty_quota = matches!(journey, ProviderFilterJourney::QuotaEmpty);
    let surface = if sessions {
        CapturedTuiSurface::Sessions
    } else {
        CapturedTuiSurface::Quota
    };
    let tools = CaptureTools::from_environment()?;
    let output_root = PathBuf::from(
        std::env::var_os("QUOTA_STORYBOARD_OUTPUT")
            .ok_or("fresh filter storyboard output root required")?,
    );
    fs::create_dir_all(&output_root)?;
    let output = fs::canonicalize(output_root)?.join(format!(
        "{}-{}-{columns}x{rows}",
        surface.label(),
        if empty_quota { "empty" } else { "filters" }
    ));
    fs::create_dir(&output)?;
    fs::set_permissions(&output, fs::Permissions::from_mode(0o700))?;
    let fixture = QuotaResetFixture::create_provider_filters(if empty_quota {
        ProviderFilterFixtureScenario::ClaudeAccountsDisabled
    } else {
        ProviderFilterFixtureScenario::MixedAccounts
    })
    .await?;
    fixture.assert_read_only()?;
    let provider = HeldLoopbackProvider::bind()?;
    let binary = fs::canonicalize(env!("CARGO_BIN_EXE_codex-router-quota-reset-test-harness"))?;
    tools.write_fixed_child_adapter(&output, &binary, &fixture, provider.address(), surface)?;
    let (tape, checkpoints) = build_filter_tape(surface, columns, rows, empty_quota);
    fs::write(output.join("quota.tape"), tape)?;
    let mut capture = tools.start_capture(&output, fixture.capability())?;
    let status = capture.wait(Duration::from_secs(90))?;
    if !status.success() {
        record_failed_child_state(&output, &binary, fixture.capability())?;
    }
    capture.release_process_group()?;
    ensure(
        status.success(),
        "filter capture failed: inspect owned VHS stdout/stderr",
    )?;
    ensure_owned_child_stopped(&output)?;
    ensure(
        fs::read_to_string(output.join("quota-child.viewport"))? == format!("{columns} {rows}"),
        "filter child viewport mismatch",
    )?;
    ensure(
        fs::read_to_string(output.join("quota-child.exit"))?.trim() == "0",
        "filter child failed",
    )?;
    ensure(
        provider.finish()?.is_empty(),
        "filtering contacted the provider",
    )?;
    fixture.assert_read_only()?;
    let transcript = fs::read_to_string(output.join("screens.txt"))?;
    for canary in FORBIDDEN_TERMINAL_CANARIES {
        ensure(
            !transcript.to_lowercase().contains(canary),
            "filter screen leaked credential canary",
        )?;
    }
    ensure(!transcript.contains("panicked at"), "filter TUI panicked")?;
    if sessions {
        ensure(
            transcript.contains("SESSION_PICKER_OUTCOME canceled"),
            "session journey must exit canceled without launching any session",
        )?;
    }
    for checkpoint in &checkpoints {
        let png = output.join(format!("{checkpoint}.png"));
        validate_png(&png)?;
        tools.verify_image(&output, checkpoint, &png, false)?;
    }
    let blank = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/quota_storyboard_capture_test/fixtures/blank-startup.png"
    ));
    tools.verify_image(
        &output,
        if sessions {
            "sessions-codex"
        } else {
            "quota-claude"
        },
        blank,
        true,
    )?;
    if !empty_quota {
        let (expected, wrong) = if sessions {
            ("sessions-codex", "sessions-claude")
        } else {
            ("quota-openai", "quota-claude")
        };
        tools.verify_image(
            &output,
            expected,
            &output.join(format!("{wrong}.png")),
            true,
        )?;
        tools.verify_image(
            &output,
            if sessions {
                "sessions-draft"
            } else {
                "quota-draft"
            },
            &output.join(if sessions {
                "sessions-chooser.png"
            } else {
                "quota-chooser.png"
            }),
            true,
        )?;
    }
    write_filter_receipt(
        &output,
        &binary,
        &tools,
        columns,
        rows,
        surface,
        &checkpoints,
    )?;
    let fixture_root = fixture.root().to_owned();
    drop(fixture);
    ensure(!fixture_root.exists(), "filter fixture survived cleanup")?;
    println!("compiled filter storyboard verified: {}", output.display());
    Ok(())
}

// Capture a bounded stack observation before cleanup destroys the evidence. This
// does not retry, send input, redraw, or change the application's startup path.
fn record_failed_child_state(output: &Path, binary: &Path, capability: &str) -> TestResult<()> {
    let record = output.join("quota-child.pid");
    if !record.exists() {
        return Ok(());
    }
    let pid: u32 = fs::read_to_string(record)?.trim().parse()?;
    let snapshot = Command::new("/bin/ps")
        .args(["-p", &pid.to_string(), "-o", "command="])
        .output()?;
    fs::write(output.join("failed-child-command.txt"), &snapshot.stdout)?;
    if !snapshot.status.success() {
        return Ok(());
    }
    let command = String::from_utf8_lossy(&snapshot.stdout);
    ensure(
        command.contains(binary.to_string_lossy().as_ref()) && command.contains(capability),
        "refusing to sample a process outside this exact owned fixture",
    )?;
    let observation = Command::new("/usr/bin/sample")
        .args([&pid.to_string(), "1", "1", "-file"])
        .arg(output.join("failed-child.sample.txt"))
        .output()?;
    fs::write(
        output.join("failed-child-sample.stdout"),
        observation.stdout,
    )?;
    fs::write(
        output.join("failed-child-sample.stderr"),
        observation.stderr,
    )?;
    Ok(())
}

fn build_filter_tape(
    surface: CapturedTuiSurface,
    columns: u16,
    rows: u16,
    empty: bool,
) -> (String, Vec<&'static str>) {
    let mut tape = format!(
        "Set Shell bash\nSet Columns {columns}\nSet Rows {rows}\nSet FontFamily {FONT_FAMILY}\nSet FontSize 16\nSet Padding 0\nSet CursorBlink false\nSet WaitTimeout 8s\nOutput screens.txt\n"
    );
    let mut checkpoints = Vec::new();
    match surface {
        CapturedTuiSurface::Sessions => {
            checkpoint(&mut tape, &mut checkpoints, "sessions-all", "6/6");
            tape.push_str("Ctrl+F\n");
            checkpoint(
                &mut tape,
                &mut checkpoints,
                "sessions-chooser",
                "Draft: All providers",
            );
            tape.push_str("Down\nEnter\n");
            checkpoint(&mut tape, &mut checkpoints, "sessions-codex", "2/6.*Codex");
            tape.push_str("F4\n");
            checkpoint(
                &mut tape,
                &mut checkpoints,
                "sessions-details",
                "Session details.*Codex Alpha",
            );
            tape.push_str("Escape\n");
            checkpoint(
                &mut tape,
                &mut checkpoints,
                "sessions-details-closed",
                "2/6.*Codex",
            );
            tape.push_str("Down\n");
            checkpoint(
                &mut tape,
                &mut checkpoints,
                "sessions-codex-bottom",
                "Codex Beta",
            );
            tape.push_str("Ctrl+F\nDown\nEnter\n");
            checkpoint(
                &mut tape,
                &mut checkpoints,
                "sessions-claude",
                "2/6.*Claude Code",
            );
            tape.push_str("Down\n");
            checkpoint(
                &mut tape,
                &mut checkpoints,
                "sessions-claude-bottom",
                "Claude Delta",
            );
            tape.push_str("Ctrl+F\nDown\nEnter\n");
            checkpoint(
                &mut tape,
                &mut checkpoints,
                "sessions-cursor",
                "1/6.*Cursor",
            );
            tape.push_str("Ctrl+F\nUp\nUp\n");
            checkpoint(
                &mut tape,
                &mut checkpoints,
                "sessions-draft",
                "Draft: Codex",
            );
            tape.push_str("Escape\n");
            checkpoint(
                &mut tape,
                &mut checkpoints,
                "sessions-cancelled",
                "1/6.*Cursor",
            );
            tape.push_str("Up\n");
            checkpoint(
                &mut tape,
                &mut checkpoints,
                "sessions-new-choice",
                "New Codex session",
            );
            tape.push_str("Type \"zznomatchzz\"\n");
            checkpoint(&mut tape, &mut checkpoints, "sessions-empty", "0/6");
            tape.push_str("Escape\nCtrl+F\nUp\nUp\nUp\nEnter\n");
            checkpoint(&mut tape, &mut checkpoints, "sessions-restored", "6/6");
            // Complementary final page demonstrates rows below the first viewport.
            tape.push_str("Down\nDown\nDown\nDown\nDown\nDown\n");
            checkpoint(
                &mut tape,
                &mut checkpoints,
                "sessions-all-bottom",
                "Claude-lookalike Zeta",
            );
            tape.push_str("Ctrl+D\n");
        }
        CapturedTuiSurface::Quota => {
            if empty {
                tape.push_str(
                    "Wait+Screen /4\\/4 accounts/\nCtrl+F\nDown\nDown\nTab\nSpace\nEnter\n",
                );
                checkpoint(
                    &mut tape,
                    &mut checkpoints,
                    "quota-empty",
                    "No accounts match these filters",
                );
            } else {
                checkpoint(&mut tape, &mut checkpoints, "quota-all", "4/4 accounts");
                tape.push_str("Up\nUp\nUp\n");
                checkpoint(&mut tape, &mut checkpoints, "quota-all-top", "Claude Delta");
                tape.push_str("Down\n");
                checkpoint(
                    &mut tape,
                    &mut checkpoints,
                    "quota-all-next",
                    "Claude Gamma",
                );
                tape.push_str("Ctrl+F\n");
                checkpoint(&mut tape, &mut checkpoints, "quota-chooser", "Draft: All");
                tape.push_str("Down\nEnter\n");
                checkpoint(
                    &mut tape,
                    &mut checkpoints,
                    "quota-openai",
                    "OpenAI.*2/4 accounts",
                );
                tape.push_str("Down\n");
                checkpoint(
                    &mut tape,
                    &mut checkpoints,
                    "quota-openai-bottom",
                    "OpenAI Beta",
                );
                tape.push_str("Ctrl+F\nDown\nEnter\n");
                checkpoint(
                    &mut tape,
                    &mut checkpoints,
                    "quota-claude",
                    "Claude.*2/4 accounts",
                );
                tape.push_str("Down\n");
                checkpoint(
                    &mut tape,
                    &mut checkpoints,
                    "quota-claude-bottom",
                    "Claude Gamma",
                );
                tape.push_str("Ctrl+F\nTab\nSpace\nEnter\n");
                checkpoint(
                    &mut tape,
                    &mut checkpoints,
                    "quota-hidden",
                    "Claude.*Disabled: Hidden.*1/4 accounts",
                );
                tape.push_str("F4\n");
                checkpoint(
                    &mut tape,
                    &mut checkpoints,
                    "quota-details",
                    "Account details",
                );
                tape.push_str("Escape\n");
                checkpoint(
                    &mut tape,
                    &mut checkpoints,
                    "quota-details-closed",
                    "Claude.*Disabled: Hidden.*1/4 accounts",
                );
                tape.push_str("Ctrl+F\nUp\n");
                checkpoint(&mut tape, &mut checkpoints, "quota-draft", "Draft: OpenAI");
                tape.push_str("Escape\n");
                checkpoint(
                    &mut tape,
                    &mut checkpoints,
                    "quota-cancelled",
                    "Claude.*Disabled: Hidden.*1/4 accounts",
                );
                tape.push_str("Ctrl+F\nUp\nUp\nTab\nSpace\nEnter\n");
                checkpoint(
                    &mut tape,
                    &mut checkpoints,
                    "quota-restored",
                    "All.*Disabled: Shown.*4/4 accounts",
                );
                tape.push_str("Down\nDown\nDown\n");
                checkpoint(
                    &mut tape,
                    &mut checkpoints,
                    "quota-all-bottom",
                    "OpenAI Beta",
                );
            }
            tape.push_str("Type \"q\"\n");
        }
    }
    tape.push_str("Wait+Screen /QUOTA_STORYBOARD_CHILD_EXIT=0/\n");
    (tape, checkpoints)
}

fn checkpoint(
    tape: &mut String,
    checkpoints: &mut Vec<&'static str>,
    name: &'static str,
    expected: &str,
) {
    let expression = expected.replace('/', "\\/");
    tape.push_str(&format!(
        "Wait+Screen /(?s){expression}/\nSleep 250ms\nScreenshot {name}.png\nSleep 250ms\n"
    ));
    checkpoints.push(name);
}

fn write_filter_receipt(
    output: &Path,
    binary: &Path,
    tools: &CaptureTools,
    columns: u16,
    rows: u16,
    surface: CapturedTuiSurface,
    checkpoints: &[&str],
) -> TestResult<()> {
    let images = checkpoints
        .iter()
        .map(|name| {
            Ok(format!(
                "{{\"checkpoint\":{},\"png_sha256\":{}}}",
                encode_json_string(name),
                encode_json_string(&digest_file(&output.join(format!("{name}.png")))?)
            ))
        })
        .collect::<TestResult<Vec<_>>>()?
        .join(",");
    let verifier_source = format!(
        "{:x}",
        Sha256::digest(include_bytes!("verify_screen_image.swift"))
    );
    let driver_source = format!(
        "{:x}",
        Sha256::digest(include_bytes!("provider_filter_capture.rs"))
    );
    fs::write(
        output.join("filter-receipt.json"),
        format!(
            "{{\"surface\":{},\"columns\":{columns},\"rows\":{rows},\"binary_sha256\":{},\"vhs_sha256\":{},\"ttyd_sha256\":{},\"verifier_sha256\":{},\"verifier_source_sha256\":{},\"driver_source_sha256\":{},\"fixed_child_sha256\":{},\"tape_sha256\":{},\"state_and_secrets_unchanged\":true,\"provider_requests\":0,\"child_exit\":0,\"owned_processes_stopped\":true,\"fixture\":\"synthetic typed inputs; actual compiled UI/input/rendering\",\"images\":[{images}]}}\n",
            encode_json_string(surface.label()),
            encode_json_string(&digest_file(binary)?),
            encode_json_string(&digest_file(&tools.vhs)?),
            encode_json_string(&digest_file(&tools.ttyd)?),
            encode_json_string(&digest_file(&tools.image_verifier)?),
            encode_json_string(&verifier_source),
            encode_json_string(&driver_source),
            encode_json_string(&digest_file(&output.join("fixed_quota_child.py"))?),
            encode_json_string(&digest_file(&output.join("quota.tape"))?)
        ),
    )?;
    Ok(())
}

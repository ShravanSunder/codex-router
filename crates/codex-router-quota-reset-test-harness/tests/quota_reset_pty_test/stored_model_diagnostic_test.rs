//! Real PTY/dispatcher ordering with sealed catalog and native-launch stand-ins.
use super::*;

const MODEL_WARNING: &str = "agent-sessions: stored model or reasoning effort for session thread-b contains characters that cannot be passed to Codex; resuming without it";
const RAW_MODEL: &str = "MALFORMED_MODEL\u{1}VALUE";

fn focused_malformed_model_picker() -> TestResult<TerminalDriver> {
    let mut terminal = TerminalDriver::spawn(
        Path::new(env!("CARGO_BIN_EXE_codex-router-quota-reset-test-harness")),
        [OsString::from("--sessions-picker")],
        Path::new(env!("CARGO_MANIFEST_DIR")),
    )?;
    terminal.resize(24, 160)?;
    terminal.wait_for_text("New Codex session", SEMANTIC_WAIT)?;
    let focus_start = terminal.transcript_len();
    terminal.send_sgr_mouse_left_down(10, 14)?;
    terminal.wait_for_text_after("BETA_CONVERSATION_ACTIVE", focus_start, SEMANTIC_WAIT)?;
    ensure(
        terminal.child_is_running()?,
        "focus must not launch the selected session",
    )?;
    Ok(terminal)
}

#[test]
fn compiled_picker_discarded_model_warning_follows_teardown_and_precedes_resume() -> TestResult<()>
{
    let mut terminal = focused_malformed_model_picker()?;
    terminal.send(b"\r")?;
    let transcript = terminal.finish(SEMANTIC_WAIT)?;
    let text = String::from_utf8_lossy(&transcript);
    ensure(
        text.matches(MODEL_WARNING).count() == 1,
        "exactly one fixed model warning required",
    )?;
    let warning_offset = text.find(MODEL_WARNING).ok_or("fixed warning absent")?;
    let runner_offset = text
        .find("SESSION_PICKER_OUTCOME resume:thread-b")
        .ok_or("real dispatcher runner marker absent")?;
    for restoration in ["\u{1b}[?1006l", "\u{1b}[?1003l", "\u{1b}[?1049l"] {
        let teardown_offset = text
            .rfind(restoration)
            .ok_or("terminal restoration absent")?;
        ensure(
            teardown_offset < warning_offset,
            "fixed model warning was emitted before terminal teardown",
        )?;
    }
    ensure(
        warning_offset < runner_offset,
        "diagnostic must precede resume runner",
    )?;
    ensure(
        !text.contains(RAW_MODEL),
        "raw malformed stored model reached terminal",
    )?;
    Ok(())
}

#[test]
fn compiled_picker_canceling_captured_model_emits_no_warning_or_launch() -> TestResult<()> {
    let mut terminal = focused_malformed_model_picker()?;
    let popup_start = terminal.transcript_len();
    terminal.send(b"\x1b\r")?;
    terminal.wait_for_text_after("Fork session", popup_start, SEMANTIC_WAIT)?;
    let cancel_start = terminal.transcript_len();
    terminal.send(b"\x1b")?;
    terminal.wait_for_text_after("BETA_CONVERSATION_ACTIVE", cancel_start, SEMANTIC_WAIT)?;
    terminal.send(&[0x03])?;
    let transcript = terminal.finish(SEMANTIC_WAIT)?;
    let text = String::from_utf8_lossy(&transcript);
    ensure(
        text.contains("SESSION_PICKER_OUTCOME canceled"),
        "whole picker cancellation not observed",
    )?;
    ensure(
        !text.contains(MODEL_WARNING),
        "canceled capture emitted stored model warning",
    )?;
    ensure(
        !text.contains("SESSION_PICKER_OUTCOME resume:")
            && !text.contains("SESSION_PICKER_OUTCOME fork:"),
        "canceled picker invoked native runner",
    )?;
    ensure(
        !text.contains(RAW_MODEL),
        "canceled picker leaked raw malformed model",
    )?;
    for restoration in ["\u{1b}[?1006l", "\u{1b}[?1003l", "\u{1b}[?1049l"] {
        ensure(
            text.contains(restoration),
            "canceled picker did not restore terminal mode",
        )?;
    }
    Ok(())
}

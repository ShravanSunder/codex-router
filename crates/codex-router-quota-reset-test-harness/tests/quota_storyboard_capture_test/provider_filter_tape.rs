//! VHS inputs and literal compact scene contract; owned lifecycle stays separate.
use super::{CapturedTuiSurface, FONT_FAMILY};

pub(super) fn build_filter_tape(
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

pub(super) fn build_compact_filter_tape(
    surface: CapturedTuiSurface,
    columns: u16,
    rows: u16,
) -> (String, Vec<&'static str>) {
    let mut tape = format!(
        "Set Shell bash\nSet Columns {columns}\nSet Rows {rows}\nSet FontFamily {FONT_FAMILY}\nSet FontSize 16\nSet Padding 0\nSet CursorBlink false\nSet WaitTimeout 8s\nOutput screens.txt\n"
    );
    let mut checkpoints = Vec::new();
    match surface {
        CapturedTuiSurface::Sessions => {
            checkpoint(
                &mut tape,
                &mut checkpoints,
                "compact-sessions-all",
                "6/6.*Codex Alpha",
            );
            tape.push_str("Ctrl+F\nDown\n");
            checkpoint(
                &mut tape,
                &mut checkpoints,
                "compact-sessions-chooser",
                "Draft: Codex",
            );
            tape.push_str("F1\n");
            checkpoint(
                &mut tape,
                &mut checkpoints,
                "compact-sessions-help",
                "Sessions Help.*Choose provider draft",
            );
            tape.push_str("Type \"x\"\nEnter\nEscape\nWait+Screen /Draft: Codex/\nDown\nEnter\n");
            checkpoint(
                &mut tape,
                &mut checkpoints,
                "compact-sessions-claude",
                "2/6.*Claude Gamma",
            );
            tape.push_str("Ctrl+F\nDown\nWait+Screen /Draft: Cursor/\nEscape\n");
            checkpoint(
                &mut tape,
                &mut checkpoints,
                "compact-sessions-cancelled",
                "2/6.*Claude Gamma",
            );
            tape.push_str("F4\n");
            checkpoint(
                &mut tape,
                &mut checkpoints,
                "compact-sessions-details",
                "Session details.*Claude Gamma",
            );
            tape.push_str("Escape\nWait+Screen /(?s)2\\/6.*Claude Gamma/\nCtrl+D\n");
        }
        CapturedTuiSurface::Quota => {
            tape.push_str("Wait+Screen /4\\/4 accounts/\nUp\nUp\nUp\n");
            checkpoint(
                &mut tape,
                &mut checkpoints,
                "compact-quota-all",
                "4/4 accounts.*Claude Delta",
            );
            tape.push_str("Ctrl+F\nDown\n");
            checkpoint(
                &mut tape,
                &mut checkpoints,
                "compact-quota-chooser",
                "Draft: OpenAI",
            );
            tape.push_str("F1\n");
            checkpoint(
                &mut tape,
                &mut checkpoints,
                "compact-quota-help",
                "Quota Help.*Apply draft",
            );
            tape.push_str(
                "Down\nEnter\nEscape\nWait+Screen /Draft: OpenAI/\nDown\nTab\nSpace\nEnter\n",
            );
            checkpoint(
                &mut tape,
                &mut checkpoints,
                "compact-quota-claude",
                "Disabled: Hidden.*1/4 accounts.*Claude Gamma",
            );
            tape.push_str("Ctrl+F\nUp\nWait+Screen /Draft: OpenAI/\nEscape\n");
            checkpoint(
                &mut tape,
                &mut checkpoints,
                "compact-quota-cancelled",
                "Disabled: Hidden.*1/4 accounts.*Claude Gamma",
            );
            tape.push_str("F4\n");
            checkpoint(
                &mut tape,
                &mut checkpoints,
                "compact-quota-details",
                "Account details.*Claude Gamma",
            );
            tape.push_str("Escape\nWait+Screen /(?s)Disabled: Hidden.*1\\/4 accounts.*Claude Gamma/\nType \"q\"\n");
        }
    }
    tape.push_str("Wait+Screen /QUOTA_STORYBOARD_CHILD_EXIT=0/\n");
    (tape, checkpoints)
}

#[cfg(test)]
mod compact_capture_contract_tests {
    use super::*;

    #[test]
    fn compact_capture_preserves_six_literal_checkpoints_per_surface() {
        for (surface, expected) in [
            (
                CapturedTuiSurface::Sessions,
                [
                    "compact-sessions-all",
                    "compact-sessions-chooser",
                    "compact-sessions-help",
                    "compact-sessions-claude",
                    "compact-sessions-cancelled",
                    "compact-sessions-details",
                ],
            ),
            (
                CapturedTuiSurface::Quota,
                [
                    "compact-quota-all",
                    "compact-quota-chooser",
                    "compact-quota-help",
                    "compact-quota-claude",
                    "compact-quota-cancelled",
                    "compact-quota-details",
                ],
            ),
        ] {
            let (tape, checkpoints) = build_compact_filter_tape(surface, 48, 24);
            assert_eq!(
                checkpoints, expected,
                "only the accepted compact journey belongs in this capture"
            );
            assert_eq!(tape.matches("Screenshot ").count(), 6);
            assert!(tape.contains("Wait+Screen /QUOTA_STORYBOARD_CHILD_EXIT=0/"));
            for forbidden in [
                "resets.png",
                "credits.png",
                "sessions-empty.png",
                "quota-empty.png",
            ] {
                assert!(!tape.contains(forbidden), "unrelated scene: {tape}");
            }
        }
    }
}

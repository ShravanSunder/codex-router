//! Compact chrome must leave the viewport to records, while active state stays visible.
use super::*;
use crate::presentation::session_picker::provider_filter_tests::mixed_request;

fn compact_request() -> SessionsPickerRequest {
    let mut request = mixed_request();
    // Literal ordering makes the two rows under test the first real records.
    request.records[0].recency_at_ms = Some(9_000);
    request.records[1].recency_at_ms = Some(8_000);
    request
}

#[tokio::test]
async fn compact_header_machine_and_provider_share_one_row_without_idle_settings() {
    for (width, height) in [(120, 36), (76, 30), (48, 24)] {
        let frame =
            render_picker_capture_at(compact_request(), width, height, vec![ctrl_key('d')]).await;
        assert!(
            frame
                .lines()
                .any(|line| line.contains("This machine") && line.contains("All providers")),
            "machine/provider must share one row at {width}: {frame}"
        );
        assert!(
            frame.contains("Sessions") && frame.contains("Sessions · 5/5"),
            "short title and count: {frame}"
        );
        for idle in ["Search: []", "View: [All]", "Sort: [updated]"] {
            assert!(
                !frame.contains(idle),
                "idle setting consumes a row: {frame}"
            );
        }
        for name in ["Native Alpha", "Native Beta"] {
            assert!(
                frame.contains(name),
                "compact chrome must leave multiple real records visible: {frame}"
            );
        }
        assert!(frame.lines().count() <= height, "{frame}");
    }
}

#[tokio::test]
async fn compact_header_active_search_stays_visible_and_escape_restores_rows() {
    let key = |character| {
        TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Char(character)))
    };
    let frame = render_picker_capture_at(
        compact_request(),
        48,
        24,
        vec![key('B'), key('e'), key('t'), key('a'), ctrl_key('d')],
    )
    .await;
    assert!(
        frame.contains("Search: [Beta]") && frame.contains("Native Beta"),
        "{frame}"
    );
    assert!(!frame.contains("Native Alpha"), "{frame}");
    let restored = render_picker_capture_at(
        compact_request(),
        48,
        24,
        vec![
            key('B'),
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Esc)),
            ctrl_key('d'),
        ],
    )
    .await;
    assert!(
        !restored.contains("Search: []") && restored.contains("Native Alpha"),
        "{restored}"
    );
}

#[test]
fn compact_header_text_respects_terminal_cell_width_for_wide_machine_names() {
    use unicode_width::UnicodeWidthStr;
    // CJK letters occupy two terminal columns; a character count is not a cell budget.
    for width in [16, 7, 5, 1, 0] {
        let rendered = fit_line("日本語の長いマシン名", width);
        assert!(
            UnicodeWidthStr::width(rendered.as_str()) <= width,
            "wide alias consumes the shortcut's cells at width {width}: {rendered}"
        );
    }
    assert_eq!(fit_line("This machine", 16), "This machine");
}

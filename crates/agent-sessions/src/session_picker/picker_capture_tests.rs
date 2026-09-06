use super::*;

#[tokio::test]
#[ignore = "writes visual session picker capture artifacts for design review"]
async fn sessions_picker_capture_artifacts_for_design_review() {
    let capture_dir = capture_dir();

    for width in [48, 80, 120] {
        let text = render_picker_capture(
            capture_picker_request(),
            width,
            vec![
                TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Down)),
                TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Enter)),
            ],
        )
        .await;
        write_capture_pair(&capture_dir, &format!("sessions-{width}"), &text);
    }

    for (width, height) in [(160, 24), (160, 32), (100, 24)] {
        let text = render_picker_capture_at(
            capture_picker_request(),
            width,
            height,
            vec![TerminalEvent::Key(KeyEvent::new(
                KeyEventKind::Press,
                KeyCode::Esc,
            ))],
        )
        .await;
        write_capture_pair(&capture_dir, &format!("sessions-{width}x{height}"), &text);
    }

    let mut empty_request = picker_request();
    empty_request.records.clear();
    let empty_text = render_picker_capture(
        empty_request,
        80,
        vec![TerminalEvent::Key(KeyEvent::new(
            KeyEventKind::Press,
            KeyCode::Esc,
        ))],
    )
    .await;
    write_capture_pair(&capture_dir, "sessions-empty-80", &empty_text);
}

use super::*;
use crate::presentation::session_picker::provider_filter_tests::mixed_request;
use unicode_width::UnicodeWidthStr;

fn key(code: KeyCode) -> TerminalEvent {
    TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, code))
}

#[tokio::test]
async fn sessions_provider_filter_real_keys_apply_cancel_and_hide_nonmatching_rows() {
    let mut outcome = Option::<SessionsPickerOutcome>::None;
    let frames = element! {
        SessionsPickerComponent(request: mixed_request(), width: 120usize, height: 36usize, selected_outcome_out: &mut outcome)
    }
    .mock_terminal_render_loop(MockTerminalConfig::with_events(futures_util::stream::iter(vec![
        ctrl_key('f'), key(KeyCode::Down), key(KeyCode::Down), key(KeyCode::Down), key(KeyCode::Enter),
        ctrl_key('f'), key(KeyCode::Up), key(KeyCode::Up), key(KeyCode::Tab), key(KeyCode::Tab), key(KeyCode::Enter),
        ctrl_key('f'), key(KeyCode::Up), key(KeyCode::Esc),
        ctrl_key('d'),
    ])))
    .map(|canvas| canvas.to_string()).collect::<Vec<_>>().await;
    assert_eq!(outcome, None);
    let final_frame = frames.last().unwrap();
    assert!(
        final_frame.contains("Filters · Cursor") && final_frame.contains("Sessions · 1/5"),
        "{final_frame}"
    );
    assert!(!final_frame.contains("Native Alpha"), "{final_frame}");
    assert!(!final_frame.contains("Native Beta"), "{final_frame}");
    assert!(
        !final_frame.contains("Claude Code guessed label"),
        "{final_frame}"
    );
    assert!(
        !final_frame.contains("Session client provider"),
        "Cancel must close: {final_frame}"
    );
    assert!(final_frame.contains("New Codex session"), "{final_frame}");
}

#[tokio::test]
async fn sessions_provider_filter_real_keys_respect_fork_and_machine_modals() {
    for (opening, expected_modal) in [
        (alt_enter_key(), "Fork session"),
        (ctrl_key('g'), "Choose machine"),
    ] {
        let frames = element! {
            SessionsPickerComponent(request: picker_request(), width: 120usize, height: 36usize)
        }
        .mock_terminal_render_loop(MockTerminalConfig::with_events(futures_util::stream::iter(
            vec![opening, ctrl_key('f'), key(KeyCode::F(3)), ctrl_key('d')],
        )))
        .map(|canvas| canvas.to_string())
        .collect::<Vec<_>>()
        .await;
        assert!(
            frames.iter().any(|frame| frame.contains(expected_modal)),
            "modal input must execute: {frames:?}"
        );
        assert!(
            frames
                .iter()
                .all(|frame| !frame.contains("Session client provider")),
            "{frames:?}"
        );
    }
}

#[tokio::test]
async fn sessions_provider_filter_mouse_uses_same_draft_and_apply_path() {
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    let events = futures_util::stream::unfold(receiver, |mut receiver| async {
        receiver.recv().await.map(|event| (event, receiver))
    });
    let (phase, final_frame) = tokio::time::timeout(Duration::from_secs(2), async {
        let mut picker = element! {
            SessionsPickerComponent(request: mixed_request(), width: 120usize, height: 36usize)
        };
        let canvases = picker.mock_terminal_render_loop(MockTerminalConfig::with_events(events));
        tokio::pin!(canvases);
        let mut phase = 0;
        let mut final_frame = String::new();
        while let Some(canvas) = canvases.next().await {
            let frame = canvas.to_string();
            if phase == 0 && frame.contains("Filters · All providers") {
                let row = frame
                    .lines()
                    .position(|line| line.contains("Filters · All providers"))
                    .unwrap();
                sender
                    .send(TerminalEvent::FullscreenMouse(FullscreenMouseEvent::new(
                        MouseEventKind::Down(MouseButton::Left),
                        {
                            let line = frame.lines().nth(row).unwrap();
                            let offset = line.find("Filters").unwrap();
                            UnicodeWidthStr::width(
                                line.get(..offset)
                                    .expect("ASCII label starts at a UTF-8 boundary"),
                            ) as u16
                        },
                        row as u16,
                    )))
                    .unwrap();
                phase = 1;
            } else if phase == 1 && frame.contains("Session client provider") {
                let row = frame
                    .lines()
                    .position(|line| line.contains("○ Cursor"))
                    .unwrap();
                sender
                    .send(TerminalEvent::FullscreenMouse(FullscreenMouseEvent::new(
                        MouseEventKind::Down(MouseButton::Left),
                        frame
                            .lines()
                            .nth(row)
                            .unwrap()
                            .chars()
                            .position(|character| character == '○')
                            .unwrap() as u16,
                        row as u16,
                    )))
                    .unwrap();
                phase = 2;
            } else if phase == 2 && frame.contains("● Cursor") {
                assert!(
                    frame.contains("Session client provider") && frame.contains("Draft: Cursor"),
                    "{frame}"
                );
                assert!(
                    frame.contains("Filters · All providers") && frame.contains("Sessions · 5/5"),
                    "draft must not commit before Apply: {frame}"
                );
                let row = frame
                    .lines()
                    .position(|line| {
                        line.contains("Apply") && line.contains("Cancel") && !line.contains("Enter")
                    })
                    .unwrap();
                sender
                    .send(TerminalEvent::FullscreenMouse(FullscreenMouseEvent::new(
                        MouseEventKind::Down(MouseButton::Left),
                        frame
                            .lines()
                            .nth(row)
                            .unwrap()
                            .chars()
                            .position(|character| character == 'A')
                            .unwrap() as u16,
                        row as u16,
                    )))
                    .unwrap();
                phase = 3;
            } else if phase == 3
                && frame.contains("Filters · Cursor")
                && frame.contains("Sessions · 1/5")
                && !frame.contains("Session client provider")
            {
                sender.send(ctrl_key('c')).unwrap();
                phase = 4;
            }
            final_frame = frame;
        }
        (phase, final_frame)
    })
    .await
    .expect("provider pointer journey must finish within bounded event wait");
    assert_eq!(phase, 4, "real pointer journey must apply: {final_frame}");
    assert!(final_frame.contains("Filters · Cursor") && final_frame.contains("Sessions · 1/5"));
    assert!(!final_frame.contains("Native Alpha"));
}

#[tokio::test]
async fn sessions_picker_ctrl_f_and_f3_open_provider_filter_chooser() {
    for opening_key in [
        ctrl_key('f'),
        TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::F(3))),
    ] {
        let frames = element! {
            SessionsPickerComponent(request: picker_request(), width: 100usize, height: 36usize)
        }
        .mock_terminal_render_loop(MockTerminalConfig::with_events(futures_util::stream::iter(
            vec![opening_key, ctrl_key('d')],
        )))
        .map(|canvas| canvas.to_string())
        .collect::<Vec<_>>()
        .await;
        for expected in [
            "Filters",
            "All providers",
            "Session client provider",
            "Draft: All providers",
            "Codex",
            "Claude Code",
            "Cursor",
            "Apply",
            "Cancel",
        ] {
            assert!(
                frames.iter().any(|frame| frame.contains(expected)),
                "provider filter opening must expose {expected:?} in the real component: {frames:?}"
            );
        }
    }
}

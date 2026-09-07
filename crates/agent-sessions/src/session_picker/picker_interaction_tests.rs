use super::*;

#[tokio::test]
async fn sessions_picker_iocraft_mock_terminal_handles_keys() {
    let mut selected_outcome = Option::<SessionsPickerOutcome>::None;
    let actual = element! {
        SessionsPickerComponent(
            request: picker_request(),
            width: 100usize,
            selected_outcome_out: &mut selected_outcome,
        )
    }
    .mock_terminal_render_loop(MockTerminalConfig::with_events(futures_util::stream::iter(
        vec![
            ctrl_key('s'),
            ctrl_key('s'),
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Down)),
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Enter)),
        ],
    )))
    .map(|canvas| canvas.to_string())
    .collect::<Vec<_>>()
    .await;

    assert!(
        actual
            .last()
            .is_some_and(|snapshot| snapshot.contains("Provider migration")),
        "picker should render the selected row before exiting: {actual:?}"
    );
    assert_eq!(
        selected_outcome,
        Some(SessionsPickerOutcome::ResumeSession("thread-b".to_owned()))
    );
}

#[tokio::test]
async fn sessions_picker_option_enter_forks_the_focused_existing_session() {
    let mut selected_outcome = Option::<SessionsPickerOutcome>::None;
    let _frames = element! {
        SessionsPickerComponent(
            request: picker_request(),
            width: 100usize,
            selected_outcome_out: &mut selected_outcome,
        )
    }
    .mock_terminal_render_loop(MockTerminalConfig::with_events(futures_util::stream::iter(
        vec![alt_enter_key()],
    )))
    .map(|canvas| canvas.to_string())
    .collect::<Vec<_>>()
    .await;

    assert_eq!(
        selected_outcome,
        Some(SessionsPickerOutcome::ForkSession("thread-a".to_owned()))
    );
}

#[tokio::test]
async fn sessions_picker_existing_row_pointer_focus_updates_conversation_without_activation() {
    let mut selected_outcome = Option::<SessionsPickerOutcome>::None;
    let frames = element! {
        SessionsPickerComponent(
            request: capture_picker_request(),
            width: 160usize,
            height: 24usize,
            selected_outcome_out: &mut selected_outcome,
        )
    }
    .mock_terminal_render_loop(MockTerminalConfig::with_events(futures_util::stream::iter(
        vec![
            TerminalEvent::FullscreenMouse(FullscreenMouseEvent::new(
                MouseEventKind::Moved,
                10,
                14,
            )),
            TerminalEvent::FullscreenMouse(FullscreenMouseEvent::new(
                MouseEventKind::Down(MouseButton::Left),
                10,
                14,
            )),
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Esc)),
        ],
    )))
    .map(|canvas| canvas.to_string())
    .collect::<Vec<_>>()
    .await;

    assert_eq!(selected_outcome, None);
    assert!(
        frames.iter().any(|frame| {
            frame.contains("❯ Provider migration")
                && frame
                    .contains("Provider migration with very very long provider metadata first real")
        }),
        "pointer focus should update the existing-session conversation without activation: {frames:?}"
    );
}

#[tokio::test]
async fn sessions_picker_start_new_hover_focuses_preview_without_activation() {
    let mut selected_outcome = Option::<SessionsPickerOutcome>::None;
    let frames = element! {
        SessionsPickerComponent(
            request: capture_picker_request(),
            width: 160usize,
            height: 24usize,
            selected_outcome_out: &mut selected_outcome,
        )
    }
    .mock_terminal_render_loop(MockTerminalConfig::with_events(futures_util::stream::iter(
        vec![
            TerminalEvent::FullscreenMouse(FullscreenMouseEvent::new(MouseEventKind::Moved, 10, 7)),
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Esc)),
        ],
    )))
    .map(|canvas| canvas.to_string())
    .collect::<Vec<_>>()
    .await;

    assert_eq!(selected_outcome, None);
    assert!(
        frames.iter().any(|frame| {
            frame.contains("❯ Start new session") && frame.contains("no extra args")
        }),
        "Start New hover should focus its preview without activation: {frames:?}"
    );
}

#[tokio::test]
async fn sessions_picker_enter_resumes_pointer_focused_existing_session() {
    let mut selected_outcome = Option::<SessionsPickerOutcome>::None;
    let _frames = element! {
        SessionsPickerComponent(
            request: capture_picker_request(),
            width: 160usize,
            height: 24usize,
            selected_outcome_out: &mut selected_outcome,
        )
    }
    .mock_terminal_render_loop(MockTerminalConfig::with_events(futures_util::stream::iter(
        vec![
            TerminalEvent::FullscreenMouse(FullscreenMouseEvent::new(
                MouseEventKind::Down(MouseButton::Left),
                10,
                14,
            )),
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Enter)),
        ],
    )))
    .collect::<Vec<_>>()
    .await;

    assert_eq!(
        selected_outcome,
        Some(SessionsPickerOutcome::ResumeSession("thread-b".to_owned()))
    );
}

#[tokio::test]
async fn sessions_picker_start_new_click_activates_immediately() {
    let mut selected_outcome = Option::<SessionsPickerOutcome>::None;
    let _frames = element! {
        SessionsPickerComponent(
            request: capture_picker_request(),
            width: 160usize,
            height: 24usize,
            selected_outcome_out: &mut selected_outcome,
        )
    }
    .mock_terminal_render_loop(MockTerminalConfig::with_events(futures_util::stream::iter(
        vec![TerminalEvent::FullscreenMouse(FullscreenMouseEvent::new(
            MouseEventKind::Down(MouseButton::Left),
            10,
            7,
        ))],
    )))
    .collect::<Vec<_>>()
    .await;

    assert_eq!(
        selected_outcome,
        Some(SessionsPickerOutcome::StartNewSession)
    );
}

#[tokio::test]
async fn sessions_picker_non_left_row_events_do_not_focus_or_activate() {
    let mut selected_outcome = Option::<SessionsPickerOutcome>::None;
    let _frames = element! {
        SessionsPickerComponent(
            request: capture_picker_request(),
            width: 160usize,
            height: 24usize,
            selected_outcome_out: &mut selected_outcome,
        )
    }
    .mock_terminal_render_loop(MockTerminalConfig::with_events(futures_util::stream::iter(
        vec![
            TerminalEvent::FullscreenMouse(FullscreenMouseEvent::new(
                MouseEventKind::Down(MouseButton::Right),
                10,
                14,
            )),
            TerminalEvent::FullscreenMouse(FullscreenMouseEvent::new(
                MouseEventKind::Up(MouseButton::Left),
                10,
                14,
            )),
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Enter)),
        ],
    )))
    .collect::<Vec<_>>()
    .await;

    assert_eq!(
        selected_outcome,
        Some(SessionsPickerOutcome::ResumeSession("thread-a".to_owned())),
        "ignored pointer events must leave the initial existing-session focus unchanged"
    );
}

#[tokio::test]
async fn sessions_picker_hover_keeps_scrolled_row_under_pointer_until_click_and_enter() {
    let mut selected_outcome = Option::<SessionsPickerOutcome>::None;
    let events = futures_util::stream::iter(vec![
        TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::End)),
        TerminalEvent::FullscreenMouse(FullscreenMouseEvent::new(MouseEventKind::Moved, 10, 8)),
        TerminalEvent::FullscreenMouse(FullscreenMouseEvent::new(
            MouseEventKind::Down(MouseButton::Left),
            10,
            8,
        )),
        TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Enter)),
    ])
    .then(|event| async move {
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        event
    });
    let frames = element! {
        SessionsPickerComponent(
            request: capture_picker_request(),
            width: 160usize,
            height: 24usize,
            selected_outcome_out: &mut selected_outcome,
        )
    }
    .mock_terminal_render_loop(MockTerminalConfig::with_events(events))
    .map(|canvas| canvas.to_string())
    .collect::<Vec<_>>()
    .await;

    assert!(
        frames.iter().any(|frame| {
            frame.contains("❯ Follow-up implementation lane 4") && frame.contains("+8 more above")
        }),
        "pointer focus should not rewindow a scrolled row before mouse-down: {frames:?}"
    );
    assert_eq!(
        selected_outcome,
        Some(SessionsPickerOutcome::ResumeSession(
            "thread-extra-4".to_owned()
        )),
        "Enter must resume the stable session that remained under the pointer"
    );
}

#[tokio::test]
async fn sessions_picker_iocraft_mock_terminal_ctrl_shortcuts_drive_filters() {
    let actual = element! {
        SessionsPickerComponent(
            request: picker_request(),
            width: 100usize,
        )
    }
    .mock_terminal_render_loop(MockTerminalConfig::with_events(futures_util::stream::iter(
        vec![
            ctrl_key('s'),
            ctrl_key('t'),
            ctrl_key('o'),
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Esc)),
        ],
    )))
    .map(|canvas| canvas.to_string())
    .collect::<Vec<_>>()
    .await;

    assert!(
        actual
            .iter()
            .any(|snapshot| snapshot.contains("[repo]    View: [Blocked]    Sort: [created]")),
        "ctrl shortcuts should cycle scope, threads, and sort: {actual:?}"
    );
}

#[tokio::test]
async fn sessions_picker_help_toggles_and_escape_closes_help_before_picker() {
    for help_key in [
        ctrl_key('/'),
        ctrl_key('_'),
        ctrl_key('7'),
        TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::F(1))),
    ] {
        let events =
            futures_util::stream::iter(vec![help_key]).chain(futures_util::stream::pending());
        let mut picker = element! {
            SessionsPickerComponent(request: picker_request(), width: 100usize)
        };
        let frames = picker.mock_terminal_render_loop(MockTerminalConfig::with_events(events));
        tokio::pin!(frames);
        tokio::time::timeout(Duration::from_secs(2), async {
            while let Some(canvas) = frames.next().await {
                if canvas.to_string().contains("ctrl-r refresh") {
                    return;
                }
            }
            panic!("picker ended before showing help");
        })
        .await
        .expect("help must render within the deadline");
    }

    // If Esc exits instead of closing help, Ctrl+N cannot produce this outcome.
    let mut selected_outcome = None;
    let mut picker = element! {
        SessionsPickerComponent(request: picker_request(), width: 100usize, selected_outcome_out: &mut selected_outcome)
    };
    let frames = picker.mock_terminal_render_loop(MockTerminalConfig::with_events(
        futures_util::stream::iter(vec![
            ctrl_key('/'),
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Esc)),
            ctrl_key('n'),
        ]),
    ));
    let actual = tokio::time::timeout(Duration::from_secs(2), frames.collect::<Vec<_>>())
        .await
        .expect("closing help then starting new must finish");
    drop(picker);
    assert_eq!(
        selected_outcome,
        Some(SessionsPickerOutcome::StartNewSession)
    );
    assert!(
        actual
            .last()
            .is_some_and(|canvas| canvas.to_string().contains("ctrl-/ Help"))
    );
}

#[tokio::test]
async fn sessions_picker_iocraft_mock_terminal_ctrl_n_starts_new_thread() {
    let mut selected_outcome = None;
    let _actual = element! {
        SessionsPickerComponent(
            request: picker_request(),
            width: 100usize,
            selected_outcome_out: &mut selected_outcome,
        )
    }
    .mock_terminal_render_loop(MockTerminalConfig::with_events(futures_util::stream::iter(
        vec![ctrl_key('n')],
    )))
    .collect::<Vec<_>>()
    .await;

    assert_eq!(
        selected_outcome,
        Some(SessionsPickerOutcome::StartNewSession)
    );
}

#[tokio::test]
async fn sessions_picker_iocraft_mock_terminal_esc_clears_search_before_exit() {
    let mut selected_outcome: Option<SessionsPickerOutcome> = None;
    let actual = element! {
        SessionsPickerComponent(
            request: picker_request(),
            width: 100usize,
            selected_outcome_out: &mut selected_outcome,
        )
    }
    .mock_terminal_render_loop(MockTerminalConfig::with_events(futures_util::stream::iter(
        vec![
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Char('r'))),
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Char('u'))),
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Char('s'))),
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Char('t'))),
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Esc)),
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Esc)),
        ],
    )))
    .map(|canvas| canvas.to_string())
    .collect::<Vec<_>>()
    .await;

    assert!(
        actual
            .iter()
            .any(|snapshot| snapshot.contains("Search: []")),
        "first escape should clear search instead of exiting immediately: {actual:?}"
    );
    assert_eq!(selected_outcome, None);
}

#[tokio::test]
async fn sessions_picker_iocraft_mock_terminal_ctrl_c_and_ctrl_d_exit() {
    for key in ['c', 'd'] {
        let mut selected_outcome: Option<SessionsPickerOutcome> = None;
        let actual = element! {
            SessionsPickerComponent(
                request: picker_request(),
                width: 100usize,
                selected_outcome_out: &mut selected_outcome,
            )
        }
        .mock_terminal_render_loop(MockTerminalConfig::with_events(futures_util::stream::iter(
            vec![ctrl_key(key)],
        )))
        .collect::<Vec<_>>()
        .await;

        assert!(!actual.is_empty(), "ctrl-{key} should render before exit");
        assert_eq!(selected_outcome, None, "ctrl-{key} should cancel picker");
    }
}

#[tokio::test]
async fn sessions_picker_iocraft_mock_terminal_search_keeps_plain_letters() {
    let actual = element! {
        SessionsPickerComponent(
            request: picker_request(),
            width: 100usize,
        )
    }
    .mock_terminal_render_loop(MockTerminalConfig::with_events(futures_util::stream::iter(
        vec![
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Char('r'))),
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Char('u'))),
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Char('s'))),
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Char('t'))),
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Esc)),
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Esc)),
        ],
    )))
    .map(|canvas| canvas.to_string())
    .collect::<Vec<_>>()
    .await;

    assert!(
        actual
            .iter()
            .any(|snapshot| snapshot.contains("[📂 cwd]    View: [All]")),
        "plain search input should leave filters unchanged: {actual:?}"
    );
}

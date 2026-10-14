use super::*;
use crate::presentation::session_picker::test_support::picker_action_selection;

#[tokio::test]
async fn provider_selection_shows_read_only_details_without_a_codex_outcome() {
    let summary: collaboration_client::protocol::ProviderSessionSummary =
        serde_json::from_value(serde_json::json!({
            "origin":"hostedProvider",
            "target":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"claude-local"},"sessionId":"provider-one"},
            "workingDirectory":"/repo/project-a","updatedAt":3,"state":"requiresAction",
            "approver":{"kind":"human","humanId":"owner"},
            "createdBy":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"sessionId":"creator"}
        }))
        .expect("provider summary");
    let mut request = picker_request();
    request.records = vec![SessionPickerRecord::from_provider_summary(
        &summary,
        "Claude fixture",
    )];
    let mut selected_outcome = Option::<SessionsPickerOutcome>::None;
    {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        let events = futures_util::stream::unfold(receiver, |mut receiver| async {
            receiver.recv().await.map(|event| (event, receiver))
        });
        let mut ui = element!(SessionsPickerComponent(request,width:100usize,selected_outcome_out:&mut selected_outcome));
        let frames = ui.mock_terminal_render_loop(MockTerminalConfig::with_events(events));
        tokio::pin!(frames);
        let mut phase = 0;
        let mut last = String::new();
        let journey = async {
            while let Some(canvas) = frames.next().await {
                let frame = canvas.to_string();
                if phase == 0 && frame.contains("Claude fixture") {
                    sender
                        .send(TerminalEvent::Key(KeyEvent::new(
                            KeyEventKind::Press,
                            KeyCode::F(4),
                        )))
                        .unwrap();
                    phase = 1;
                } else if phase == 1
                    && frame.contains("Session details")
                    && frame.contains("Read-only")
                {
                    assert!(
                        frame.contains("Claude fixture"),
                        "dialog must identify its provider session: {frame}"
                    );
                    sender.send(alt_enter_key()).unwrap();
                    sender
                        .send(TerminalEvent::Key(KeyEvent::new(
                            KeyEventKind::Press,
                            KeyCode::Enter,
                        )))
                        .unwrap();
                    sender
                        .send(TerminalEvent::Key(KeyEvent::new(
                            KeyEventKind::Press,
                            KeyCode::Esc,
                        )))
                        .unwrap();
                    phase = 2;
                } else if phase == 2 && !frame.contains("Session details") {
                    sender.send(ctrl_key('c')).unwrap();
                    phase = 3;
                }
                last = frame;
            }
        };
        let result = tokio::time::timeout(Duration::from_secs(2), journey).await;
        assert!(result.is_ok(), "provider details phase={phase}: {last}");
        assert_eq!(phase, 3, "{last}");
    }
    assert_eq!(
        selected_outcome, None,
        "provider details keys cannot create a Codex outcome"
    );
}

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
        Some(SessionsPickerOutcome::ResumeSession(
            picker_action_selection("thread-b")
        ))
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
        vec![
            alt_enter_key(),
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Enter)),
        ],
    )))
    .map(|canvas| canvas.to_string())
    .collect::<Vec<_>>()
    .await;

    assert_eq!(
        selected_outcome,
        Some(SessionsPickerOutcome::ForkSession(picker_action_selection(
            "thread-a"
        )))
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
            TerminalEvent::FullscreenMouse(FullscreenMouseEvent::new(MouseEventKind::Moved, 10, 8)),
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Esc)),
        ],
    )))
    .map(|canvas| canvas.to_string())
    .collect::<Vec<_>>()
    .await;

    assert_eq!(selected_outcome, None);
    assert!(
        frames.iter().any(|frame| {
            frame.contains("❯ New Codex session") && frame.contains("no extra args")
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
        Some(SessionsPickerOutcome::ResumeSession(
            picker_action_selection("thread-b")
        ))
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
            8,
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
        Some(SessionsPickerOutcome::ResumeSession(
            picker_action_selection("thread-a")
        )),
        "ignored pointer events must leave the initial existing-session focus unchanged"
    );
}

#[tokio::test]
async fn sessions_picker_hover_keeps_scrolled_row_under_pointer_until_click_and_enter() {
    let mut selected_outcome = Option::<SessionsPickerOutcome>::None;
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    let events = futures_util::stream::unfold(receiver, |mut receiver| async {
        receiver.recv().await.map(|event| (event, receiver))
    });
    let mut phase = 0;
    let mut pointer_row = 0;
    {
        let mut picker = element! { SessionsPickerComponent(request: capture_picker_request(), width: 160usize, height: 24usize, selected_outcome_out: &mut selected_outcome) };
        let frames = picker.mock_terminal_render_loop(MockTerminalConfig::with_events(events));
        tokio::pin!(frames);
        tokio::time::timeout(Duration::from_secs(2), async {
            while let Some(canvas) = frames.next().await {
                let frame = canvas.to_string();
                match phase {
                    0 => {
                        sender
                            .send(TerminalEvent::Key(KeyEvent::new(
                                KeyEventKind::Press,
                                KeyCode::End,
                            )))
                            .unwrap();
                        phase = 1;
                    }
                    1 if frame.contains("+8 more above") => {
                        pointer_row = frame
                            .lines()
                            .position(|line| line.contains("Follow-up implementation lane 5"))
                            .expect("literal row must be visible after End")
                            as u16;
                        sender
                            .send(TerminalEvent::FullscreenMouse(FullscreenMouseEvent::new(
                                MouseEventKind::Moved,
                                10,
                                pointer_row,
                            )))
                            .unwrap();
                        phase = 2;
                    }
                    2 if frame.contains("❯ Follow-up implementation lane 5") => {
                        assert!(
                            frame
                                .lines()
                                .nth(pointer_row as usize)
                                .unwrap()
                                .contains("❯ Follow-up implementation lane 5"),
                            "hover rewindowed the row: {frame}"
                        );
                        sender
                            .send(TerminalEvent::FullscreenMouse(FullscreenMouseEvent::new(
                                MouseEventKind::Down(MouseButton::Left),
                                10,
                                pointer_row,
                            )))
                            .unwrap();
                        sender
                            .send(TerminalEvent::Key(KeyEvent::new(
                                KeyEventKind::Press,
                                KeyCode::Enter,
                            )))
                            .unwrap();
                        phase = 3;
                    }
                    _ => {}
                }
            }
        })
        .await
        .expect("real pointer journey must complete within the event bound");
    }
    assert_eq!(phase, 3);
    assert_eq!(
        selected_outcome,
        Some(SessionsPickerOutcome::ResumeSession(
            picker_action_selection("thread-extra-5")
        )),
        "Enter must resume the literal row that stayed under the pointer"
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
        actual.iter().any(
            |snapshot| snapshot.lines().any(|line| line.contains("[repo]")
                && line.contains("View: [Blocked]")
                && line.contains("Sort: [created]"))
        ),
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
                if canvas.to_string().contains("Sessions Help") {
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
            .is_some_and(|canvas| canvas.to_string().contains("Help"))
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
            .last()
            .is_some_and(|snapshot| snapshot.contains("Feature design session")
                && !snapshot.contains("Search: [rust]")),
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
        actual.iter().any(|snapshot| snapshot.contains("[📂 cwd]")
            && !snapshot.contains("View: [Blocked]")
            && !snapshot.contains("View: [Active]")
            && !snapshot.contains("View: [Idle]")
            && !snapshot.contains("Sort: [created]")),
        "plain search input should leave filters unchanged: {actual:?}"
    );
}

#[tokio::test]
async fn picker_control_g_opens_a_visible_machine_selector_without_selecting_an_action() {
    let mut selected_outcome = Option::<SessionsPickerOutcome>::None;
    let (send_event, receive_event) = tokio::sync::mpsc::unbounded_channel();
    send_event.send(ctrl_key('g')).unwrap();
    let events = futures_util::stream::unfold(receive_event, |mut receiver| async {
        receiver.recv().await.map(|event| (event, receiver))
    });
    let frames = tokio::time::timeout(Duration::from_secs(2), async {
        let mut picker = element! {
            SessionsPickerComponent(request: picker_request(), width: 100usize, height: 40usize,
                selected_outcome_out: &mut selected_outcome)
        };
        let canvases = picker.mock_terminal_render_loop(MockTerminalConfig::with_events(events));
        tokio::pin!(canvases);
        let mut frames = Vec::new();
        let mut opened = false;
        while let Some(canvas) = canvases.next().await {
            let text = canvas.to_string();
            if !opened && text.contains("Choose machine") {
                opened = true;
                send_event.send(ctrl_key('c')).unwrap();
            }
            frames.push(text);
        }
        frames
    })
    .await
    .expect("machine chooser must render and cancel within bounded event wait");

    assert!(
        frames.iter().any(|text| text.contains("Choose machine")),
        "{frames:?}"
    );
    assert_eq!(
        selected_outcome, None,
        "opening/canceling the selector never launches"
    );
}

#[tokio::test]
async fn alt_enter_opens_fork_confirmation_before_any_action_is_selected() {
    let mut outcome = Option::<SessionsPickerOutcome>::None;
    let (send_event, receive_event) = tokio::sync::mpsc::unbounded_channel();
    send_event.send(alt_enter_key()).unwrap();
    let events = futures_util::stream::unfold(receive_event, |mut receiver| async {
        receiver.recv().await.map(|event| (event, receiver))
    });
    let frames = tokio::time::timeout(Duration::from_secs(2), async {
        let mut picker = element! { SessionsPickerComponent(request: picker_request(), width: 100usize,
            height: 40usize, selected_outcome_out: &mut outcome) };
        let canvases = picker.mock_terminal_render_loop(MockTerminalConfig::with_events(events));
        tokio::pin!(canvases);
        let mut frames = Vec::new();
        let mut opened = false;
        while let Some(canvas) = canvases.next().await {
            let text = canvas.to_string();
            if !opened && text.contains("Fork session") {
                opened = true;
                send_event.send(ctrl_key('c')).unwrap();
            }
            frames.push(text);
        }
        frames
    }).await.unwrap();
    assert!(frames.iter().any(|frame| frame.contains("Fork session")));
    assert_eq!(
        outcome, None,
        "opening/canceling fork confirmation creates no fork"
    );
}

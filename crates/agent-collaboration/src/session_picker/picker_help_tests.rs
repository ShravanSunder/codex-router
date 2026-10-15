//! Real component claims for Help chrome and retained underlying modal state.
use super::*;

fn key(code: KeyCode) -> TerminalEvent {
    TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, code))
}

#[tokio::test]
async fn sessions_help_top_line_hides_shortcuts_outside_help() {
    let frame = render_picker_capture_at(picker_request(), 120, 36, vec![ctrl_key('d')]).await;
    let title = frame
        .lines()
        .find(|line| line.contains("Sessions ·"))
        .unwrap();
    assert!(title.contains("Help"), "top line must expose Help: {frame}");
    for shortcut in [
        "F1",
        "F2",
        "F3",
        "F4",
        "ctrl-",
        "Ctrl+",
        "enter apply",
        "esc cancel",
    ] {
        assert!(
            !frame.contains(shortcut),
            "shortcut {shortcut} leaked: {frame}"
        );
    }
}

#[tokio::test]
async fn sessions_help_phone_shows_filter_context_before_general_navigation() {
    let frame = render_picker_capture_at(
        picker_request(),
        48,
        24,
        vec![
            ctrl_key('f'),
            key(KeyCode::Down),
            key(KeyCode::F(1)),
            ctrl_key('d'),
        ],
    )
    .await;
    assert!(frame.contains("Sessions Help"), "{frame}");
    assert!(
        frame.contains("Session filters"),
        "current dialog context must be visible: {frame}"
    );
    assert!(frame.contains("Choose provider draft"), "{frame}");
}

#[tokio::test]
async fn sessions_help_preserves_provider_draft_and_blocks_background_keys() {
    let frame = render_picker_capture_at(
        picker_request(),
        120,
        36,
        vec![
            ctrl_key('f'),
            key(KeyCode::Down),
            key(KeyCode::F(1)),
            key(KeyCode::Down),
            key(KeyCode::Enter),
            key(KeyCode::Char('x')),
            key(KeyCode::Esc),
            ctrl_key('d'),
        ],
    )
    .await;
    assert!(
        frame.contains("Draft: Codex"),
        "Help close must restore draft: {frame}"
    );
    assert!(
        frame.contains("Current: All providers"),
        "Help must not apply draft: {frame}"
    );
    assert!(
        !frame.contains("Search: [x]"),
        "background search leaked: {frame}"
    );
}

#[tokio::test]
async fn sessions_help_precedes_existing_machine_fork_and_details_input() {
    for (opening, restored) in [
        (ctrl_key('g'), "Choose machine"),
        (alt_enter_key(), "Fork session"),
        (key(KeyCode::F(4)), "Session details"),
    ] {
        let frame = render_picker_capture_at(
            picker_request(),
            120,
            36,
            vec![
                opening,
                key(KeyCode::F(1)),
                key(KeyCode::Enter),
                ctrl_key('f'),
                key(KeyCode::F(1)),
                ctrl_key('d'),
            ],
        )
        .await;
        assert!(
            frame.contains(restored),
            "Help must restore {restored}: {frame}"
        );
        assert!(
            !frame.contains("Session filters"),
            "Help leaked filter shortcut: {frame}"
        );
    }
}

#[tokio::test]
async fn sessions_help_click_opens_overlay_with_surface_and_section_styles() {
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    let events = futures_util::stream::unfold(receiver, |mut receiver| async {
        receiver.recv().await.map(|event| (event, receiver))
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        let mut picker = element! { SessionsPickerComponent(request: picker_request(), width: 120usize, height: 36usize) };
        let frames = picker.mock_terminal_render_loop(MockTerminalConfig::with_events(events));
        tokio::pin!(frames);
        let mut opened = false;
        while let Some(canvas) = frames.next().await {
            let text = canvas.to_string();
            if !opened {
                let (row, line) = text.lines().enumerate().find(|(_, line)| line.contains("Sessions ·") && line.contains("Help")).unwrap();
                let column = line.chars().position(|character| character == 'H').unwrap();
                let title_column = line.chars().position(|character| character == 'S').unwrap();
                let title_style = canvas.cell(title_column, row).unwrap().text_style().unwrap();
                assert_eq!(title_style.color, Some(Color::Cyan));
                assert_eq!(title_style.weight, Weight::Bold);
                let help_style = canvas.cell(column, row).unwrap().text_style().unwrap();
                assert_eq!(help_style.color, Some(Color::Grey), "Help is secondary, not a second heading");
                assert_eq!(help_style.weight, Weight::Normal);
                sender.send(TerminalEvent::FullscreenMouse(FullscreenMouseEvent::new(MouseEventKind::Down(MouseButton::Left), column as u16, row as u16))).unwrap();
                opened = true;
            } else if text.contains("Sessions Help") {
                let (row, line) = text.lines().enumerate().find(|(_, line)| line.contains("Sessions") && !line.contains("Sessions Help") && !line.contains("Sessions ·")).unwrap_or_else(|| panic!("actual Help section must be painted: {text}"));
                let column = line.chars().position(|character| character == 'S').unwrap();
                let section_style = canvas.cell(column, row).unwrap().text_style().unwrap();
                assert_eq!(section_style.color, Some(Color::White));
                assert_eq!(section_style.weight, Weight::Bold);
                sender.send(ctrl_key('c')).unwrap();
                return;
            }
        }
        panic!("Help click must reach actual overlay");
    }).await.expect("Help click must finish within bounded event wait");
}

#[tokio::test]
async fn owner_feedback_help_page_keys_fit_one_row() {
    for (width, height) in [(48, 24), (76, 30), (120, 36)] {
        let frame = render_picker_capture_at(
            picker_request(),
            width,
            height,
            vec![key(KeyCode::F(1)), ctrl_key('d')],
        )
        .await;
        assert!(
            frame
                .lines()
                .any(|line| line.contains("PgUp / PgDn") && line.contains("Move a page")),
            "page keys must stay together at {width}: {frame}"
        );
    }
}

#[tokio::test]
async fn owner_feedback_atomic_paste_searches_full_session_id_without_executing_controls() {
    const SESSION_ID: &str = "00000000-0000-4000-8000-000000000042";
    for pasted in [
        SESSION_ID.to_owned(),
        format!("\u{3}\u{1b}{SESSION_ID}\n\r\u{e}"),
    ] {
        let mut request = picker_request();
        request.root = SessionsRoot::Any;
        request.source = SessionsSource::All;
        request.records[0].session_id = SESSION_ID.to_owned();
        request.records[0].identity =
            crate::sessions::SessionPickerIdentity::LocalCodex(SESSION_ID.to_owned());
        let mut outcome: Option<SessionsPickerOutcome> = None;
        let canvases = element! {
            SessionsPickerComponent(request, width: 120usize, height: 36usize, selected_outcome_out: &mut outcome)
        }.mock_terminal_render_loop(MockTerminalConfig::with_events(futures_util::stream::iter(vec![
            TerminalEvent::BracketedPaste(pasted), ctrl_key('d'),
        ]))).collect::<Vec<_>>().await;
        let frame = canvases.last().unwrap().to_string();
        assert!(
            frame.contains(&format!("Search: [{SESSION_ID}]")),
            "paste must reach search atomically: {frame}"
        );
        assert!(frame.contains("Feature design session"), "{frame}");
        assert!(
            !frame.contains("Provider migration") && !frame.contains("Subagent planning"),
            "full ID must match only its own row: {frame}"
        );
        assert!(
            outcome.is_none(),
            "pasted control bytes cannot activate a session or start a new one"
        );
    }
}

#[tokio::test]
async fn owner_feedback_modal_input_blocks_atomic_paste() {
    for opening in [
        key(KeyCode::F(1)),
        ctrl_key('f'),
        ctrl_key('g'),
        alt_enter_key(),
        key(KeyCode::F(4)),
    ] {
        let frame = render_picker_capture_at(
            picker_request(),
            120,
            36,
            vec![
                opening,
                TerminalEvent::BracketedPaste("thread-b".to_owned()),
                key(KeyCode::Esc),
                ctrl_key('d'),
            ],
        )
        .await;
        assert!(
            !frame.contains("Search: [thread-b]"),
            "modal paste leaked into the background query: {frame}"
        );
        assert!(
            frame.contains("Feature design session"),
            "modal close must preserve the browse query: {frame}"
        );
    }
}

#[tokio::test]
async fn owner_feedback_session_metadata_keeps_readable_secondary_contrast() {
    let mut request = picker_request();
    request.root = SessionsRoot::Any;
    request.source = SessionsSource::All;
    let canvases = element!(SessionsPickerComponent(request, width: 120usize, height: 36usize))
        .mock_terminal_render_loop(MockTerminalConfig::with_events(futures_util::stream::iter(
            vec![ctrl_key('d')],
        )))
        .collect::<Vec<_>>()
        .await;
    let canvas = canvases.last().unwrap();
    let frame = canvas.to_string();
    let mut metadata_rows = 0;
    for (row, line) in frame
        .lines()
        .enumerate()
        .filter(|(_, line)| line.contains("Codex This machine"))
    {
        let offset = line.find("Codex").unwrap();
        let column = line
            .get(..offset)
            .expect("matched label is a UTF-8 boundary")
            .chars()
            .count();
        let style = canvas.cell(column, row).unwrap().text_style().unwrap();
        assert_eq!(
            style.color,
            Some(Color::Grey),
            "supporting text must not use the disabled role: {frame}"
        );
        assert_eq!(style.weight, Weight::Normal);
        metadata_rows += 1;
    }
    assert!(
        metadata_rows >= 2,
        "exercise both focused and unfocused metadata: {frame}"
    );
}

#[tokio::test]
async fn owner_feedback_atomic_paste_advances_the_real_source_query_once() {
    const SESSION_ID: &str = "00000000-0000-4000-8000-000000000042";
    let (started_sender, mut started_receiver) = tokio::sync::mpsc::unbounded_channel();
    let loader: SessionsPickerRecordLoader = Arc::new(move |request| {
        let started_sender = started_sender.clone();
        Box::pin(async move {
            started_sender
                .send((request.request_generation, request.query.search.clone()))
                .unwrap();
            if request.query.search.is_empty() {
                // Changing the query must supersede this real pending source read.
                futures_util::future::pending::<()>().await;
            }
            crate::presentation::session_picker::SourceInventoryResult::Ready {
                request,
                binding: None,
                progress: crate::presentation::session_picker::InventoryPageProgress::Complete,
                snapshot: observed_records(vec![picker_record(
                    SESSION_ID,
                    "Paste source result",
                    "/repo/project-a",
                    "codex-router",
                    "cli",
                )]),
            }
        })
    });
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    let events = futures_util::stream::unfold(receiver, |mut receiver| async {
        receiver.recv().await.map(|event| (event, receiver))
    });
    let mut request = picker_request();
    request.root = SessionsRoot::Any;
    request.source = SessionsSource::All;
    let mut picker = element!(SessionsPickerComponent(request, record_loader: Some(loader), width: 120usize, height: 36usize));
    let frames = picker.mock_terminal_render_loop(MockTerminalConfig::with_events(events));
    tokio::pin!(frames);
    let mut queries = Vec::new();
    let mut pasted = false;
    let mut observed = false;
    let mut exit_sent = false;
    let mut last_frame = String::new();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            tokio::select! {
                Some(query) = started_receiver.recv() => {
                    if query.1.is_empty() && !pasted {
                        pasted = true;
                        sender.send(TerminalEvent::BracketedPaste(SESSION_ID.to_owned())).unwrap();
                    }
                    queries.push(query);
                }
                frame = frames.next() => {
                    let Some(canvas) = frame else { break; };
                    last_frame = canvas.to_string();
                    if !observed && last_frame.contains("Paste source result") && last_frame.contains(SESSION_ID) {
                        observed = true;
                    }
                }
            }
            if observed && !exit_sent && queries.iter().any(|(_, search)| !search.is_empty()) {
                exit_sent = true;
                sender.send(ctrl_key('d')).unwrap();
            }
        }
    }).await.unwrap_or_else(|_| panic!("paste must supersede the pending read and render its own result: {last_frame}"));
    assert!(observed, "{last_frame}");
    let pasted_queries = queries
        .into_iter()
        .filter(|(_, search)| !search.is_empty())
        .collect::<Vec<_>>();
    assert_eq!(
        pasted_queries,
        vec![(1, SESSION_ID.to_owned())],
        "one atomic paste must create one source generation, not one per character"
    );
}

#[tokio::test]
async fn sessions_help_hierarchy_preserves_selected_and_focused_action_styles() {
    let canvases = element! { SessionsPickerComponent(request: picker_request(), width: 120usize, height: 36usize) }
        .mock_terminal_render_loop(MockTerminalConfig::with_events(futures_util::stream::iter(vec![
            ctrl_key('f'), key(KeyCode::Tab), ctrl_key('d'),
        ]))).collect::<Vec<_>>().await;
    let canvas = canvases.last().unwrap();
    let text = canvas.to_string();
    let (row, line) = text
        .lines()
        .enumerate()
        .find(|(_, line)| line.contains("Apply") && line.contains("Cancel"))
        .unwrap();
    let column = line.chars().position(|character| character == 'A').unwrap();
    let style = canvas.cell(column, row).unwrap().text_style().unwrap();
    assert_eq!(style.color, Some(Color::Yellow));
    assert_eq!(style.weight, Weight::Bold);
    assert!(
        style.underline,
        "focused Apply must differ from selected radio: {text}"
    );
    let (row, line) = text
        .lines()
        .enumerate()
        .find(|(_, line)| line.contains("Session client provider"))
        .unwrap();
    let column = line.chars().position(|character| character == 'S').unwrap();
    let style = canvas.cell(column, row).unwrap().text_style().unwrap();
    assert_eq!(style.color, Some(Color::White));
    assert_eq!(style.weight, Weight::Bold);
    let (row, line) = text
        .lines()
        .enumerate()
        .find(|(_, line)| line.contains("○ Codex"))
        .unwrap();
    let offset = line.find("Codex").unwrap();
    let column = line.get(..offset).unwrap().chars().count();
    let style = canvas.cell(column, row).unwrap().text_style().unwrap();
    assert_eq!(
        style.color,
        Some(Color::White),
        "an available choice is body text: {text}"
    );
    let (row, line) = text
        .lines()
        .enumerate()
        .find(|(_, line)| line.contains("● All providers"))
        .unwrap();
    let column = line.chars().position(|character| character == '●').unwrap();
    let style = canvas.cell(column, row).unwrap().text_style().unwrap();
    assert_eq!(
        style.color,
        Some(Color::Yellow),
        "selection survives action focus: {text}"
    );
}

#[tokio::test]
async fn sessions_help_wide_overlay_preserves_background_top_bottom_and_inactive_title() {
    let canvases = element! { SessionsPickerComponent(request: picker_request(), width: 120usize, height: 36usize) }
        .mock_terminal_render_loop(MockTerminalConfig::with_events(futures_util::stream::iter(vec![
            key(KeyCode::F(1)), key(KeyCode::Down), key(KeyCode::Char('x')), ctrl_key('d'),
        ]))).collect::<Vec<_>>().await;
    let canvas = canvases.last().unwrap();
    let text = canvas.to_string();
    let (row, line) = text
        .lines()
        .enumerate()
        .find(|(_, line)| line.contains("Sessions ·"))
        .unwrap();
    assert_eq!(row, 1, "background title must retain top row: {text}");
    assert!(
        line.contains("Help"),
        "background top Help remains mounted: {text}"
    );
    let column = line.chars().position(|character| character == 'S').unwrap();
    let style = canvas.cell(column, row).unwrap().text_style().unwrap();
    assert_eq!(style.color, Some(Color::Grey));
    assert_eq!(style.weight, Weight::Bold);
    assert!(
        text.lines().nth(34).unwrap().contains("Details"),
        "background action must retain bottom row: {text}"
    );
    assert!(
        text.lines().nth(35).unwrap().contains('╰'),
        "background border must retain viewport: {text}"
    );
    assert!(
        text.contains("Sessions Help"),
        "Help is the active dialog: {text}"
    );
    let restored = render_picker_capture_at(
        picker_request(),
        120,
        36,
        vec![
            key(KeyCode::F(1)),
            key(KeyCode::Down),
            key(KeyCode::Char('x')),
            key(KeyCode::Esc),
            ctrl_key('d'),
        ],
    )
    .await;
    assert!(
        restored.contains("❯ Feature design session"),
        "Help must preserve identity focus: {restored}"
    );
    assert!(
        !restored.contains("Search: [x]"),
        "Help must preserve query: {restored}"
    );
}

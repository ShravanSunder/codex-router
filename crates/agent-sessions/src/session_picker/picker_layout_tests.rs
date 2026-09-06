use super::*;

#[tokio::test]
async fn sessions_picker_selected_row_uses_contract_marker_and_metadata() {
    let actual = element! {
        SessionsPickerComponent(
            request: picker_request(),
            width: 100usize,
        )
    }
    .mock_terminal_render_loop(MockTerminalConfig::with_events(futures_util::stream::iter(
        vec![
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Down)),
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Esc)),
        ],
    )))
    .collect::<Vec<_>>()
    .await;
    let Some(canvas) = actual.last() else {
        panic!("picker should render a canvas");
    };
    let snapshot = canvas.to_string();
    assert!(
        snapshot.contains("❯ Feature design session"),
        "selected row should use the contracted focus marker: {canvas}"
    );
    assert!(
        snapshot.contains("⎇ main") && snapshot.contains("/repo/project-a"),
        "selected row should keep branch and cwd on the metadata row: {canvas}"
    );
    assert!(
        snapshot.contains('╭') && snapshot.contains('╰'),
        "picker should render an iocraft bordered panel: {canvas}"
    );
}

#[tokio::test]
async fn sessions_picker_start_new_row_uses_outline_instead_of_filled_background() {
    let mut request = picker_request();
    request.new_session_args_display =
        "--router-root /Users/shravansunder/.codex-router".to_owned();
    let text = render_picker_capture(
        request,
        100,
        vec![
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Up)),
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Esc)),
        ],
    )
    .await;
    let lines = text.lines().collect::<Vec<_>>();
    let start_index = lines
        .iter()
        .position(|line| line.contains("Start new session"))
        .unwrap_or_else(|| panic!("start-new row should render:\n{text}"));

    assert!(
        lines
            .get(start_index.saturating_sub(1))
            .is_some_and(|line| line.contains('┌') && line.contains('┐')),
        "start-new row should have a thin outline top border:\n{text}"
    );
    assert!(
        lines
            .get(start_index + 2)
            .is_some_and(|line| line.contains('└') && line.contains('┘')),
        "start-new row should have a thin outline bottom border:\n{text}"
    );
    assert!(
        !lines[start_index].contains('█'),
        "start-new row should not read as a filled selected row:\n{text}"
    );
}

#[tokio::test]
async fn sessions_picker_iocraft_mock_terminal_too_narrow_exits_without_selection() {
    let mut selected_outcome = None;
    let actual = element! {
        SessionsPickerComponent(
            request: picker_request(),
            width: 20usize,
            selected_outcome_out: &mut selected_outcome,
        )
    }
    .mock_terminal_render_loop(MockTerminalConfig::with_events(futures_util::stream::iter(
        vec![TerminalEvent::Key(KeyEvent::new(
            KeyEventKind::Press,
            KeyCode::Enter,
        ))],
    )))
    .map(|canvas| canvas.to_string())
    .collect::<Vec<_>>()
    .await;

    assert_eq!(
        selected_outcome,
        Some(SessionsPickerOutcome::TerminalTooNarrow)
    );
    assert!(
        actual
            .last()
            .is_some_and(|snapshot| snapshot.contains("terminal too narrow")),
        "too-narrow picker should render only the concise error: {actual:?}"
    );
}

#[tokio::test]
async fn sessions_picker_width_contract_preserves_layout() {
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
        assert!(
            text.lines().all(|line| line.chars().count() <= width),
            "session picker capture width {width} overflowed:\n{text}"
        );
        assert!(
            text.lines()
                .any(|line| line.contains("❯ Provider migration")),
            "capture should select the long-title row:\n{text}"
        );
        assert!(
            text.contains("+"),
            "capture should show the more-below affordance:\n{text}"
        );
        assert!(
            text.contains("ctrl-n new"),
            "capture should expose the new-thread shortcut:\n{text}"
        );
        assert!(
            text.contains("opt-enter fork"),
            "capture should expose the fork shortcut:\n{text}"
        );
        let lines = text.lines().collect::<Vec<_>>();
        let footer_index = lines
            .iter()
            .rposition(|line| line.contains("ctrl-o sort"))
            .unwrap_or_else(|| panic!("capture should render footer:\n{text}"));
        let bottom_border_index = lines
            .iter()
            .rposition(|line| line.contains('╰'))
            .unwrap_or_else(|| panic!("capture should render bottom border:\n{text}"));
        assert_eq!(
            bottom_border_index,
            footer_index + 1,
            "picker outer border should sit directly below footer at width {width}:\n{text}"
        );
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
    assert!(
        empty_text.contains("Start new session"),
        "empty state should offer a new session:\n{empty_text}"
    );
}

#[tokio::test]
async fn sessions_picker_stacked_details_wraps_conversation_without_preview_or_metadata() {
    let mut request = capture_picker_request();
    let record = request
        .records
        .get_mut(0)
        .unwrap_or_else(|| panic!("capture request should have a record"));
    record.session_id = "019ff0bb-5993-70d3-b1ba-f56724b94919".to_owned();
    record.conversation.snippets = vec![format!(
        "{} final-conversation-marker",
        "conversation words that should use the available detail width ".repeat(3)
    )];

    let width = 120;
    let text = render_picker_capture(
        request,
        width,
        vec![TerminalEvent::Key(KeyEvent::new(
            KeyEventKind::Press,
            KeyCode::Esc,
        ))],
    )
    .await;

    assert!(
        text.lines().all(|line| line.chars().count() <= width),
        "wrapped conversation should not overflow stacked details frame:\n{text}"
    );
    assert!(text.contains("final-conversation-marker"), "{text}");
    let session_id_index = text
        .find("Session ID: 019ff0bb-5993-70d3-b1ba-f56724b94919")
        .unwrap_or_else(|| panic!("selected session ID should render:\n{text}"));
    let conversation_index = text
        .find("Conversation")
        .unwrap_or_else(|| panic!("conversation heading should render:\n{text}"));
    assert!(
        session_id_index < conversation_index,
        "selected session ID should appear before Conversation:\n{text}"
    );
    assert!(!text.contains("Preview"), "{text}");
    assert!(!text.contains("Metadata"), "{text}");
}

#[tokio::test]
async fn sessions_picker_stacked_details_clip_long_conversation_without_hiding_the_panel() {
    let mut request = capture_picker_request();
    let record = request
        .records
        .get_mut(0)
        .unwrap_or_else(|| panic!("capture request should have a record"));
    record.conversation.snippets = (0..10)
        .map(|index| format!("message {index}: {}", "conversation text ".repeat(12)))
        .collect();

    let text = render_picker_capture_at(
        request,
        120,
        40,
        vec![TerminalEvent::Key(KeyEvent::new(
            KeyEventKind::Press,
            KeyCode::Esc,
        ))],
    )
    .await;

    assert!(
        text.contains("Conversation"),
        "stacked details should remain visible and clip overflow:\n{text}"
    );
    assert!(
        text.contains("Feature design session")
            && text.contains("Provider migration with very very long provider metadata"),
        "stacked details should leave room for multiple session rows:\n{text}"
    );
}

#[tokio::test]
async fn sessions_picker_renders_minimum_height_from_short_resize() {
    let text = render_picker_capture_at(
        capture_picker_request(),
        160,
        12,
        vec![TerminalEvent::Key(KeyEvent::new(
            KeyEventKind::Press,
            KeyCode::Esc,
        ))],
    )
    .await;

    assert_eq!(
        meaningful_line_count(&text),
        24,
        "short terminals should still render the 24-row minimum:\n{text}"
    );
}

#[tokio::test]
async fn sessions_picker_uses_taller_height_for_more_visible_rows() {
    let short_text = render_picker_capture_at(
        capture_picker_request(),
        160,
        24,
        vec![TerminalEvent::Key(KeyEvent::new(
            KeyEventKind::Press,
            KeyCode::Esc,
        ))],
    )
    .await;
    let tall_text = render_picker_capture_at(
        capture_picker_request(),
        160,
        32,
        vec![TerminalEvent::Key(KeyEvent::new(
            KeyEventKind::Press,
            KeyCode::Esc,
        ))],
    )
    .await;

    let short_rows = visible_followup_row_count(&short_text);
    let tall_rows = visible_followup_row_count(&tall_text);
    assert!(
        tall_rows > short_rows,
        "taller sessions view should spend height on visible rows before blank space; short={short_rows}, tall={tall_rows}\nshort:\n{short_text}\ntall:\n{tall_text}"
    );
}

#[tokio::test]
async fn sessions_picker_live_resize_uses_terminal_height_and_taller_heights() {
    let short_text = render_picker_capture_at(
        capture_picker_request(),
        0,
        0,
        vec![
            TerminalEvent::Resize(160, 12),
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Esc)),
        ],
    )
    .await;
    let tall_text = render_picker_capture_at(
        capture_picker_request(),
        0,
        0,
        vec![
            TerminalEvent::Resize(160, 32),
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Esc)),
        ],
    )
    .await;

    assert_eq!(
        meaningful_line_count(&short_text),
        12,
        "live resize below 24 rows should respect terminal height:\n{short_text}"
    );
    let short_rows = visible_followup_row_count(&short_text);
    let tall_rows = visible_followup_row_count(&tall_text);
    assert!(
        tall_rows > short_rows,
        "live resize to a taller terminal should render more rows; short={short_rows}, tall={tall_rows}\nshort:\n{short_text}\ntall:\n{tall_text}"
    );
}

#[tokio::test]
async fn sessions_picker_sidecar_clamps_tall_details_to_body_budget() {
    let mut request = capture_picker_request();
    let record = request
        .records
        .get_mut(0)
        .unwrap_or_else(|| panic!("capture request should have a record"));
    record.conversation.snippets = vec![
        "first long sidecar snippet".to_owned(),
        "second long sidecar snippet".to_owned(),
        "third long sidecar snippet".to_owned(),
        "fourth long sidecar snippet".to_owned(),
    ];
    let text = render_picker_capture_at(
        request,
        160,
        24,
        vec![TerminalEvent::Key(KeyEvent::new(
            KeyEventKind::Press,
            KeyCode::Esc,
        ))],
    )
    .await;

    assert_eq!(
        meaningful_line_count(&text),
        24,
        "sidecar details should stay within the 24-row frame:\n{text}"
    );
    assert!(
        text.contains("Search: id:<id>"),
        "sidecar details should not clip the footer at 160x24:\n{text}"
    );
}

#[tokio::test]
async fn sessions_picker_sidecar_panels_reach_footer_without_unframed_gap() {
    let text = render_picker_capture_at(
        capture_picker_request(),
        160,
        32,
        vec![TerminalEvent::Key(KeyEvent::new(
            KeyEventKind::Press,
            KeyCode::Esc,
        ))],
    )
    .await;
    let lines = text.lines().collect::<Vec<_>>();
    let footer_index = lines
        .iter()
        .position(|line| line.contains("Search: id:<id>"))
        .unwrap_or_else(|| panic!("sidecar footer should render:\n{text}"));
    let panel_bottom_index = lines[..footer_index]
        .iter()
        .rposition(|line| line.matches('└').count() >= 2)
        .unwrap_or_else(|| panic!("both sidecar panel bottoms should render:\n{text}"));

    assert_eq!(
        panel_bottom_index + 2,
        footer_index,
        "sidecar panels should meet the footer divider without an unframed gap:\n{text}"
    );
}

#[tokio::test]
async fn sessions_picker_renders_one_line_controls_when_width_allows() {
    let text = render_picker_capture_at(
        capture_picker_request(),
        160,
        24,
        vec![TerminalEvent::Key(KeyEvent::new(
            KeyEventKind::Press,
            KeyCode::Esc,
        ))],
    )
    .await;

    assert!(
        text.lines().any(|line| {
            line.contains("Search: []")
                && line.contains("[all]")
                && line.contains("Threads:")
                && line.contains("Sort:")
        }),
        "wide sessions controls should fit on one line:\n{text}"
    );
    assert!(
        !text.contains("Search text, id:, b:branch, repo:name"),
        "{text}"
    );
    assert!(
            text.contains("Search: id:<id> | b:<branch> | repo:<name>    opt-enter fork | ctrl-n new | ctrl-s scope | ctrl-t threads | ctrl-o sort"),
            "wide footer should group qualified search and shortcuts:\n{text}"
        );
    assert!(!text.contains("type search"), "{text}");
    assert!(!text.contains("enter resume"), "{text}");
    assert!(!text.contains("esc exit"), "{text}");
}

#[tokio::test]
async fn sessions_picker_budgets_wrapped_controls_from_actual_search_text() {
    let mut events = "012345678901234567890123456789012345678901234567890123456789012345678901234567890123456789"
            .chars()
            .map(|character| {
                TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Char(character)))
            })
            .collect::<Vec<_>>();
    events.push(TerminalEvent::Key(KeyEvent::new(
        KeyEventKind::Press,
        KeyCode::Esc,
    )));
    events.push(TerminalEvent::Key(KeyEvent::new(
        KeyEventKind::Press,
        KeyCode::Esc,
    )));
    let text = render_picker_capture_at(capture_picker_request(), 100, 24, events).await;

    assert_eq!(
        meaningful_line_count(&text),
        24,
        "long search controls should not grow the picker past the 24-row frame:\n{text}"
    );
    assert!(
        text.contains("Search: id:<id>"),
        "wrapped controls should not clip the footer at 100x24:\n{text}"
    );
    assert!(
        text.lines().all(|line| line.chars().count() <= 100),
        "wrapped controls should fit the terminal width:\n{text}"
    );
}

#[tokio::test]
async fn sessions_picker_stacked_layout_removes_top_padding_and_dead_list_tail() {
    let text = render_picker_capture_at(
        capture_picker_request(),
        120,
        24,
        vec![TerminalEvent::Key(KeyEvent::new(
            KeyEventKind::Press,
            KeyCode::Esc,
        ))],
    )
    .await;
    let lines = text.lines().collect::<Vec<_>>();
    let title_index = lines
        .iter()
        .position(|line| line.contains("Resume a previous session"))
        .unwrap_or_else(|| panic!("sessions title should render:\n{text}"));
    let top_border_index = lines
        .iter()
        .position(|line| line.contains('╭'))
        .unwrap_or_else(|| panic!("sessions top border should render:\n{text}"));
    assert_eq!(
        title_index,
        top_border_index + 1,
        "title should sit directly below the outer border:\n{text}"
    );

    let header_index = lines
        .iter()
        .position(|line| line.contains("Session") && line.contains("Upd"))
        .unwrap_or_else(|| panic!("sessions list header should render:\n{text}"));
    let list_top_border_index = lines[..header_index]
        .iter()
        .rposition(|line| line.contains('┌'))
        .unwrap_or_else(|| panic!("sessions list top border should render:\n{text}"));
    assert_eq!(
        header_index,
        list_top_border_index + 1,
        "list header should sit directly below the list border:\n{text}"
    );

    let more_below_index = lines
        .iter()
        .position(|line| line.contains("more below"))
        .unwrap_or_else(|| panic!("sessions list should render a more-below row:\n{text}"));
    let list_bottom_border_index = more_below_index + 1;
    assert!(
        lines
            .get(list_bottom_border_index)
            .is_some_and(|line| line.contains('└')),
        "sessions list bottom border should sit directly below the more-below row:\n{text}"
    );
    let row_before_bottom = lines
        .get(list_bottom_border_index.saturating_sub(1))
        .unwrap_or_else(|| panic!("sessions list should have content above bottom:\n{text}"));
    assert!(
        row_before_bottom.contains("Follow-up implementation lane")
            || row_before_bottom.contains("more below")
            || row_before_bottom.contains("more above"),
        "sessions list should not leave an empty tail above its bottom border:\n{text}"
    );
}

#[tokio::test]
async fn sessions_picker_uses_sidecar_only_at_160_columns() {
    let stacked_text = render_picker_capture(
        capture_picker_request(),
        159,
        vec![TerminalEvent::Key(KeyEvent::new(
            KeyEventKind::Press,
            KeyCode::Esc,
        ))],
    )
    .await;
    assert!(
        !has_sidecar_details(&stacked_text),
        "session picker should stack details below 160 columns:\n{stacked_text}"
    );

    let sidecar_text = render_picker_capture(
        capture_picker_request(),
        160,
        vec![TerminalEvent::Key(KeyEvent::new(
            KeyEventKind::Press,
            KeyCode::Esc,
        ))],
    )
    .await;
    assert!(
        has_sidecar_details(&sidecar_text),
        "session picker should place details on the right at 160 columns:\n{sidecar_text}"
    );
}

#[tokio::test]
async fn sessions_picker_reflows_when_terminal_width_changes() {
    let frames = element! {
        SessionsPickerComponent(
            request: capture_picker_request(),
            width: 0usize,
        )
    }
    .mock_terminal_render_loop(MockTerminalConfig::with_events(futures_util::stream::iter(
        vec![
            TerminalEvent::Resize(159, 40),
            TerminalEvent::Resize(160, 40),
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Esc)),
        ],
    )))
    .map(|canvas| canvas.to_string())
    .collect::<Vec<_>>()
    .await;

    assert!(
        frames.iter().any(|frame| !has_sidecar_details(frame)),
        "session picker should render a stacked frame after shrinking below 160 columns: {frames:?}"
    );
    assert!(
        frames.iter().any(|frame| has_sidecar_details(frame)),
        "session picker should render a sidecar frame after growing to 160 columns: {frames:?}"
    );
}

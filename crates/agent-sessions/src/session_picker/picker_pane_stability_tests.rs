use super::*;

#[tokio::test]
async fn sessions_picker_stacked_panel_boundaries_ignore_conversation_height() {
    let mut short_request = capture_picker_request();
    for record in &mut short_request.records {
        record.conversation.snippets = vec!["SHORT_CONVERSATION_MARKER".to_owned()];
    }

    let mut long_request = short_request.clone();
    for record in &mut long_request.records {
        record.conversation.snippets = (0..10)
            .map(|index| {
                format!(
                    "LONG_CONVERSATION_MARKER {index}: {}",
                    "wrapped text ".repeat(12)
                )
            })
            .collect();
    }
    let start_new_request = short_request.clone();
    let mut empty_request = short_request.clone();
    empty_request.records.clear();

    let select_first_session = || {
        vec![
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Down)),
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Esc)),
        ]
    };
    let short_text = render_picker_capture_at(short_request, 120, 40, select_first_session()).await;
    let long_text = render_picker_capture_at(long_request, 120, 40, select_first_session()).await;
    let start_new_text = render_picker_capture_at(
        start_new_request,
        120,
        40,
        vec![
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Up)),
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Esc)),
        ],
    )
    .await;
    let empty_text = render_picker_capture_at(
        empty_request,
        120,
        40,
        vec![TerminalEvent::Key(KeyEvent::new(
            KeyEventKind::Press,
            KeyCode::Esc,
        ))],
    )
    .await;

    assert!(
        short_text.contains("SHORT_CONVERSATION_MARKER"),
        "short selected conversation should render:\n{short_text}"
    );
    assert!(
        long_text.contains("LONG_CONVERSATION_MARKER"),
        "long selected conversation should render:\n{long_text}"
    );
    assert_eq!(
        start_new_text.matches("Start new session").count(),
        2,
        "Start New selection should render both its list row and fixed detail pane:\n{start_new_text}"
    );
    assert!(
        !start_new_text.contains("Session ID:"),
        "Start New detail pane should not retain session details:\n{start_new_text}"
    );

    let panel_boundaries = |text: &str| {
        let lines = text.lines().collect::<Vec<_>>();
        let list_top = lines
            .iter()
            .position(|line| line.contains('┌'))
            .unwrap_or_else(|| panic!("stacked list top border should render:\n{text}"));
        let panel_border_column = lines[list_top]
            .chars()
            .position(|character| character == '┌')
            .unwrap_or_else(|| panic!("stacked list top border should render:\n{text}"));
        let list_bottom = lines
            .iter()
            .enumerate()
            .skip(list_top + 1)
            .find_map(|(index, line)| {
                (line.chars().nth(panel_border_column) == Some('└')).then_some(index)
            })
            .unwrap_or_else(|| panic!("stacked list bottom border should render:\n{text}"));
        let details_top = lines
            .iter()
            .enumerate()
            .skip(list_bottom + 1)
            .find_map(|(index, line)| {
                (line.chars().nth(panel_border_column) == Some('┌')).then_some(index)
            })
            .unwrap_or_else(|| panic!("stacked details top border should render:\n{text}"));
        let details_bottom = lines
            .iter()
            .enumerate()
            .skip(details_top + 1)
            .find_map(|(index, line)| {
                (line.chars().nth(panel_border_column) == Some('└')).then_some(index)
            })
            .unwrap_or_else(|| panic!("stacked details bottom border should render:\n{text}"));
        (list_top, list_bottom, details_top, details_bottom)
    };

    let short_boundaries = panel_boundaries(&short_text);
    let long_boundaries = panel_boundaries(&long_text);
    let start_new_boundaries = panel_boundaries(&start_new_text);
    let empty_boundaries = panel_boundaries(&empty_text);
    assert_eq!(
        short_boundaries, long_boundaries,
        "stacked list and detail panes should not move with selected conversation height\nshort:\n{short_text}\nlong:\n{long_text}"
    );
    assert_eq!(
        short_boundaries, start_new_boundaries,
        "stacked panes should not move when Start New is selected\nsession:\n{short_text}\nstart new:\n{start_new_text}"
    );
    assert_eq!(
        short_boundaries, empty_boundaries,
        "stacked panes should keep their allocation when no existing sessions match\nsessions:\n{short_text}\nempty:\n{empty_text}"
    );

    let (list_top, _, details_top, details_bottom) = short_boundaries;
    let body_height = details_bottom - list_top + 1;
    let details_height = details_bottom - details_top + 1;
    assert!(
        details_height == body_height.saturating_mul(2).div_ceil(5),
        "stacked details should receive 40% of the usable pane area; body={body_height}, details={details_height}:\n{short_text}"
    );
    let footer_index = short_text
        .lines()
        .position(|line| line.contains("ctrl-/ Help"))
        .unwrap_or_else(|| panic!("stacked footer should render:\n{short_text}"));
    assert_eq!(
        details_bottom + 2,
        footer_index,
        "stacked pane allocation should consume the body budget without an unframed gap:\n{short_text}"
    );
}

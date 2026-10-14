use super::*;

#[tokio::test]
async fn sessions_list_viewport_is_independent_of_hidden_conversation_height() {
    let mut short_request = capture_picker_request();
    for record in &mut short_request.records {
        record.conversation.snippets = vec!["SHORT_CONVERSATION_MARKER".to_owned()];
    }
    let mut long_request = short_request.clone();
    for record in &mut long_request.records {
        record.conversation.snippets = (0..40)
            .map(|index| format!("LONG_MESSAGE {index}: {}", "text ".repeat(30)))
            .collect();
    }
    let events = || {
        vec![TerminalEvent::Key(KeyEvent::new(
            KeyEventKind::Press,
            KeyCode::Esc,
        ))]
    };
    let short_text = render_picker_capture_at(short_request, 120, 40, events()).await;
    let long_text = render_picker_capture_at(long_request, 120, 40, events()).await;
    assert_eq!(
        short_text, long_text,
        "hidden details must not change list geometry"
    );
    assert!(!short_text.contains("SHORT_CONVERSATION_MARKER"));
    assert!(!long_text.contains("LONG_MESSAGE"));
    assert!(short_text.contains("Details F4"));
    assert!(
        visible_followup_row_count(&short_text) > 1,
        "list must use reclaimed height: {short_text}"
    );
    assert_eq!(meaningful_line_count(&short_text), 40);
}

#[tokio::test]
async fn sessions_details_dialog_scrolls_without_launching_or_mutating_search() {
    let mut request = capture_picker_request();
    for record in &mut request.records {
        record.conversation.snippets = (0..40).map(|index| format!("MESSAGE {index}")).collect();
    }
    let mut outcome: Option<SessionsPickerOutcome> = None;
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    let events = futures_util::stream::unfold(receiver, |mut receiver| async {
        receiver.recv().await.map(|event| (event, receiver))
    });
    {
        let mut ui = element!(SessionsPickerComponent(request,width:48usize,height:30usize,selected_outcome_out:&mut outcome));
        let frames = ui.mock_terminal_render_loop(MockTerminalConfig::with_events(events));
        tokio::pin!(frames);
        let mut phase = 0;
        let mut last = String::new();
        let journey = async {
            while let Some(canvas) = frames.next().await {
                let frame = canvas.to_string();
                if phase == 0 && frame.contains("Details F4") {
                    sender
                        .send(TerminalEvent::Key(KeyEvent::new(
                            KeyEventKind::Press,
                            KeyCode::F(4),
                        )))
                        .unwrap();
                    phase = 1;
                } else if phase == 1 && frame.contains("Session details") && frame.contains('┃') {
                    sender.send(ctrl_key('n')).unwrap();
                    sender.send(alt_enter_key()).unwrap();
                    sender
                        .send(TerminalEvent::Key(KeyEvent::new(
                            KeyEventKind::Press,
                            KeyCode::Char('x'),
                        )))
                        .unwrap();
                    sender
                        .send(TerminalEvent::Key(KeyEvent::new(
                            KeyEventKind::Press,
                            KeyCode::End,
                        )))
                        .unwrap();
                    phase = 2;
                } else if phase == 2 && frame.contains("MESSAGE 39") {
                    assert!(
                        frame.contains("Close · Esc"),
                        "scroll must keep close action: {frame}"
                    );
                    assert!(!frame.contains("Search: [x]"));
                    assert!(frame.lines().count() <= 30, "{frame}");
                    sender.send(ctrl_key('d')).unwrap();
                    phase = 3;
                }
                last = frame;
            }
        };
        let result = tokio::time::timeout(Duration::from_secs(2), journey).await;
        assert!(result.is_ok(), "dialog journey phase={phase}: {last}");
        assert_eq!(phase, 3, "{last}");
    }
    assert_eq!(
        outcome, None,
        "background launch/fork keys must not activate"
    );
}

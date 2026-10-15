#[test]
fn review_qhelp_preserves_legacy_and_disambiguated_help_keys() {
    use super::quota_help_rendering::quota_help_key;

    // Legacy 0x1f becomes Control+7 in the pinned Unix decoder. The compiled
    // PTY regression below this layer supplies that byte to the actual decoder.
    for (code, modifiers) in [
        (KeyCode::Char('7'), KeyModifiers::CONTROL),
        (KeyCode::Char('_'), KeyModifiers::CONTROL),
        (KeyCode::Char('/'), KeyModifiers::CONTROL),
        (KeyCode::Char('\u{1f}'), KeyModifiers::empty()),
        (KeyCode::F(1), KeyModifiers::empty()),
    ] {
        assert!(quota_help_key(code, modifiers), "Help key rejected: {code:?}");
    }
    for code in [KeyCode::Char('7'), KeyCode::Char('_'), KeyCode::Char('/')] {
        assert!(!quota_help_key(code, KeyModifiers::empty()));
    }
}

#[tokio::test]
async fn review_qdetail_help_describes_existing_escape_close() {
    let frames = render_quota_filter_phases(
        quota_two_account_view_model(),
        120,
        36,
        vec![
            ("Quota status", vec![provider_filter_key(KeyCode::F(4))]),
            ("Account details", vec![provider_filter_key(KeyCode::F(1))]),
            ("Quota Help", vec![provider_filter_key(KeyCode::Esc)]),
            ("Account details", vec![provider_filter_key(KeyCode::Esc)]),
            ("Filters", vec![provider_filter_key(KeyCode::Esc)]),
        ],
        None,
    ).await;
    let help = frames.iter().find(|frame| frame.contains("Quota Help"))
        .expect("Details contextual Help must render");
    assert!(help.contains("Close details"), "{help}");
    assert!(!help.contains("Esc / F4"), "F4 is an opener, not an existing close action: {help}");
    let browse = frames.last().expect("Esc returns to Browse");
    assert!(browse.contains("Quota status") && !browse.contains("Account details"), "{browse}");
}

#[tokio::test]
async fn quota_help_top_line_hides_inline_shortcut_labels() {
    let text = render_quota_capture_model_at(quota_two_account_view_model(), 120, 36,
        vec![provider_filter_key(KeyCode::Esc)]).await;
    let title = text.lines().find(|line| line.contains("Quota status")).expect("surface title");
    assert!(title.contains("Help"), "{text}");
    for shortcut in ["Ctrl+F", "F4", "ctrl-r", "↑/↓ focus", "esc/q"] {
        assert!(!text.contains(shortcut), "visible shortcut {shortcut}: {text}");
    }
}

#[tokio::test]
async fn owner_feedback_quota_help_is_secondary_in_the_actual_header() {
    let mut component = element!(QuotaStatusComponent(view_model: quota_two_account_view_model(), width: 48usize, height: 24usize));
    let canvases = component.mock_terminal_render_loop(MockTerminalConfig::with_events(futures_util::stream::iter(vec![provider_filter_key(KeyCode::Esc)]))).collect::<Vec<_>>().await;
    let canvas = canvases.last().unwrap();
    let frame = canvas.to_string();
    let (row, line) = frame.lines().enumerate().find(|(_, line)| line.contains("Quota status") && line.contains("Help")).unwrap();
    let offset = line.find("Help").unwrap();
    let column = line.get(..offset).expect("matched label is a UTF-8 boundary").chars().count();
    let style = canvas.cell(column, row).unwrap().text_style().unwrap();
    assert_eq!(style.color, Some(Color::Grey), "Help must stay enabled but secondary: {frame}");
    assert_eq!(style.weight, Weight::Normal);
}

#[tokio::test]
async fn quota_help_phone_header_keeps_title_and_help_when_pool_status_is_long() {
    let mut model = quota_two_account_view_model();
    model.pool_freshness_summary = "3 accounts stale · one refresh failed".to_owned();
    let frame = render_quota_capture_model_at(model, 48, 24, vec![provider_filter_key(KeyCode::Esc)]).await;
    let title = frame.lines().find(|line| line.contains("Quota status")).expect("phone title must retain its meaning");
    assert!(title.contains("Help"), "status cannot push Help outside viewport: {frame}");
    assert!(title.contains("stale"), "retain the status while clipping its excess: {frame}");
}

#[tokio::test]
async fn quota_help_filter_selection_stays_yellow_when_apply_has_focus() {
    let mut exit = KeyEvent::new(KeyEventKind::Press, KeyCode::Char('d'));
    exit.modifiers = KeyModifiers::CONTROL;
    let mut component = element!(QuotaStatusComponent(view_model: mixed_provider_filter_view_model(), width: 120usize, height: 36usize));
    let canvases = component.mock_terminal_render_loop(MockTerminalConfig::with_events(futures_util::stream::iter(vec![
        provider_filter_key(KeyCode::F(3)), provider_filter_key(KeyCode::Down),
        provider_filter_key(KeyCode::Tab), provider_filter_key(KeyCode::Tab),
        TerminalEvent::Key(exit),
    ]))).collect::<Vec<_>>().await;
    let canvas = canvases.last().expect("filter frame");
    let frame = canvas.to_string();
    assert!(frame.contains("Account filters"), "dialog title names its own task: {frame}");
    let (row, line) = frame.lines().enumerate().find(|(_, line)| line.contains("● OpenAI")).expect("selected provider");
    let offset = line.find("OpenAI").expect("provider label");
    let column = line.get(..offset).expect("UTF-8 label boundary").chars().count();
    let style = canvas.cell(column, row).expect("provider cell").text_style().expect("provider style");
    assert_eq!(style.color, Some(Color::Yellow), "selection cannot depend on action focus: {frame}");
    assert_eq!(style.weight, Weight::Bold);
    let (row, line) = frame.lines().enumerate().find(|(_, line)| line.contains("Apply") && line.contains("Cancel")).expect("actions");
    let offset = line.find("Apply").expect("Apply label");
    let column = line.get(..offset).expect("UTF-8 action boundary").chars().count();
    let style = canvas.cell(column, row).expect("action cell").text_style().expect("action style");
    assert_eq!(style.color, Some(Color::Yellow));
    assert!(style.underline, "focus must differ from provider selection: {frame}");
}

#[tokio::test]
async fn quota_help_wide_editor_keeps_plain_save_and_cancel_actions_visible() {
    let saver: super::quota_floor_editor::WeeklyQuotaFloorSaver = Arc::new(|_, _| Box::pin(async { Ok(()) }));
    let mut open = KeyEvent::new(KeyEventKind::Press, KeyCode::Char('e'));
    open.modifiers = KeyModifiers::CONTROL;
    let mut exit = KeyEvent::new(KeyEventKind::Press, KeyCode::Char('d'));
    exit.modifiers = KeyModifiers::CONTROL;
    let mut component = element!(QuotaStatusComponent(view_model: quota_two_account_view_model(), width: 160usize, height: 40usize, weekly_floor_saver: Some(saver)));
    let frames = component.mock_terminal_render_loop(MockTerminalConfig::with_events(futures_util::stream::iter(vec![
        TerminalEvent::Key(open), TerminalEvent::Key(exit),
    ]))).map(|canvas| canvas.to_string()).collect::<Vec<_>>().await;
    let frame = frames.last().expect("wide editor frame");
    assert!(frame.contains("Weekly floor"), "{frame}");
    assert!(frame.contains("Save") && frame.contains("Cancel"), "plain actions cannot disappear in sidecar layout: {frame}");
    assert!(!frame.contains("enter save"), "{frame}");
}

#[tokio::test]
async fn quota_help_retains_filter_draft_and_blocks_background_keys() {
    let frames = render_quota_filter_phases(mixed_provider_filter_view_model(), 120, 36, vec![
        ("Filters", vec![provider_filter_key(KeyCode::F(3))]),
        ("Account provider · Draft: All", vec![provider_filter_key(KeyCode::Down), provider_filter_key(KeyCode::F(1))]),
        ("Quota Help", vec![provider_filter_key(KeyCode::Down), provider_filter_key(KeyCode::Enter), provider_filter_key(KeyCode::Esc)]),
        ("Account provider · Draft: OpenAI", vec![provider_filter_key(KeyCode::Esc)]),
        ("Provider: All", vec![provider_filter_key(KeyCode::Esc)]),
    ], None).await;
    assert!(frames.iter().any(|frame| frame.contains("Quota Help") && frame.contains("Space") && frame.contains("Apply draft")));
    assert!(frames.iter().any(|frame| frame.contains("Account provider · Draft: OpenAI")));
    assert!(frames.last().expect("browse return").contains("Provider: All"));
}

#[tokio::test]
async fn quota_help_control_slash_opens_and_f1_returns_to_browse() {
    let mut open = KeyEvent::new(KeyEventKind::Press, KeyCode::Char('/'));
    open.modifiers = KeyModifiers::CONTROL;
    let frames = render_quota_filter_phases(quota_two_account_view_model(), 120, 36, vec![
        ("Quota status", vec![TerminalEvent::Key(open)]),
        ("Quota Help", vec![provider_filter_key(KeyCode::F(1))]),
        ("Filters", vec![provider_filter_key(KeyCode::Esc)]),
    ], None).await;
    assert!(frames.iter().any(|frame| frame.contains("Quota Help") && frame.contains("Edit weekly floor")));
    assert!(!frames.last().expect("closed help").contains("Quota Help"));
}

#[test]
fn quota_help_static_output_keeps_existing_footer_and_has_no_help_action() {
    let text = render_quota_static_capture(quota_two_account_view_model(), 120, false);
    assert!(!text.contains("Help"));
    assert!(text.contains("ctrl-r account options"));
}

#[tokio::test]
async fn quota_help_real_mouse_header_blocks_background_and_close_preserves_focus() {
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    let events = futures_util::stream::unfold(receiver, |mut receiver| async move {
        receiver.recv().await.map(|event| (event, receiver))
    });
    let mut stage = 0;
    let mut component = element!(QuotaStatusComponent(view_model: quota_two_account_view_model(), width: 160usize, height: 36usize));
    let journey = component.mock_terminal_render_loop(MockTerminalConfig::with_events(events))
        .map(|canvas| canvas.to_string()).inspect(move |frame| {
            if stage == 0 && frame.contains("Quota status") {
                let (row, line) = frame.lines().enumerate().find(|(_, line)| line.contains("Quota status") && line.contains("Help")).expect("Help shares surface top line");
                let column = line.chars().collect::<String>().find("Help").expect("Help label");
                let column = line.get(..column).expect("Help label starts at a UTF-8 boundary").chars().count();
                sender.send(TerminalEvent::FullscreenMouse(FullscreenMouseEvent::new(MouseEventKind::Down(MouseButton::Left), column as u16, row as u16))).expect("mouse header input");
                stage = 1;
            } else if stage == 1 && frame.contains("Quota Help") {
                sender.send(TerminalEvent::FullscreenMouse(FullscreenMouseEvent::new(MouseEventKind::Down(MouseButton::Left), 10, 10))).expect("background click");
                sender.send(provider_filter_key(KeyCode::Down)).expect("background navigation");
                sender.send(provider_filter_key(KeyCode::Esc)).expect("close Help");
                stage = 2;
            } else if stage == 2 && !frame.contains("Quota Help") {
                assert!(frame.contains("❯ alpha"), "background focus changed: {frame}");
                sender.send(provider_filter_key(KeyCode::Esc)).expect("exit");
                stage = 3;
            }
        }).collect::<Vec<_>>();
    let frames = tokio::time::timeout(Duration::from_secs(2), journey).await.expect("bounded mouse Help journey");
    assert!(frames.iter().any(|frame| frame.contains("Quota Help")));
}

#[tokio::test]
async fn quota_help_plain_floor_save_action_uses_existing_command_port() {
    let saves = Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorded = Arc::clone(&saves);
    let saver: super::quota_floor_editor::WeeklyQuotaFloorSaver = Arc::new(move |account_id, percent| {
        recorded.lock().expect("save receipt").push((account_id, percent));
        Box::pin(async { Err(super::quota_floor_editor::WeeklyQuotaFloorSaveError::StateOperationFailed) })
    });
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    let events = futures_util::stream::unfold(receiver, |mut receiver| async move { receiver.recv().await.map(|event| (event, receiver)) });
    let mut open = KeyEvent::new(KeyEventKind::Press, KeyCode::Char('e'));
    open.modifiers = KeyModifiers::CONTROL;
    let mut stage = 0;
    let model = quota_two_account_view_model();
    let expected_id = model.rows.first().expect("fixture account").account_id.clone();
    let mut component = element!(QuotaStatusComponent(view_model: model, width: 120usize, height: 36usize, weekly_floor_saver: Some(saver)));
    let render = component.mock_terminal_render_loop(MockTerminalConfig::with_events(events)).map(|canvas| canvas.to_string()).inspect(move |frame| {
        if stage == 0 { sender.send(TerminalEvent::Key(open.clone())).expect("open floor"); stage = 1; }
        else if stage == 1 && frame.contains("Weekly floor") {
            let (row, line) = frame.lines().enumerate().find(|(_, line)| line.contains("Save") && line.contains("Cancel")).expect("plain real Save/Cancel actions");
            let offset = line.find("Save").expect("Save action");
            sender.send(TerminalEvent::FullscreenMouse(FullscreenMouseEvent::new(MouseEventKind::Down(MouseButton::Left), line.get(..offset).expect("action label starts at a UTF-8 boundary").chars().count() as u16, row as u16))).expect("click Save");
            stage = 2;
        } else if stage == 2 && frame.contains("weekly floor update failed") {
            sender.send(provider_filter_key(KeyCode::Esc)).expect("cancel failed draft"); stage = 3;
        } else if stage == 3 && !frame.contains("Weekly floor") {
            sender.send(provider_filter_key(KeyCode::Esc)).expect("exit"); stage = 4;
        }
    }).collect::<Vec<_>>();
    tokio::time::timeout(Duration::from_secs(2), render).await.expect("bounded plain Save journey");
    assert_eq!(*saves.lock().expect("save receipt"), vec![(expected_id, 0)]);
}

#[tokio::test]
async fn quota_help_wide_sidecar_floor_arrow_is_blocked_until_help_closes() {
    let saves = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let saved_calls = Arc::clone(&saves);
    let saver: super::quota_floor_editor::WeeklyQuotaFloorSaver = Arc::new(move |_, _| {
        saved_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Box::pin(async { Ok(()) })
    });
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    let events = futures_util::stream::unfold(receiver, |mut receiver| async move {
        receiver.recv().await.map(|event| (event, receiver))
    });
    let mut open_floor = KeyEvent::new(KeyEventKind::Press, KeyCode::Char('e'));
    open_floor.modifiers = KeyModifiers::CONTROL;
    let click = |column, row| TerminalEvent::FullscreenMouse(FullscreenMouseEvent::new(
        MouseEventKind::Down(MouseButton::Left), column, row));
    let mut stage = 0;
    let mut original_increment = None;
    let mut component = element!(QuotaStatusComponent(view_model: quota_two_account_view_model(), width: 160usize, height: 40usize, weekly_floor_saver: Some(saver)));
    let render = component.mock_terminal_render_loop(MockTerminalConfig::with_events(events))
        .map(|canvas| canvas.to_string()).inspect(move |frame| {
            if stage == 0 {
                sender.send(TerminalEvent::Key(open_floor.clone())).expect("open real floor editor"); stage = 1;
            } else if stage == 1 && frame.contains("Weekly floor") {
                let (row, line) = frame.lines().enumerate().find(|(_, line)| line.contains('◀') && line.contains('▶')).expect("sidecar floor arrows");
                assert!(line.contains("0%"), "initial floor draft: {frame}");
                let offset = line.find('▶').expect("increment arrow");
                original_increment = Some((line.get(..offset).expect("action label starts at a UTF-8 boundary").chars().count() as u16, row as u16));
                sender.send(provider_filter_key(KeyCode::F(1))).expect("open contextual Help"); stage = 2;
            } else if stage == 2 && frame.contains("Quota Help") {
                assert!(frame.contains("Adjust floor"), "floor-context Help: {frame}");
                let (column, row) = original_increment.expect("recorded original sidecar arrow position");
                sender.send(click(column, row)).expect("attempt blocked sidecar increment");
                sender.send(provider_filter_key(KeyCode::Esc)).expect("close Help"); stage = 3;
            } else if stage == 3 && !frame.contains("Quota Help") && frame.contains("Weekly floor") {
                let line = frame.lines().find(|line| line.contains('◀') && line.contains('▶')).expect("retained floor editor");
                assert!(line.contains("0%"), "Help allowed background arrow to mutate draft: {frame}");
                let (column, row) = original_increment.expect("retained increment position");
                sender.send(click(column, row)).expect("unblocked real increment"); stage = 4;
            } else if stage == 4 && frame.lines().any(|line| line.contains('◀') && line.contains('▶') && line.contains("1%")) {
                sender.send(provider_filter_key(KeyCode::Esc)).expect("cancel unchanged owner editor"); stage = 5;
            } else if stage == 5 && !frame.contains("Weekly floor") {
                sender.send(provider_filter_key(KeyCode::Esc)).expect("exit"); stage = 6;
            }
        }).collect::<Vec<_>>();
    let frames = tokio::time::timeout(Duration::from_secs(2), render).await.expect("bounded wide sidecar Help mouse journey");
    assert!(frames.iter().any(|frame| frame.lines().any(|line| line.contains('◀') && line.contains('▶') && line.contains("1%"))), "unblocked increment must still act");
    assert_eq!(saves.load(std::sync::atomic::Ordering::SeqCst), 0, "Help and arrow browsing never save");
}

#[test]
fn quota_status_selected_panel_renders_weekly_before_five_hour_window() {
    let text = render_quota_static_capture(quota_view_model(), 160, false);
    let lines = text.lines().collect::<Vec<_>>();
    let quota_windows_index = lines
        .iter()
        .position(|line| line.contains("Quota windows"))
        .unwrap_or_else(|| panic!("selected panel should render quota windows:\n{text}"));
    let weekly_index = lines
        .iter()
        .enumerate()
        .skip(quota_windows_index + 1)
        .find_map(|(index, line)| line.contains("weekly").then_some(index))
        .unwrap_or_else(|| panic!("selected panel should render weekly quota:\n{text}"));
    let five_hour_index = lines
        .iter()
        .enumerate()
        .skip(quota_windows_index + 1)
        .find_map(|(index, line)| line.contains("5h").then_some(index))
        .unwrap_or_else(|| panic!("selected panel should render 5h quota:\n{text}"));

    assert!(weekly_index < five_hour_index, "{text}");
}
#[test]
fn quota_status_selected_panel_spaces_activity_header_after_5h_pace() {
    let text = render_quota_static_capture(quota_view_model(), 160, false);
    let lines = text.lines().collect::<Vec<_>>();
    let conn_line_index = lines
        .iter()
        .rposition(|line| line.contains("conn"))
        .unwrap_or_else(|| panic!("conn line should render:\n{text}"));
    let short_pace_line = lines
        .get(conn_line_index + 1)
        .unwrap_or_else(|| panic!("5h pace should follow conn:\n{text}"));
    let spacer_line = lines
        .get(conn_line_index + 2)
        .unwrap_or_else(|| panic!("5h pace should have a following spacer:\n{text}"));
    let activity_line = lines
        .get(conn_line_index + 3)
        .unwrap_or_else(|| panic!("activity should follow 5h pace spacer:\n{text}"));

    assert!(short_pace_line.contains("5h"), "{text}");
    assert!(short_pace_line.contains("runs out 2d 16h"), "{text}");
    assert!(
        spacer_line.contains("│                                                            │"),
        "Activity should remain separated as a header:\n{text}"
    );
    assert!(activity_line.contains("Activity"), "{text}");
}

#[test]
fn quota_status_unavailable_reset_pace_renders_marker_meter() {
    let mut view_model = quota_view_model();
    let unavailable_reset_pace = ResetPaceViewModel::default();
    let selected_details = selected_account_details("ssdev", "safest quota");
    view_model.rows[0].reset_pace = unavailable_reset_pace.clone();
    view_model.rows[0].details = QuotaSelectedAccountViewModel {
        reset_pace: unavailable_reset_pace,
        ..selected_details
    };
    view_model.selected = Some(view_model.rows[0].details.clone());

    let text = render_quota_static_capture(view_model, 160, false);

    assert!(
        text.contains("□□□□□□□│□□□□□□□"),
        "unavailable reset pace must keep the visible center-marker meter:\n{text}"
    );
    assert!(text.contains("burn unavailable"), "{text}");
}

#[test]
fn quota_status_ansi_colors_selected_reset_pace() {
    let view_model = quota_state_color_view_model();
    let text = render_quota_static_capture(view_model, 160, true);

    assert!(
        text.contains("\u{1b}[38;5;10m") && text.contains("1.00x reset pace healthy"),
        "healthy reset pace should render green:\n{text:?}"
    );
}

#[tokio::test]
async fn quota_status_down_arrow_focuses_next_account_details() {
    let frames = element! {
        QuotaStatusComponent(
            view_model: quota_two_account_view_model(),
            width: 120usize,
            height: 48usize,
        )
    }
    .mock_terminal_render_loop(MockTerminalConfig::with_events(futures_util::stream::iter(
        vec![
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Down)),
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Esc)),
        ],
    )))
    .map(|canvas| canvas.to_string())
    .collect::<Vec<_>>()
    .await;

    let text = frames
        .last()
        .cloned()
        .unwrap_or_else(|| panic!("quota status should render at least one frame"));
    assert!(
        text.contains("beta    [usable]    beta detail"),
        "down arrow should show details for the next quota account:\n{text}"
    );
}

#[tokio::test]
async fn quota_status_reloads_view_model_on_timer() {
    let reload_count = Arc::new(AtomicUsize::new(0));
    let reload_view_model: QuotaStatusViewModelLoader = {
        let reload_count = Arc::clone(&reload_count);
        Arc::new(move || {
            let reload_count = Arc::clone(&reload_count);
            Box::pin(async move {
                tokio::task::yield_now().await;
                reload_count.fetch_add(1, Ordering::SeqCst);
                let mut view_model = quota_view_model();
                view_model.route_line = "responses -> beta    [preferred]".to_owned();
                let stale_sample = SampleMetadata {
                    confidence: SampleConfidence::Stale,
                    age_label: "15m 1s".to_owned(),
                    age_seconds: Some(901),
                    semantic_label: "sample stale",
                };
                view_model.rows[0].account = "beta".to_owned();
                view_model.rows[0].sample_metadata = stale_sample.clone();
                view_model.rows[0].details.account = "beta".to_owned();
                view_model.rows[0].details.sample_metadata = stale_sample;
                view_model.selected = Some(view_model.rows[0].details.clone());
                Some(view_model)
            })
        })
    };
    let exit_events = futures_util::stream::unfold(false, |sent| async move {
        if sent {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(35)).await;
        Some((
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Esc)),
            true,
        ))
    });

    let frames = element! {
        QuotaStatusComponent(
            view_model: quota_view_model(),
            width: 120usize,
            height: 48usize,
            reload_view_model,
            reload_interval: Duration::from_millis(10),
            spinner_interval: Duration::from_secs(60),
        )
    }
    .mock_terminal_render_loop(MockTerminalConfig::with_events(exit_events))
    .map(|canvas| canvas.to_string())
    .collect::<Vec<_>>()
    .await;

    assert!(
        reload_count.load(Ordering::SeqCst) > 0,
        "quota status should invoke the reload callback"
    );
    assert!(
        frames
            .iter()
            .any(|frame| frame.contains("responses -> beta    [preferred]")),
        "quota status should render the reloaded route line: {frames:?}"
    );
    assert!(
        frames
            .iter()
            .any(|frame| frame.contains("stale 15m 1s ago")),
        "quota status title should render reloaded stale freshness: {frames:?}"
    );
}

#[tokio::test]
async fn quota_status_up_arrow_focuses_previous_account_details() {
    let frames = element! {
        QuotaStatusComponent(
            view_model: quota_two_account_view_model(),
            width: 120usize,
            height: 48usize,
        )
    }
    .mock_terminal_render_loop(MockTerminalConfig::with_events(futures_util::stream::iter(
        vec![
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Down)),
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Up)),
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Esc)),
        ],
    )))
    .map(|canvas| canvas.to_string())
    .collect::<Vec<_>>()
    .await;

    let text = frames
        .last()
        .cloned()
        .unwrap_or_else(|| panic!("quota status should render at least one frame"));
    assert!(
        text.contains("alpha    [usable]    alpha detail"),
        "up arrow should show details for the previous quota account:\n{text}"
    );
}

#[test]
fn quota_live_width_sample_updates_observed_width_without_resize_event() {
    let mut observed_width = 159;

    assert!(apply_live_terminal_width_sample(
        &mut observed_width,
        Some(160)
    ));
    assert_eq!(observed_width, 160);

    assert!(!apply_live_terminal_width_sample(
        &mut observed_width,
        Some(160)
    ));
    assert_eq!(observed_width, 160);
}

#[tokio::test]
async fn quota_status_renderer_uses_reset_pace_fields_without_parsing_strings() {
    let sample_metadata = SampleMetadata {
        confidence: SampleConfidence::Stale,
        age_label: "15m 1s".to_owned(),
        age_seconds: Some(901),
        semantic_label: "sample stale",
    };
    let reset_pace = ResetPaceViewModel {
        state: ResetPaceState::OverBurning,
        multiple_label: "1.21x reset pace".to_owned(),
        impact_label: None,
        semantic_label: "over",
        meter_left_segments: ResetPaceMeterSegments {
            filled: 0,
            empty: 7,
        },
        meter_right_segments: ResetPaceMeterSegments {
            filled: 3,
            empty: 4,
        },
        center_marker: '│',
        unavailable_reason: Some("conflicting unavailable sentinel".to_owned()),
    };
    let selected_details = QuotaSelectedAccountViewModel {
        sample_metadata: sample_metadata.clone(),
        reset_pace: reset_pace.clone(),
        ..selected_account_details("ssdev", "safest quota")
    };
    let view_model = QuotaStatusViewModel {
        width: 120,
        route_line: "responses -> ssdev    [preferred]".to_owned(),
        why_line: "why: safest quota".to_owned(),
        serving_clients: None,
        rows: vec![QuotaStatusAccountViewModel {
            account_id: test_account_id("ssdev"),
            account_tag: "test-tag".to_owned(),
            active_credential_generation: Some(1),
            enabled: true,
            selected: true,
            account: "ssdev".to_owned(),
            status: "[usable]".to_owned(),
            active_clients: "1 client".to_owned(),
            reset_credits: "2 resets".to_owned(),
            reason: "safest quota".to_owned(),
            weekly_window: "█████ 83%".to_owned(),
            short_window: "█████ 99%".to_owned(),
            burn_meter: "legacy-meter-sentinel".to_owned(),
            sample_metadata,
            reset_pace,
            weekly_pace: "legacy safe pace sentinel".to_owned(),
            details: selected_details.clone(),
        }],
        selected: Some(selected_details),
    };

    let frames = element! {
        QuotaStatusComponent(view_model: view_model, width: 120usize, height: 48usize)
    }
    .mock_terminal_render_loop(MockTerminalConfig::with_events(futures_util::stream::iter(
        vec![TerminalEvent::Key(KeyEvent::new(
            KeyEventKind::Press,
            KeyCode::Esc,
        ))],
    )))
    .map(|canvas| canvas.to_string())
    .collect::<Vec<_>>()
    .await;
    let text = frames
        .last()
        .cloned()
        .unwrap_or_else(|| panic!("quota status should render at least one frame"));

    assert!(text.contains("1.21x reset pace"), "{text}");
    assert!(text.contains("over"), "{text}");
    assert!(text.contains("sample stale 15m 1s"), "{text}");
    assert!(text.contains("│■■■"), "{text}");
    assert!(
        !text.contains("legacy safe pace sentinel")
            && !text.contains("legacy-meter-sentinel")
            && !text.contains("conflicting unavailable sentinel"),
        "renderer must use typed reset-pace/sample fields instead of parsing legacy strings:\n{text}"
    );
}

#[test]
fn quota_status_row_renders_runout_impact_label() {
    let mut view_model = quota_view_model();
    view_model.rows[0].reset_pace = ResetPaceViewModel {
        state: ResetPaceState::OverBurning,
        multiple_label: "3.00x reset pace".to_owned(),
        impact_label: Some("runs out 3h".to_owned()),
        semantic_label: "over",
        meter_left_segments: ResetPaceMeterSegments {
            filled: 0,
            empty: 7,
        },
        meter_right_segments: ResetPaceMeterSegments {
            filled: 7,
            empty: 0,
        },
        center_marker: '│',
        unavailable_reason: None,
    };

    let text = render_quota_static_capture(view_model, 120, false);

    assert!(text.contains("runs out 3h"), "{text}");
    assert!(
        !text.contains("3.00x reset pace over"),
        "runout impact should replace the capped over-pace copy in account rows:\n{text}"
    );
}

#[test]
fn quota_status_list_shows_weekly_5h_and_weekly_forecast() {
    let mut view_model = quota_view_model();
    view_model.rows[0].weekly_window = "██████████ 94% weekly · resets 6d 19h".to_owned();
    view_model.rows[0].short_window = "████████░░ 72% 5h · resets 3h 12m".to_owned();
    view_model.rows[0].reset_pace = ResetPaceViewModel {
        state: ResetPaceState::OverBurning,
        multiple_label: "1.37x pace".to_owned(),
        impact_label: Some("runs out 2d 4h".to_owned()),
        semantic_label: "over",
        meter_left_segments: ResetPaceMeterSegments {
            filled: 0,
            empty: 7,
        },
        meter_right_segments: ResetPaceMeterSegments {
            filled: 5,
            empty: 2,
        },
        center_marker: '│',
        unavailable_reason: None,
    };

    let text = render_quota_static_capture(view_model, 120, false);

    assert!(text.contains("94% weekly · resets 6d 19h"), "{text}");
    assert!(text.contains("72% 5h · resets 3h 12m"), "{text}");
    assert!(text.contains("weekly · runs out 2d 4h"), "{text}");
    assert!(
        !text.contains("Account") && !text.contains("Status") && !text.contains("Pace"),
        "{text}"
    );

    let weekly_line = text
        .lines()
        .find(|line| line.contains("94% weekly · resets 6d 19h"))
        .unwrap_or_else(|| panic!("weekly account-list line should render:\n{text}"));
    let forecast_line = text
        .lines()
        .find(|line| line.contains("weekly · runs out 2d 4h"))
        .unwrap_or_else(|| panic!("weekly account-list forecast should render:\n{text}"));
    assert_eq!(
        weekly_line.find("██████████"),
        forecast_line.find("□□□□□□□│"),
        "weekly capacity and forecast meters must share the Pace-column origin:\n{text}"
    );
}

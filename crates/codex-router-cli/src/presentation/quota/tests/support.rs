async fn render_quota_capture(width: usize) -> String {
    render_quota_capture_model_at(
        quota_view_model(),
        width,
        MIN_RENDER_HEIGHT,
        vec![TerminalEvent::Key(KeyEvent::new(
            KeyEventKind::Press,
            KeyCode::Esc,
        ))],
    )
    .await
}

async fn render_quota_capture_model_at(
    view_model: QuotaStatusViewModel,
    width: usize,
    height: usize,
    events: Vec<TerminalEvent>,
) -> String {
    let frames = render_quota_capture_frames(view_model, width, height, events).await;
    frames
        .last()
        .cloned()
        .unwrap_or_else(|| panic!("quota status should render at least one frame"))
}

async fn render_quota_capture_frames(
    view_model: QuotaStatusViewModel,
    width: usize,
    height: usize,
    events: Vec<TerminalEvent>,
) -> Vec<String> {
    element! {
        QuotaStatusComponent(
            view_model,
            width,
            height,
        )
    }
    .mock_terminal_render_loop(MockTerminalConfig::with_events(futures_util::stream::iter(
        events,
    )))
    .map(|canvas| canvas.to_string())
    .collect::<Vec<_>>()
    .await
}

pub(super) fn assert_quota_golden(name: &str, actual: &str) {
    let normalized = normalize_quota_golden(actual);
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden/quota")
        .join(format!("{name}.txt"));
    if std::env::var_os("UPDATE_QUOTA_GOLDENS").as_deref() == Some(std::ffi::OsStr::new("1")) {
        std::fs::create_dir_all(path.parent().expect("golden path should have a parent"))
            .unwrap_or_else(|error| panic!("quota golden directory should be writable: {error}"));
        std::fs::write(&path, &normalized)
            .unwrap_or_else(|error| panic!("quota golden should be writable: {error}"));
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "missing quota golden {} ({error}); run with UPDATE_QUOTA_GOLDENS=1",
            path.display()
        )
    });
    assert_eq!(
        normalized,
        expected,
        "quota golden drift: {}",
        path.display()
    );
}

fn normalize_quota_golden(actual: &str) -> String {
    let mut normalized = String::with_capacity(actual.len());
    let mut characters = actual.chars().peekable();
    while let Some(character) = characters.next() {
        if character == '\u{1b}' && characters.peek() == Some(&'[') {
            characters.next();
            for sequence_character in characters.by_ref() {
                if ('@'..='~').contains(&sequence_character) {
                    break;
                }
            }
            continue;
        }
        if matches!(
            character,
            '⠋' | '⠙' | '⠹' | '⠸' | '⠼' | '⠴' | '⠦' | '⠧' | '⠇' | '⠏'
        ) {
            normalized.push('◌');
        } else {
            normalized.push(character);
        }
    }
    normalized
}

fn meaningful_line_count(text: &str) -> usize {
    text.lines().count()
}

fn visible_quota_account_count(text: &str) -> usize {
    text.lines()
        .filter(|line| line.contains("acct") && !line.contains("Selected account"))
        .count()
}

fn capture_dir() -> PathBuf {
    let dir = std::env::var_os("CODEX_ROUTER_CAPTURE_DIR").map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tmp/ux-proof/production"),
        PathBuf::from,
    );
    std::fs::create_dir_all(&dir)
        .unwrap_or_else(|error| panic!("capture dir should be writable: {error}"));
    dir
}

fn write_capture_pair(dir: &Path, name: &str, text: &str) {
    std::fs::write(dir.join(format!("{name}.txt")), text)
        .unwrap_or_else(|error| panic!("text capture should write: {error}"));
    std::fs::write(dir.join(format!("{name}.svg")), terminal_svg(name, text))
        .unwrap_or_else(|error| panic!("svg capture should write: {error}"));
}

fn terminal_svg(title: &str, text: &str) -> String {
    let lines = text.lines().collect::<Vec<_>>();
    let width = lines
        .iter()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(1);
    let height = lines.len().max(1);
    let pixel_width = width * 9 + 32;
    let pixel_height = height * 18 + 34;
    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{pixel_width}\" height=\"{pixel_height}\" viewBox=\"0 0 {pixel_width} {pixel_height}\"><rect width=\"100%\" height=\"100%\" fill=\"#111318\"/>"
    );
    svg.push_str(&format!(
            "<text x=\"16\" y=\"24\" xml:space=\"preserve\" font-family=\"SFMono-Regular, Menlo, Consolas, monospace\" font-size=\"14\" fill=\"#e6edf3\"><tspan>{}</tspan>",
            escape_xml(title)
        ));
    for (index, line) in lines.iter().enumerate() {
        svg.push_str(&format!(
            "<tspan x=\"16\" dy=\"{}\">{}</tspan>",
            if index == 0 { 20 } else { 18 },
            escape_xml(line)
        ));
    }
    svg.push_str("</text></svg>");
    svg
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn render_quota_static_capture(
    view_model: QuotaStatusViewModel,
    width: usize,
    ansi: bool,
) -> String {
    let mut output = Vec::new();
    write_quota_status_view(
        &mut output,
        QuotaStatusViewModel {
            width,
            ..view_model
        },
        ansi,
    )
    .unwrap_or_else(|error| panic!("quota status should render: {error}"));
    String::from_utf8(output)
        .unwrap_or_else(|error| panic!("quota status should render utf8: {error}"))
}

fn has_quota_sidecar_details(text: &str) -> bool {
    text.lines()
        .any(|line| line.matches('┌').count() >= 2 && line.matches('┐').count() >= 2)
}

fn quota_view_model() -> QuotaStatusViewModel {
    let selected_details = selected_account_details("ssdev", "safest quota");
    QuotaStatusViewModel {
        width: 100,
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
            weekly_window: "█████ 83% left, reset 7d".to_owned(),
            short_window: "█████ 99% left, reset 5h".to_owned(),
            burn_meter: "■□□□".to_owned(),
            sample_metadata: SampleMetadata {
                confidence: SampleConfidence::Fresh,
                age_label: "14s".to_owned(),
                age_seconds: Some(14),
                semantic_label: "sample fresh",
            },
            reset_pace: ResetPaceViewModel {
                state: ResetPaceState::Healthy,
                multiple_label: "1.00x reset pace".to_owned(),
                impact_label: None,
                semantic_label: "healthy",
                meter_left_segments: ResetPaceMeterSegments {
                    filled: 0,
                    empty: 7,
                },
                meter_right_segments: ResetPaceMeterSegments {
                    filled: 0,
                    empty: 7,
                },
                center_marker: '│',
                unavailable_reason: None,
            },
            weekly_pace: "ahead reset by 2d".to_owned(),
            details: selected_details.clone(),
        }],
        selected: Some(selected_details),
    }
}

fn quota_empty_view_model() -> QuotaStatusViewModel {
    QuotaStatusViewModel {
        width: 100,
        route_line: "responses -> none    [empty]".to_owned(),
        why_line: "why: no accounts configured".to_owned(),
        serving_clients: None,
        rows: Vec::new(),
        selected: None,
    }
}

fn quota_error_view_model() -> QuotaStatusViewModel {
    QuotaStatusViewModel {
        width: 100,
        route_line: "responses -> unavailable    [error]".to_owned(),
        why_line: "why: quota status unavailable".to_owned(),
        serving_clients: None,
        rows: Vec::new(),
        selected: None,
    }
}

pub(super) fn quota_two_account_view_model() -> QuotaStatusViewModel {
    let alpha_details = selected_account_details("alpha", "alpha detail");
    let beta_details = selected_account_details("beta", "beta detail");
    QuotaStatusViewModel {
        width: 120,
        route_line: "responses -> alpha    [preferred]".to_owned(),
        why_line: "why: alpha detail".to_owned(),
        serving_clients: None,
        rows: vec![
            QuotaStatusAccountViewModel {
                account_id: test_account_id("alpha"),
                account_tag: "alpha-tag".to_owned(),
                active_credential_generation: Some(1),
                enabled: true,
                selected: true,
                account: "alpha".to_owned(),
                status: "[usable]".to_owned(),
                active_clients: "1 client".to_owned(),
                reset_credits: "2 resets".to_owned(),
                reason: "alpha detail".to_owned(),
                weekly_window: "█████ 83% left, reset 7d".to_owned(),
                short_window: "█████ 99% left, reset 5h".to_owned(),
                burn_meter: "■□□□".to_owned(),
                sample_metadata: SampleMetadata::default(),
                reset_pace: ResetPaceViewModel::default(),
                weekly_pace: "ahead reset by 2d".to_owned(),
                details: alpha_details.clone(),
            },
            QuotaStatusAccountViewModel {
                account_id: test_account_id("beta"),
                account_tag: "beta-tag".to_owned(),
                active_credential_generation: Some(1),
                enabled: true,
                selected: false,
                account: "beta".to_owned(),
                status: "[usable]".to_owned(),
                active_clients: "0 clients".to_owned(),
                reset_credits: "2 resets".to_owned(),
                reason: "beta detail".to_owned(),
                weekly_window: "████ 75% left, reset 6d".to_owned(),
                short_window: "████ 70% left, reset 4h".to_owned(),
                burn_meter: "■■□□".to_owned(),
                sample_metadata: SampleMetadata::default(),
                reset_pace: ResetPaceViewModel::default(),
                weekly_pace: "behind reset by 1d".to_owned(),
                details: beta_details,
            },
        ],
        selected: Some(alpha_details),
    }
}

fn quota_many_account_view_model() -> QuotaStatusViewModel {
    let selected_details = selected_account_details("acct00", "primary");
    let mut rows = Vec::new();
    for index in 0..10 {
        let account = format!("acct{index:02}");
        let details = selected_account_details(&account, &format!("account {index:02} detail"));
        rows.push(QuotaStatusAccountViewModel {
            account_id: test_account_id(&account),
            account_tag: format!("tag-{index:02}"),
            active_credential_generation: Some(1),
            enabled: true,
            selected: index == 0,
            account,
            status: "[usable]".to_owned(),
            active_clients: format!("{index} clients"),
            reset_credits: "2 resets".to_owned(),
            reason: format!("account {index:02} detail"),
            weekly_window: "█████ 83% left, reset 7d".to_owned(),
            short_window: "█████ 99% left, reset 5h".to_owned(),
            burn_meter: "■□□□".to_owned(),
            sample_metadata: SampleMetadata::default(),
            reset_pace: ResetPaceViewModel::default(),
            weekly_pace: "ahead reset by 2d".to_owned(),
            details,
        });
    }
    QuotaStatusViewModel {
        width: 160,
        route_line: "responses -> acct00    [preferred]".to_owned(),
        why_line: "why: primary".to_owned(),
        serving_clients: None,
        rows,
        selected: Some(selected_details),
    }
}

fn quota_no_authoritative_selection_view_model() -> QuotaStatusViewModel {
    let mut view_model = quota_view_model();
    view_model.route_line = "responses -> none    [blocked]    no selectable account".to_owned();
    view_model.why_line = "why: no usable accounts".to_owned();
    view_model.selected = None;
    for row in &mut view_model.rows {
        row.selected = false;
        row.status = "[blocked]".to_owned();
        row.reason = "quota ineligible".to_owned();
        row.weekly_window = "░░░░░░░░░░ 0% left, reset 7d".to_owned();
        row.reset_pace = ResetPaceViewModel::default();
        row.details.status = "[blocked]".to_owned();
        row.details.reason = "quota ineligible".to_owned();
        row.details.weekly_window = "░░░░░░░░░░ 0% left, reset 7d".to_owned();
        row.details.reset_pace = ResetPaceViewModel::default();
        row.details.total_rate = "rate unknown".to_owned();
        row.details.connection_rate = "not measured (unknown)".to_owned();
        row.details.guards = "5h 100% / weekly 100%".to_owned();
        row.details.note = "quota ineligible".to_owned();
    }
    view_model
}

fn selected_account_details(account: &str, reason: &str) -> QuotaSelectedAccountViewModel {
    QuotaSelectedAccountViewModel {
        account: account.to_owned(),
        status: "[usable]".to_owned(),
        reason: reason.to_owned(),
        short_window: "█████ 99% left, reset 5h".to_owned(),
        weekly_window: "████ 83% left, reset 7d".to_owned(),
        burn_meter: "■□□□".to_owned(),
        burn_pace: "ahead reset by 2d".to_owned(),
        sample_metadata: SampleMetadata {
            confidence: SampleConfidence::Fresh,
            age_label: "14s".to_owned(),
            age_seconds: Some(14),
            semantic_label: "sample fresh",
        },
        reset_pace: ResetPaceViewModel {
            state: ResetPaceState::Healthy,
            multiple_label: "1.00x reset pace".to_owned(),
            impact_label: None,
            semantic_label: "healthy",
            meter_left_segments: ResetPaceMeterSegments {
                filled: 0,
                empty: 7,
            },
            meter_right_segments: ResetPaceMeterSegments {
                filled: 0,
                empty: 7,
            },
            center_marker: '│',
            unavailable_reason: None,
        },
        short_reset_pace: ResetPaceViewModel {
            state: ResetPaceState::OverBurning,
            multiple_label: "2.50x reset pace".to_owned(),
            impact_label: Some("runs out 2d 16h".to_owned()),
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
        },
        total_rate: "0.10%/h".to_owned(),
        connection_rate: "0.05%/h/conn".to_owned(),
        active_clients: "1 client".to_owned(),
        guards: "5h 0% / weekly 8%".to_owned(),
        reset: "2 available".to_owned(),
        note: reason.to_owned(),
    }
}

fn quota_state_color_view_model() -> QuotaStatusViewModel {
    let healthy_details = selected_account_details("healthy", "healthy");
    QuotaStatusViewModel {
        width: 160,
        route_line: "responses -> healthy    [preferred]".to_owned(),
        why_line: "why: reset pace colors".to_owned(),
        serving_clients: None,
        rows: vec![
            quota_state_color_row("healthy", true, ResetPaceState::Healthy, "1.00x reset pace"),
            quota_state_color_row(
                "under",
                false,
                ResetPaceState::UnderBurning,
                "0.50x reset pace",
            ),
            quota_state_color_row(
                "over",
                false,
                ResetPaceState::OverBurning,
                "1.50x reset pace",
            ),
        ],
        selected: Some(healthy_details),
    }
}

fn quota_state_color_row(
    account: &str,
    selected: bool,
    state: ResetPaceState,
    multiple_label: &str,
) -> QuotaStatusAccountViewModel {
    let semantic_label = match state {
        ResetPaceState::UnderBurning => "under",
        ResetPaceState::Healthy => "healthy",
        ResetPaceState::OverBurning => "over",
        ResetPaceState::Unavailable => "burn unavailable",
    };
    let meter_segments = if matches!(
        state,
        ResetPaceState::Healthy | ResetPaceState::UnderBurning
    ) {
        (
            ResetPaceMeterSegments {
                filled: 0,
                empty: 7,
            },
            ResetPaceMeterSegments {
                filled: 0,
                empty: 7,
            },
        )
    } else {
        (
            ResetPaceMeterSegments {
                filled: 0,
                empty: 7,
            },
            ResetPaceMeterSegments {
                filled: 4,
                empty: 3,
            },
        )
    };
    let reset_pace = ResetPaceViewModel {
        state,
        multiple_label: multiple_label.to_owned(),
        impact_label: None,
        semantic_label,
        meter_left_segments: meter_segments.0,
        meter_right_segments: meter_segments.1,
        center_marker: '│',
        unavailable_reason: None,
    };
    QuotaStatusAccountViewModel {
        account_id: test_account_id(account),
        account_tag: format!("{account}-tag"),
        active_credential_generation: Some(1),
        enabled: true,
        selected,
        account: account.to_owned(),
        status: "[usable]".to_owned(),
        active_clients: "0 clients".to_owned(),
        reset_credits: "2 resets".to_owned(),
        reason: semantic_label.to_owned(),
        weekly_window: "█████ 83% left, reset 7d".to_owned(),
        short_window: "█████ 99% left, reset 5h".to_owned(),
        burn_meter: String::new(),
        sample_metadata: SampleMetadata {
            confidence: SampleConfidence::Fresh,
            age_label: "0s".to_owned(),
            age_seconds: Some(0),
            semantic_label: "sample fresh",
        },
        reset_pace: reset_pace.clone(),
        weekly_pace: String::new(),
        details: QuotaSelectedAccountViewModel {
            reset_pace,
            ..selected_account_details(account, semantic_label)
        },
    }
}

fn test_account_id(value: &str) -> AccountId {
    AccountId::new(format!("test-{value}"))
        .unwrap_or_else(|error| panic!("test account id should be valid: {error}"))
}

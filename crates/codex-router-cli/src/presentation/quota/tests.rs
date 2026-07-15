use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;

use codex_router_core::ids::AccountId;
use futures_util::StreamExt;
use iocraft::prelude::*;

use super::component::*;
use super::*;

#[tokio::test]
async fn quota_status_uses_sidecar_only_at_160_columns() {
    let stacked_text = render_quota_capture(159).await;
    assert!(
        !has_quota_sidecar_details(&stacked_text),
        "quota status should stack details below 160 columns:\n{stacked_text}"
    );

    let sidecar_text = render_quota_capture(160).await;
    assert!(
        has_quota_sidecar_details(&sidecar_text),
        "quota status should place details on the right at 160 columns:\n{sidecar_text}"
    );
}

#[test]
fn quota_focus_identity_survives_duplicate_labels_reorder_and_insertion() {
    let mut view_model = quota_two_account_view_model();
    view_model.rows[0].account = "duplicate".to_owned();
    view_model.rows[1].account = "duplicate".to_owned();
    let focused_account_id = view_model.rows[1].account_id.clone();

    view_model.rows.swap(0, 1);
    view_model.rows.insert(0, quota_view_model().rows.remove(0));

    assert_eq!(
        focused_row_index_for_account(&view_model.rows, Some(&focused_account_id)),
        Some(1)
    );
}

#[test]
fn quota_focus_identity_has_no_render_index_when_account_is_removed() {
    let mut view_model = quota_two_account_view_model();
    let focused_account_id = view_model.rows[1].account_id.clone();
    view_model
        .rows
        .retain(|row| row.account_id != focused_account_id);

    assert_eq!(
        focused_row_index_for_account(&view_model.rows, Some(&focused_account_id)),
        None
    );
    assert_eq!(
        moved_quota_focus_index(None, view_model.rows.len(), QuotaFocusMove::Next),
        Some(0)
    );
    assert_eq!(
        moved_quota_focus_index(None, view_model.rows.len(), QuotaFocusMove::Previous),
        Some(view_model.rows.len() - 1)
    );
}

#[test]
fn quota_focus_identity_does_not_change_when_credential_generation_changes() {
    let mut view_model = quota_two_account_view_model();
    let focused_account_id = view_model.rows[1].account_id.clone();
    view_model.rows[1].active_credential_generation = Some(42);

    assert_eq!(
        focused_row_index_for_account(&view_model.rows, Some(&focused_account_id)),
        Some(1)
    );
    assert_eq!(view_model.rows[1].active_credential_generation, Some(42));
}

#[tokio::test]
async fn quota_browse_matches_normalized_goldens() {
    let ordinary_exit = vec![TerminalEvent::Key(KeyEvent::new(
        KeyEventKind::Press,
        KeyCode::Esc,
    ))];
    let cases = [
        (
            "width-48",
            render_quota_capture_model_at(quota_view_model(), 48, 48, ordinary_exit.clone()).await,
        ),
        (
            "width-100-height-24",
            render_quota_capture_model_at(
                quota_many_account_view_model(),
                100,
                24,
                ordinary_exit.clone(),
            )
            .await,
        ),
        (
            "width-159",
            render_quota_capture_model_at(quota_view_model(), 159, 24, ordinary_exit.clone()).await,
        ),
        (
            "width-160",
            render_quota_capture_model_at(quota_view_model(), 160, 24, ordinary_exit.clone()).await,
        ),
        (
            "clipped-short-height",
            render_quota_capture_model_at(
                quota_many_account_view_model(),
                160,
                12,
                ordinary_exit.clone(),
            )
            .await,
        ),
        (
            "empty",
            render_quota_capture_model_at(quota_empty_view_model(), 100, 24, ordinary_exit.clone())
                .await,
        ),
        (
            "error",
            render_quota_capture_model_at(quota_error_view_model(), 100, 24, ordinary_exit.clone())
                .await,
        ),
        (
            "ordinary-exit",
            render_quota_capture_model_at(quota_view_model(), 120, 24, ordinary_exit).await,
        ),
    ];

    for (name, actual) in cases {
        assert_quota_golden(name, &actual);
    }

    let resize_frames = render_quota_capture_frames(
        quota_view_model(),
        0,
        0,
        vec![
            TerminalEvent::Resize(159, 24),
            TerminalEvent::Resize(160, 24),
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Esc)),
        ],
    )
    .await;
    let resize_transcript = resize_frames
        .iter()
        .enumerate()
        .map(|(index, frame)| format!("=== frame {index} ===\n{frame}"))
        .collect::<Vec<_>>()
        .join("\n");
    assert_quota_golden("resize", &resize_transcript);
}

#[tokio::test]
async fn quota_status_reflows_when_terminal_width_changes() {
    let frames = element! {
        QuotaStatusComponent(
            view_model: quota_view_model(),
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
        frames.iter().any(|frame| !has_quota_sidecar_details(frame)),
        "quota status should render a stacked frame after shrinking below 160 columns: {frames:?}"
    );
    assert!(
        frames.iter().any(|frame| has_quota_sidecar_details(frame)),
        "quota status should render a sidecar frame after growing to 160 columns: {frames:?}"
    );
}

#[tokio::test]
async fn quota_status_renders_minimum_height_from_short_resize() {
    let text = render_quota_capture_model_at(
        quota_view_model(),
        0,
        0,
        vec![
            TerminalEvent::Resize(160, 12),
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Esc)),
        ],
    )
    .await;

    assert_eq!(
        meaningful_line_count(&text),
        24,
        "short terminals should still render the 24-row quota minimum:\n{text}"
    );
}

#[tokio::test]
async fn quota_status_uses_taller_height_for_account_rows() {
    let short_text = render_quota_capture_model_at(
        quota_many_account_view_model(),
        160,
        24,
        vec![TerminalEvent::Key(KeyEvent::new(
            KeyEventKind::Press,
            KeyCode::Esc,
        ))],
    )
    .await;
    let tall_text = render_quota_capture_model_at(
        quota_many_account_view_model(),
        160,
        32,
        vec![TerminalEvent::Key(KeyEvent::new(
            KeyEventKind::Press,
            KeyCode::Esc,
        ))],
    )
    .await;

    let short_rows = visible_quota_account_count(&short_text);
    let tall_rows = visible_quota_account_count(&tall_text);
    assert!(
        tall_rows > short_rows,
        "taller quota view should spend height on more account rows; short={short_rows}, tall={tall_rows}\nshort:\n{short_text}\ntall:\n{tall_text}"
    );
}

#[tokio::test]
async fn quota_status_keeps_focused_account_visible_when_height_clips_list() {
    let text = render_quota_capture_model_at(
        quota_many_account_view_model(),
        160,
        24,
        vec![
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Down)),
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Down)),
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Down)),
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Down)),
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Down)),
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Down)),
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Esc)),
        ],
    )
    .await;

    assert!(
        text.contains("❯ acct06"),
        "focused quota account should stay visible when height clips the list:\n{text}"
    );
    assert!(
        text.contains("more above"),
        "clipped focused quota view should expose above-window context:\n{text}"
    );
}

#[tokio::test]
async fn quota_status_preserves_selected_panel_at_stacked_minimum_height() {
    let text = render_quota_capture_model_at(
        quota_many_account_view_model(),
        100,
        24,
        vec![TerminalEvent::Key(KeyEvent::new(
            KeyEventKind::Press,
            KeyCode::Esc,
        ))],
    )
    .await;

    assert!(
        text.contains("Selected account"),
        "stacked 100x24 quota view should preserve the selected panel:\n{text}"
    );
    assert!(
        text.contains("❯ acct00"),
        "stacked 100x24 quota view should still show a focused account row:\n{text}"
    );
}

#[tokio::test]
async fn quota_status_removes_panel_top_padding_and_dead_tail() {
    let text = render_quota_capture_model_at(
        quota_view_model(),
        160,
        24,
        vec![TerminalEvent::Key(KeyEvent::new(
            KeyEventKind::Press,
            KeyCode::Esc,
        ))],
    )
    .await;
    let lines = text.lines().collect::<Vec<_>>();
    let sidecar_top_border_index = lines
        .iter()
        .position(|line| line.matches('┌').count() >= 2)
        .unwrap_or_else(|| panic!("quota sidecar panels should render:\n{text}"));
    let first_panel_content = lines
        .get(sidecar_top_border_index + 1)
        .unwrap_or_else(|| panic!("quota panel content should follow top border:\n{text}"));
    assert!(
        first_panel_content.contains("ssdev") && first_panel_content.contains("Selected account"),
        "first account and selected header should sit directly below panel borders:\n{text}"
    );
    assert!(
        !lines
            .iter()
            .rev()
            .skip(1)
            .take(3)
            .any(|line| line.trim_matches(['│', '╰', '╯', ' ', '─']).is_empty()),
        "quota inner panels should not leave a dead blank tail near the bottom border:\n{text}"
    );
}

#[tokio::test]
#[ignore = "writes visual quota presentation capture artifacts for design review"]
async fn quota_status_capture_artifacts_for_design_review() {
    let capture_dir = capture_dir();
    for (width, height) in [(160, 24), (160, 32), (100, 24)] {
        let text = render_quota_capture_model_at(
            quota_many_account_view_model(),
            width,
            height,
            vec![TerminalEvent::Key(KeyEvent::new(
                KeyEventKind::Press,
                KeyCode::Esc,
            ))],
        )
        .await;
        write_capture_pair(&capture_dir, &format!("quota-{width}x{height}"), &text);
    }
}

#[test]
fn quota_status_without_authoritative_selection_shows_focused_account_details() {
    let view_model = quota_no_authoritative_selection_view_model();

    let text = render_quota_static_capture(view_model, 160, false);

    assert!(text.contains("Selected account"), "{text}");
    assert!(
        text.contains("ssdev    [blocked]    quota ineligible"),
        "{text}"
    );
    assert!(
        !text.contains("No selectable account"),
        "blocked quota status should still expose focused account details:\n{text}"
    );
}

#[tokio::test]
async fn quota_status_stacked_without_authoritative_selection_shows_focused_account_details() {
    let text = render_quota_capture_model_at(
        quota_no_authoritative_selection_view_model(),
        100,
        24,
        vec![TerminalEvent::Key(KeyEvent::new(
            KeyEventKind::Press,
            KeyCode::Esc,
        ))],
    )
    .await;

    assert!(text.contains("Selected account"), "{text}");
    assert!(
        text.contains("ssdev    [blocked]    quota ineligible"),
        "{text}"
    );
    assert!(
        !text.contains("No selectable account"),
        "stacked blocked quota status should still expose focused account details:\n{text}"
    );
}

#[test]
fn quota_status_static_output_uses_natural_height_without_tui_padding() {
    let view_model = quota_no_authoritative_selection_view_model();
    let natural_height = quota_static_render_height(
        &QuotaStatusViewModel {
            width: 120,
            serving_clients: None,
            ..view_model.clone()
        },
        120,
    );
    let text = render_quota_static_capture(view_model, 120, false);

    assert_eq!(
        meaningful_line_count(&text),
        natural_height,
        "static quota output should use natural content height instead of the interactive viewport minimum:\n{text}"
    );
    assert!(text.contains("Selected account"), "{text}");
    assert!(
        text.contains("ssdev    [blocked]    quota ineligible"),
        "{text}"
    );
}

#[tokio::test]
async fn quota_status_narrow_rows_preserve_quota_windows_and_forecast() {
    let text = render_quota_capture_model_at(
        quota_view_model(),
        48,
        48,
        vec![TerminalEvent::Key(KeyEvent::new(
            KeyEventKind::Press,
            KeyCode::Esc,
        ))],
    )
    .await;

    assert!(text.contains("weekly"), "{text}");
    assert!(text.contains("5h"), "{text}");
    assert!(text.contains("□□□□□□□│"), "{text}");
    assert!(
        text.contains("1 client") && text.contains("2 resets"),
        "{text}"
    );
}

#[test]
fn quota_status_static_narrow_rows_preserve_quota_windows_and_forecast() {
    let text = render_quota_static_capture(quota_view_model(), 48, false);

    assert!(text.contains("weekly"), "{text}");
    assert!(text.contains("5h"), "{text}");
    assert!(text.contains("□□□□□□□│"), "{text}");
    assert!(
        text.contains("1 client") && text.contains("2 resets"),
        "{text}"
    );
}

#[test]
fn quota_status_title_right_aligns_live_freshness() {
    let text = render_quota_static_capture(quota_view_model(), 120, false);
    let title_line = text
        .lines()
        .find(|line| line.contains("Quota status"))
        .unwrap_or_else(|| panic!("quota title line should render:\n{text}"));

    assert!(title_line.contains("Quota status"), "{text}");
    assert!(title_line.contains("fresh 14s ago"), "{text}");
    assert!(
        !title_line.contains("fresh ok") && !title_line.contains("sample fresh"),
        "title should show compact freshness, not refresh-status or sample copy:\n{text}"
    );
}

#[test]
fn quota_status_title_shows_serving_spinner_when_active_clients_exist() {
    let mut view_model = quota_view_model();
    view_model.serving_clients = Some(1);

    let text = render_quota_static_capture(view_model, 120, false);
    let title_line = text
        .lines()
        .find(|line| line.contains("Quota status"))
        .unwrap_or_else(|| panic!("quota title line should render:\n{text}"));

    assert!(title_line.contains("serving 1 client"), "{text}");
    assert!(title_line.contains("fresh 14s ago"), "{text}");
}

#[test]
fn quota_status_title_uses_row_freshness_when_all_accounts_are_exhausted() {
    let mut view_model = quota_view_model();
    view_model.route_line = "responses -> none    [blocked]".to_owned();
    view_model.why_line = "why: no usable accounts".to_owned();
    view_model.rows[0].selected = false;
    view_model.rows[0].status = "blocked".to_owned();
    view_model.selected = None;

    let text = render_quota_static_capture(view_model, 120, false);
    let title_line = text
        .lines()
        .find(|line| line.contains("Quota status"))
        .unwrap_or_else(|| panic!("quota title line should render:\n{text}"));

    assert!(title_line.contains("fresh 14s ago"), "{text}");
    assert!(!title_line.contains("unknown"), "{text}");
}

#[test]
fn quota_status_selected_panel_renders_5h_after_conn_before_activity() {
    let text = render_quota_static_capture(quota_view_model(), 160, false);
    let reset_pace_index = text
        .find("Reset pace")
        .unwrap_or_else(|| panic!("selected panel should render reset pace:\n{text}"));
    let short_pace_index = text
        .find("5h        □□□□□□□│■■■■■■■  runs out 2d 16h")
        .unwrap_or_else(|| panic!("selected panel should render 5h after conn:\n{text}"));
    let activity_index = text
        .find("Activity")
        .unwrap_or_else(|| panic!("selected panel should render activity:\n{text}"));

    assert!(
        reset_pace_index < short_pace_index && short_pace_index < activity_index,
        "5h reset pace should sit inside Reset pace before Activity:\n{text}"
    );
    assert!(
        !text.contains("5h pace"),
        "5h reset pace should not render a separate section header:\n{text}"
    );
}

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
            active_credential_generation: Some(1),
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

fn assert_quota_golden(name: &str, actual: &str) {
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
            active_credential_generation: Some(1),
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

fn quota_two_account_view_model() -> QuotaStatusViewModel {
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
                active_credential_generation: Some(1),
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
                active_credential_generation: Some(1),
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
            active_credential_generation: Some(1),
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
        active_credential_generation: Some(1),
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

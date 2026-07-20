use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;

use codex_router_core::ids::AccountId;
use crossterm::event::MouseButton;
use futures_util::StreamExt;
use iocraft::prelude::*;

use super::quota_status_component::*;
use super::quota_status_entrypoint::*;
use super::responsive_quota_layout::*;
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
async fn quota_browse_matches_canonical_responsive_goldens() {
    let ordinary_exit = vec![TerminalEvent::Key(KeyEvent::new(
        KeyEventKind::Press,
        KeyCode::Esc,
    ))];
    let cases = [
        (
            "browse-narrow-width-48",
            render_quota_capture_model_at(quota_view_model(), 48, 48, ordinary_exit.clone()).await,
        ),
        (
            "browse-stacked-width-159",
            render_quota_capture_model_at(quota_view_model(), 159, 24, ordinary_exit.clone()).await,
        ),
        (
            "browse-sidecar-width-160",
            render_quota_capture_model_at(quota_view_model(), 160, 24, ordinary_exit.clone()).await,
        ),
        (
            "browse-clipped-height-12",
            render_quota_capture_model_at(
                quota_many_account_view_model(),
                160,
                12,
                ordinary_exit.clone(),
            )
            .await,
        ),
    ];

    for (name, actual) in cases {
        assert_quota_golden(name, &actual);
    }

}

#[tokio::test]
async fn quota_browse_empty_and_error_states_are_structurally_explicit() {
    let exit = vec![TerminalEvent::Key(KeyEvent::new(
        KeyEventKind::Press,
        KeyCode::Esc,
    ))];
    let empty = render_quota_capture_model_at(quota_empty_view_model(), 100, 24, exit.clone()).await;
    let error = render_quota_capture_model_at(quota_error_view_model(), 100, 24, exit).await;

    assert!(empty.contains("responses -> none    [empty]"), "{empty}");
    assert!(empty.contains("No selectable account"), "{empty}");
    assert!(error.contains("responses -> unavailable    [error]"), "{error}");
    assert!(error.contains("No selectable account"), "{error}");
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

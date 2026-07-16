use crossterm::terminal;

use super::component::MIN_QUOTA_WIDTH;
use super::component::QUOTA_STATUS_SPINNER_TICKS;
use super::model::QuotaStatusAccountViewModel;
use super::model::QuotaStatusViewModel;
use super::model::SampleConfidence;
use super::render::fit_line;

pub(super) fn quota_title_line(
    view_model: &QuotaStatusViewModel,
    width: usize,
    spinner_tick: usize,
) -> String {
    let title = "Quota status";
    let status = quota_title_status(view_model, spinner_tick);
    let title_width = title.chars().count();
    let status_width = status.chars().count();
    if title_width + status_width + 1 > width {
        return fit_line(title, width);
    }
    format!(
        "{title}{}{status}",
        " ".repeat(width - title_width - status_width)
    )
}

fn quota_title_status(view_model: &QuotaStatusViewModel, spinner_tick: usize) -> String {
    let spinner = quota_spinner_tick(spinner_tick);
    let freshness = quota_title_freshness(view_model);
    if let Some(serving_clients) = view_model.serving_clients.filter(|clients| *clients > 0) {
        return format!(
            "{spinner} serving {}  {freshness}",
            serving_client_count_label(serving_clients)
        );
    }
    format!("{spinner} {freshness}")
}

fn quota_title_freshness(view_model: &QuotaStatusViewModel) -> String {
    let metadata = view_model
        .selected
        .as_ref()
        .map(|selected| &selected.sample_metadata)
        .or_else(|| {
            view_model
                .rows
                .iter()
                .find(|row| row.selected)
                .map(|row| &row.sample_metadata)
        })
        .or_else(|| {
            view_model
                .rows
                .iter()
                .map(|row| &row.sample_metadata)
                .filter(|metadata| metadata.confidence != SampleConfidence::Unknown)
                .min_by_key(|metadata| metadata.age_seconds.unwrap_or(u64::MAX))
        });
    match metadata.map(|metadata| metadata.confidence) {
        Some(SampleConfidence::Fresh) => {
            let age = metadata
                .map(|metadata| metadata.age_label.as_str())
                .filter(|age| !age.is_empty())
                .unwrap_or("unknown");
            format!("fresh {age} ago")
        }
        Some(SampleConfidence::Stale) => {
            let age = metadata
                .map(|metadata| metadata.age_label.as_str())
                .filter(|age| !age.is_empty())
                .unwrap_or("unknown");
            format!("stale {age} ago")
        }
        Some(SampleConfidence::Unknown) | None => "unknown".to_owned(),
    }
}

fn serving_client_count_label(serving_clients: u32) -> String {
    if serving_clients == 1 {
        "1 client".to_owned()
    } else {
        format!("{serving_clients} clients")
    }
}

fn quota_spinner_tick(tick: usize) -> &'static str {
    QUOTA_STATUS_SPINNER_TICKS
        .get(tick % QUOTA_STATUS_SPINNER_TICKS.len())
        .copied()
        .unwrap_or("⠋")
}

pub(super) fn quota_status_height(height: usize) -> usize {
    height.max(1)
}

pub(super) fn quota_body_budget(height: usize) -> usize {
    height.max(1).saturating_sub(5)
}

pub(super) fn quota_account_list_height(
    row_count: usize,
    focused_row_index: Option<usize>,
    visible_rows: usize,
) -> usize {
    let visible_rows = visible_rows.min(row_count);
    let window_start = visible_account_window_start(focused_row_index, row_count, visible_rows);
    let visible_count = row_count.saturating_sub(window_start).min(visible_rows);
    let remaining = row_count.saturating_sub(window_start + visible_rows);
    let row_height = visible_count * 4;
    let row_gap_height = visible_count.saturating_sub(1);
    let more_above_height = if window_start > 0 { 2 } else { 0 };
    let more_below_height = if remaining > 0 { 2 } else { 0 };
    2 + more_above_height + row_height + row_gap_height + more_below_height
}

pub(super) fn selected_detail_height(has_selected_details: bool) -> usize {
    if has_selected_details { 21 } else { 3 }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum QuotaFocusMove {
    Previous,
    Next,
}

pub(super) fn focused_row_index_for_account(
    rows: &[QuotaStatusAccountViewModel],
    focused_account_id: Option<&codex_router_core::ids::AccountId>,
) -> Option<usize> {
    let focused_account_id = focused_account_id?;
    rows.iter()
        .position(|row| &row.account_id == focused_account_id)
}

pub(super) fn moved_quota_focus_index(
    current_index: Option<usize>,
    row_count: usize,
    movement: QuotaFocusMove,
) -> Option<usize> {
    if row_count == 0 {
        return None;
    }
    let Some(current_index) = current_index else {
        return Some(match movement {
            QuotaFocusMove::Previous => row_count - 1,
            QuotaFocusMove::Next => 0,
        });
    };
    let next_index = match movement {
        QuotaFocusMove::Previous => current_index.saturating_sub(1),
        QuotaFocusMove::Next => (current_index + 1).min(row_count - 1),
    };
    (next_index != current_index).then_some(next_index)
}

pub(super) fn quota_visible_account_budget(
    row_count: usize,
    focused_row_index: Option<usize>,
    available_height: usize,
) -> usize {
    if row_count == 0 {
        return 0;
    }
    (1..=row_count)
        .rev()
        .find(|candidate| {
            quota_account_list_height(row_count, focused_row_index, *candidate) <= available_height
        })
        .unwrap_or(1)
}

pub(super) fn visible_account_window_start(
    focused_row_index: Option<usize>,
    row_count: usize,
    visible_rows: usize,
) -> usize {
    if row_count == 0 || visible_rows == 0 || row_count <= visible_rows {
        return 0;
    }
    focused_row_index
        .unwrap_or(0)
        .min(row_count - 1)
        .saturating_add(1)
        .saturating_sub(visible_rows)
        .min(row_count.saturating_sub(visible_rows))
}

pub(super) fn current_terminal_width() -> Option<usize> {
    terminal::size().ok().map(|(width, _)| usize::from(width))
}

pub(super) fn apply_live_terminal_width_sample(
    observed_width: &mut usize,
    terminal_width: Option<usize>,
) -> bool {
    let Some(terminal_width) = terminal_width else {
        return false;
    };
    let terminal_width = terminal_width.max(MIN_QUOTA_WIDTH);
    if *observed_width == terminal_width {
        return false;
    }
    *observed_width = terminal_width;
    true
}

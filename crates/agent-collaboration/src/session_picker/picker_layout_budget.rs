//! Picker layout budget.
use super::{MIN_STACKED_DETAILS_HEIGHT, SessionsPickerModel};

const MIN_STACKED_LIST_HEIGHT: usize = 8;

pub(super) fn picker_body_budget(
    height: usize,
    control_height: usize,
    footer_line_count: usize,
    minimum_render_height: usize,
) -> usize {
    let root_border_height = 2;
    let title_height = 1;
    let footer_border_height = 1;
    let footer_height = footer_border_height + footer_line_count;
    height
        .max(minimum_render_height)
        .saturating_sub(root_border_height + title_height + control_height + footer_height)
}

pub(super) fn session_visible_row_budget(
    model: &SessionsPickerModel,
    available_height: usize,
) -> usize {
    let visible_len = model.visible_len();
    if visible_len == 0 {
        return 0;
    }
    for candidate in (1..=visible_len).rev() {
        if session_list_height(
            visible_len,
            model.focused_window_start(candidate),
            candidate,
        ) <= available_height
        {
            return candidate;
        }
    }
    1
}

pub(super) fn session_list_height(
    visible_len: usize,
    window_start: usize,
    visible_rows: usize,
) -> usize {
    let visible_rows = visible_rows.max(1);
    let window_end = (window_start + visible_rows).min(visible_len);
    let visible_count = window_end.saturating_sub(window_start);
    let remaining = visible_len.saturating_sub(window_start + visible_rows);

    let header_height = 2;
    let record_rows_height = (window_start..window_end)
        .map(session_choice_row_height)
        .sum::<usize>();
    let record_gap_height = visible_count.saturating_sub(1);
    let more_above_height = if window_start > 0 { 2 } else { 0 };
    let more_below_height = if remaining > 0 { 2 } else { 0 };
    let list_border_and_padding_height = 2;

    list_border_and_padding_height
        + header_height
        + more_above_height
        + record_rows_height
        + record_gap_height
        + more_below_height
}

pub(super) fn stacked_panel_heights(available_height: usize) -> Option<(usize, usize)> {
    let details_height = available_height.saturating_mul(2).div_ceil(5);
    let list_height = available_height.saturating_sub(details_height);
    if details_height < MIN_STACKED_DETAILS_HEIGHT || list_height < MIN_STACKED_LIST_HEIGHT {
        return None;
    }

    Some((list_height, details_height))
}

const fn session_choice_row_height(visible_index: usize) -> usize {
    if visible_index == 0 { 4 } else { 2 }
}

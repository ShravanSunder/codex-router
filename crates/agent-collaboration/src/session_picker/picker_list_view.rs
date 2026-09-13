//! Picker list view.
use super::{
    InteractiveSessionChoiceRow, SessionPickerRecord, SessionsPickerModel, SessionsPickerOutcome,
    compact_age, fit_line, truncate_end,
};
use iocraft::prelude::*;

const STATUS_COLUMN_WIDTH: usize = 6;
const AGE_COLUMN_WIDTH: usize = 6;

fn session_column_widths(width: usize) -> (usize, usize, usize) {
    let inner_width = width.saturating_sub(2);
    let status_width = STATUS_COLUMN_WIDTH;
    let age_width = if width < 43 { 3 } else { AGE_COLUMN_WIDTH };
    let fixed_width = 2 + status_width + (age_width * 2) + 3;
    let title_width = inner_width.saturating_sub(fixed_width).max(2);
    (title_width, status_width, age_width)
}

pub(super) fn render_session_list(
    model: &SessionsPickerModel,
    mut model_state: State<SessionsPickerModel>,
    mut selected_outcome: State<Option<SessionsPickerOutcome>>,
    width: usize,
    visible_rows: usize,
    panel_height: usize,
) -> AnyElement<'static> {
    let row_width = width.saturating_sub(6).max(24);
    let mut rows = vec![render_session_header(row_width)];
    let visible_len = model.visible_len();
    let focused_index = model.focused_visible_index();
    let visible_rows = visible_rows.max(1);
    let window_start = model.focused_window_start(visible_rows);
    if window_start > 0 {
        rows.push(
            element! {
                Text(
                    content: format!("+{window_start} more above"),
                    color: Color::DarkGrey,
                    weight: Weight::Light,
                )
            }
            .into_any(),
        );
        rows.push(list_gap());
    }
    let window_end = (window_start + visible_rows).min(visible_len);
    for visible_index in window_start..window_end {
        if visible_index > window_start {
            rows.push(list_gap());
        }
        if visible_index == 0 {
            let row = render_start_new_row(model, visible_index == focused_index, row_width);
            rows.push(
                element! {
                    InteractiveSessionChoiceRow(
                        focus_handler: move |_| {
                            let should_update_focus = {
                                let model_value = model_state.read();
                                model_value.focused_session_id().is_some()
                                    || model_value.focused_window_start(visible_rows) != window_start
                            };
                            if should_update_focus {
                                model_state.write().focus_start_new_in_window(window_start);
                            }
                        },
                        activation_handler: move |_| {
                            selected_outcome.set(Some(SessionsPickerOutcome::StartNewSession));
                        },
                        activates_on_click: true,
                    ) {
                        #(row)
                    }
                }
                .into_any(),
            );
        } else if let Some(record) = model.visible_choice_record_at(visible_index) {
            let session_id = record.session_id.clone();
            let row = render_record_row(record, visible_index == focused_index, row_width);
            rows.push(
                element! {
                    InteractiveSessionChoiceRow(
                        focus_handler: move |_| {
                            let should_update_focus = {
                                let model_value = model_state.read();
                                model_value.focused_session_id() != Some(session_id.as_str())
                                    || model_value.focused_window_start(visible_rows) != window_start
                            };
                            if should_update_focus {
                                let _ = model_state.write().focus_visible_session_in_window(
                                    &session_id,
                                    Some(window_start),
                                );
                            }
                        },
                        activates_on_click: false,
                    ) {
                        #(row)
                    }
                }
                .into_any(),
            );
        }
    }
    let remaining = visible_len.saturating_sub(window_start + visible_rows);
    if remaining > 0 {
        rows.push(list_gap());
        rows.push(
            element! {
                Text(
                    content: format!("+{remaining} more below"),
                    color: Color::DarkGrey,
                    weight: Weight::Light,
                )
            }
            .into_any(),
        );
    }

    element! {
        View(
            width: width as u32,
            height: panel_height as u32,
            flex_direction: FlexDirection::Column,
            border_style: BorderStyle::Single,
            border_color: Color::DarkGrey,
            overflow: Overflow::Hidden,
            padding_left: 1,
            padding_right: 1,
            padding_top: 0,
            padding_bottom: 0,
        ) {
            #(rows)
        }
    }
    .into_any()
}

pub(super) fn render_start_new_row(
    model: &SessionsPickerModel,
    selected: bool,
    width: usize,
) -> AnyElement<'static> {
    let foreground = if selected { Color::White } else { Color::Grey };
    let title_prefix = if selected { "❯ " } else { "  " };
    let inner_width = width.saturating_sub(2);
    let (title_width, status_width, age_width) = session_column_widths(width);
    let metadata_line = if model.visible_record_len() == 0 {
        format!(
            "    {}  {}",
            no_matching_sessions_label(model),
            start_new_args_label(model)
        )
    } else {
        format!("    {}", start_new_args_label(model))
    };
    let second_line = fit_line(&metadata_line, inner_width);

    element! {
        View(
            width: width as u32,
            flex_direction: FlexDirection::Column,
            border_style: BorderStyle::Single,
            border_color: Color::DarkGrey,
            padding_left: 1,
            padding_right: 1,
            padding_top: 0,
            padding_bottom: 0,
        ) {
            View(width: 100pct) {
                View(width: 2) { Text(content: title_prefix, color: if selected { Color::Yellow } else { foreground }, weight: Weight::Bold, wrap: TextWrap::NoWrap) }
                View(width: title_width as u32, overflow: Overflow::Hidden) { Text(content: truncate_end("Start new session", title_width), color: if selected { Color::Yellow } else { foreground }, weight: Weight::Bold, wrap: TextWrap::NoWrap) }
                Text(content: " ")
                View(width: status_width as u32, overflow: Overflow::Hidden) { Text(content: "-", color: foreground, weight: Weight::Bold, wrap: TextWrap::NoWrap) }
                Text(content: " ")
                View(width: age_width as u32, overflow: Overflow::Hidden, justify_content: JustifyContent::FlexEnd) { Text(content: "-", color: foreground, weight: Weight::Bold, wrap: TextWrap::NoWrap) }
                Text(content: " ")
                View(width: age_width as u32, overflow: Overflow::Hidden, justify_content: JustifyContent::FlexEnd) { Text(content: "-", color: foreground, weight: Weight::Bold, wrap: TextWrap::NoWrap) }
            }
            Text(content: second_line, color: Color::Grey, weight: Weight::Light, wrap: TextWrap::NoWrap)
        }
    }
    .into_any()
}

pub(super) fn no_matching_sessions_label(model: &SessionsPickerModel) -> &'static str {
    if model.search.is_empty() {
        "No existing sessions match these filters"
    } else {
        "No matching sessions"
    }
}

pub(super) fn start_new_args_label(model: &SessionsPickerModel) -> String {
    if model.request.new_session_args_display.is_empty() {
        "no extra args".to_owned()
    } else {
        format!("args: {}", model.request.new_session_args_display)
    }
}

pub(super) fn render_session_header(width: usize) -> AnyElement<'static> {
    let (title_width, status_width, age_width) = session_column_widths(width);
    element! {
        View(
            width: width as u32,
            border_style: BorderStyle::Single,
            border_edges: Edges::Bottom,
            border_color: Color::DarkGrey,
            padding_left: 1,
            padding_right: 1,
        ) {
            View(width: 2) { Text(content: "  ", color: Color::Cyan, weight: Weight::Bold) }
            View(width: title_width as u32) { Text(content: "Session", color: Color::Cyan, weight: Weight::Bold) }
            Text(content: " ")
            View(width: status_width as u32, overflow: Overflow::Hidden) { Text(content: "Status", color: Color::Cyan, weight: Weight::Bold) }
            Text(content: " ")
            View(width: age_width as u32, overflow: Overflow::Hidden, justify_content: JustifyContent::FlexEnd) { Text(content: "Upd", color: Color::Cyan, weight: Weight::Bold) }
            Text(content: " ")
            View(width: age_width as u32, overflow: Overflow::Hidden, justify_content: JustifyContent::FlexEnd) { Text(content: "New", color: Color::Cyan, weight: Weight::Bold) }
        }
    }
    .into_any()
}

pub(super) fn list_gap() -> AnyElement<'static> {
    element! {
        View(height: 1) {
            Text(content: "")
        }
    }
    .into_any()
}

pub(super) fn render_record_row(
    record: &SessionPickerRecord,
    selected: bool,
    width: usize,
) -> AnyElement<'static> {
    let foreground = if selected { Color::White } else { Color::Grey };
    let metadata = if selected {
        Color::Grey
    } else {
        Color::DarkGrey
    };
    let title_prefix = if selected { "❯ " } else { "  " };
    let (title_width, status_width, age_width) = session_column_widths(width);
    let title = truncate_end(&record.title, title_width);
    let status = record.runtime_status.icon();
    let cwd = record.cwd.as_deref().unwrap_or("-");
    let metadata_line = if width < 43 {
        format!("    ⎇ {:<10}  {cwd}", record.branch)
    } else {
        format!("    ⎇ {:<12}  📂 {cwd}", record.branch)
    };
    let second_line = fit_line(&metadata_line, width.saturating_sub(2));

    element! {
        View(
            width: width as u32,
            flex_direction: FlexDirection::Column,
            padding_left: 1,
            padding_right: 1,
            padding_top: 0,
            padding_bottom: 0,
        ) {
            View(width: 100pct) {
                View(width: 2) { Text(content: title_prefix, color: if selected { Color::Yellow } else { foreground }, weight: Weight::Bold, wrap: TextWrap::NoWrap) }
                View(width: title_width as u32, overflow: Overflow::Hidden) { Text(content: title, color: if selected { Color::Yellow } else { foreground }, weight: Weight::Bold, wrap: TextWrap::NoWrap) }
                Text(content: " ")
                View(width: status_width as u32, overflow: Overflow::Hidden) { Text(content: status, color: foreground, weight: Weight::Bold, wrap: TextWrap::NoWrap) }
                Text(content: " ")
                View(width: age_width as u32, overflow: Overflow::Hidden, justify_content: JustifyContent::FlexEnd) { Text(content: compact_age(&record.recency), color: foreground, weight: Weight::Bold, wrap: TextWrap::NoWrap) }
                Text(content: " ")
                View(width: age_width as u32, overflow: Overflow::Hidden, justify_content: JustifyContent::FlexEnd) { Text(content: compact_age(&record.created), color: foreground, weight: Weight::Bold, wrap: TextWrap::NoWrap) }
            }
            Text(content: second_line, color: metadata, weight: Weight::Light, wrap: TextWrap::NoWrap)
        }
    }
    .into_any()
}

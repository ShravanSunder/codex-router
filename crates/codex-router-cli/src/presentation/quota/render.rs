use iocraft::prelude::*;

use super::layout::visible_account_window_start;
use super::model::*;

const DETAIL_LABEL_WIDTH: usize = 10;

pub(super) fn render_account_list(
    rows: &[QuotaStatusAccountViewModel],
    width: usize,
    height: usize,
    focused_row_index: Option<usize>,
    visible_rows: usize,
) -> AnyElement<'static> {
    let row_width = width.saturating_sub(4).max(32);
    let mut children = Vec::new();
    let window_start = visible_account_window_start(focused_row_index, rows.len(), visible_rows);
    let compact_overflow_markers =
        super::layout::quota_account_list_height(rows.len(), focused_row_index, visible_rows)
            > height;
    if window_start > 0 {
        children.push(quota_more_marker(format!("+{window_start} more above")));
        if !compact_overflow_markers {
            children.push(quota_gap());
        }
    }
    let window_end = (window_start + visible_rows).min(rows.len());
    for (offset, index) in (window_start..window_end).enumerate() {
        if offset > 0 {
            children.push(quota_gap());
        }
        if let Some(row) = rows.get(index) {
            children.push(render_account_row(
                row,
                row_width,
                focused_row_index == Some(index),
            ));
        }
    }
    let remaining = rows.len().saturating_sub(window_start + visible_rows);
    if remaining > 0 {
        if !compact_overflow_markers {
            children.push(quota_gap());
        }
        children.push(quota_more_marker(format!("+{remaining} more below")));
    }

    element! {
        View(
            width: width as u32,
            height: height as u32,
            flex_direction: FlexDirection::Column,
            border_style: BorderStyle::Single,
            border_color: Color::DarkGrey,
            overflow: Overflow::Hidden,
            padding_left: 1,
            padding_right: 1,
            padding_top: 0,
            padding_bottom: 0,
        ) {
            #(children)
        }
    }
    .into_any()
}

pub(super) fn quota_more_marker(content: String) -> AnyElement<'static> {
    element! {
        Text(content, color: Color::DarkGrey, weight: Weight::Light)
    }
    .into_any()
}

pub(super) fn quota_list_columns(width: usize) -> (usize, usize, usize) {
    let account_width = if width < 74 { 13 } else { 17 };
    let status_width = if width < 74 { 13 } else { 18 };
    let pace_width = width.saturating_sub(account_width + status_width);
    (account_width, status_width, pace_width)
}

pub(super) fn fit_column(value: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    format!("{} ", fit_line(value, width - 1))
}

pub(super) fn render_account_row(
    row: &QuotaStatusAccountViewModel,
    width: usize,
    focused: bool,
) -> AnyElement<'static> {
    let inner_width = width.saturating_sub(2);
    let (account_width, status_width, pace_width) = quota_list_columns(inner_width);
    let marker = if focused { "❯" } else { " " };
    let account = fit_column(&format!("{marker} {}", row.account), account_width);
    let status_color = if focused { Color::Yellow } else { Color::White };
    let metadata_color = if focused { Color::Yellow } else { Color::Grey };
    let reset_sample_pace = reset_pace_row_line(&row.reset_pace, pace_width);

    element! {
        View(
            width: width as u32,
            flex_direction: FlexDirection::Column,
            padding_left: 1,
            padding_right: 1,
        ) {
            View(width: inner_width as u32) {
                Text(content: account, color: if focused { Color::Yellow } else { Color::White }, weight: Weight::Bold, wrap: TextWrap::NoWrap)
                Text(content: fit_column(&row.status, status_width), color: status_color, wrap: TextWrap::NoWrap)
                Text(content: fit_line(&row.reason, pace_width), color: metadata_color, wrap: TextWrap::NoWrap)
            }
            View(width: inner_width as u32) {
                Text(content: " ".repeat(account_width), wrap: TextWrap::NoWrap)
                Text(content: fit_column(&row.active_clients, status_width), color: Color::Grey, wrap: TextWrap::NoWrap)
                Text(content: fit_line(&row.weekly_window, pace_width), color: Color::White, wrap: TextWrap::NoWrap)
            }
            View(width: inner_width as u32) {
                Text(content: " ".repeat(account_width), wrap: TextWrap::NoWrap)
                Text(content: fit_column(&row.reset_credits, status_width), color: Color::Grey, wrap: TextWrap::NoWrap)
                Text(content: fit_line(&row.short_window, pace_width), color: Color::White, wrap: TextWrap::NoWrap)
            }
            View(width: inner_width as u32) {
                Text(content: " ".repeat(account_width), wrap: TextWrap::NoWrap)
                Text(content: " ".repeat(status_width), wrap: TextWrap::NoWrap)
                #(reset_sample_pace)
            }
        }
    }
    .into_any()
}

pub(super) fn render_selected_panel(
    selected: Option<&QuotaSelectedAccountViewModel>,
    width: usize,
    height: usize,
) -> AnyElement<'static> {
    let details = selected
        .map(|selected| render_selected_details(selected, width.saturating_sub(4)))
        .unwrap_or_else(|| {
            element! {
                Text(content: "No selectable account", color: Color::Grey)
            }
            .into_any()
        });

    element! {
        View(
            width: width as u32,
            height: height as u32,
            flex_direction: FlexDirection::Column,
            border_style: BorderStyle::Single,
            border_color: Color::DarkGrey,
            overflow: Overflow::Hidden,
            padding_left: 1,
            padding_right: 1,
            padding_top: 0,
            padding_bottom: 0,
        ) {
            #(details)
        }
    }
    .into_any()
}

pub(super) fn render_selected_details(
    selected: &QuotaSelectedAccountViewModel,
    detail_width: usize,
) -> AnyElement<'static> {
    element! {
        View(
            width: detail_width as u32,
            flex_direction: FlexDirection::Column,
            row_gap: 0,
            padding_top: 0,
        ) {
            Text(content: "Selected account", color: Color::Cyan, weight: Weight::Bold)
            Text(content: fit_line(&format!("{}    {}    {}", selected.account, selected.status, selected.reason), detail_width), color: Color::White, weight: Weight::Bold, wrap: TextWrap::NoWrap)
            Text(content: "Quota windows", color: Color::Cyan, weight: Weight::Bold)
            #(detail_line("weekly", &selected.weekly_window, detail_width, Color::White))
            #(detail_line("5h", &selected.short_window, detail_width, Color::White))
            #(quota_gap())
            Text(content: "Reset pace", color: Color::Cyan, weight: Weight::Bold)
            #(reset_pace_detail_line("weekly", &selected.reset_pace, detail_width))
            #(detail_line("sample", &sample_metadata_summary(&selected.sample_metadata), detail_width, Color::White))
            #(detail_line("rate", &selected.total_rate, detail_width, Color::White))
            #(detail_line("conn", &selected.connection_rate, detail_width, Color::White))
            #(reset_pace_detail_line("5h", &selected.short_reset_pace, detail_width))
            #(quota_gap())
            Text(content: "Activity", color: Color::Cyan, weight: Weight::Bold)
            #(detail_line("clients", &selected.active_clients, detail_width, Color::White))
            #(detail_line("guards", &selected.guards, detail_width, Color::White))
            #(detail_line("reset", &selected.reset, detail_width, Color::White))
            #(detail_line("note", &selected.note, detail_width, Color::Grey))
        }
    }
    .into_any()
}

pub(super) fn reset_pace_detail_line(
    label: &str,
    reset_pace: &ResetPaceViewModel,
    width: usize,
) -> AnyElement<'static> {
    let value_width = width.saturating_sub(DETAIL_LABEL_WIDTH).max(12);
    let reset_pace_text = fit_line(&reset_pace_summary(reset_pace), value_width);
    element! {
        View(width: width as u32) {
            Text(content: fit_line(label, DETAIL_LABEL_WIDTH), color: Color::Grey, wrap: TextWrap::NoWrap)
            MixedText(
                contents: vec![
                    MixedTextContent::new(reset_pace_text).color(reset_pace_color(reset_pace.state)),
                ],
                wrap: TextWrap::NoWrap,
            )
        }
    }
    .into_any()
}

pub(super) fn detail_line(
    label: &str,
    value: &str,
    width: usize,
    color: Color,
) -> AnyElement<'static> {
    let value_width = width.saturating_sub(DETAIL_LABEL_WIDTH).max(12);
    element! {
        View(width: width as u32) {
            Text(content: fit_line(label, DETAIL_LABEL_WIDTH), color: Color::Grey, wrap: TextWrap::NoWrap)
            Text(content: fit_line(value, value_width), color, wrap: TextWrap::NoWrap)
        }
    }
    .into_any()
}

pub(super) fn reset_pace_row_line(
    reset_pace: &ResetPaceViewModel,
    width: usize,
) -> AnyElement<'static> {
    let reset_pace_text = list_pace_summary_for_width(reset_pace, width);
    element! {
        View(width: width as u32) {
            MixedText(
                contents: vec![
                    MixedTextContent::new(reset_pace_text)
                        .color(reset_pace_color(reset_pace.state)),
                ],
                wrap: TextWrap::NoWrap,
            )
        }
    }
    .into_any()
}

pub(super) fn list_pace_summary_for_width(reset_pace: &ResetPaceViewModel, width: usize) -> String {
    let summary = if reset_pace.state == ResetPaceState::Unavailable {
        format!(
            "{}  {}",
            reset_pace_meter(reset_pace),
            reset_pace.semantic_label
        )
    } else if let Some(impact_label) = &reset_pace.impact_label {
        format!("{}  weekly · {impact_label}", reset_pace_meter(reset_pace))
    } else {
        format!(
            "{}  {}",
            reset_pace_meter(reset_pace),
            reset_pace
                .multiple_label
                .replace("reset pace", "weekly pace"),
        )
    };
    fit_line(&summary, width)
}

pub(super) const fn reset_pace_color(state: ResetPaceState) -> Color {
    match state {
        ResetPaceState::UnderBurning => Color::Yellow,
        ResetPaceState::Healthy => Color::Green,
        ResetPaceState::OverBurning => Color::Red,
        ResetPaceState::Unavailable => Color::Grey,
    }
}

pub(super) fn colorize_reset_pace_ansi(text: &str) -> String {
    text.lines()
        .map(colorize_reset_pace_line_ansi)
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn colorize_reset_pace_line_ansi(line: &str) -> String {
    let mut ranges = reset_pace_ansi_ranges(line);
    if ranges.is_empty() {
        return line.to_owned();
    }
    ranges.sort_by_key(|(start, _, _)| *start);
    let mut output = String::new();
    let mut cursor = 0;
    for (start, end, color) in ranges {
        if start < cursor {
            continue;
        }
        output.push_str(line.get(cursor..start).unwrap_or_default());
        output.push_str(color);
        output.push_str(line.get(start..end).unwrap_or_default());
        output.push_str("\u{1b}[0m");
        cursor = end;
    }
    output.push_str(line.get(cursor..).unwrap_or_default());
    output
}

pub(super) fn reset_pace_ansi_ranges(line: &str) -> Vec<(usize, usize, &'static str)> {
    let mut ranges = Vec::new();
    for (needle, color) in [
        (" reset pace healthy", "\u{1b}[38;5;10m"),
        (" reset pace under", "\u{1b}[38;5;11m"),
        (" reset pace over", "\u{1b}[38;5;9m"),
        (" pace healthy", "\u{1b}[38;5;10m"),
        (" pace under", "\u{1b}[38;5;11m"),
        (" pace over", "\u{1b}[38;5;9m"),
        (" weekly runs out", "\u{1b}[38;5;9m"),
        ("Exhausted", "\u{1b}[38;5;9m"),
    ] {
        let mut cursor = 0;
        while let Some(relative_index) = line.get(cursor..).and_then(|suffix| suffix.find(needle)) {
            let phrase_start = cursor + relative_index;
            let end = phrase_start + needle.len();
            let start = if needle == "Exhausted" {
                phrase_start
            } else {
                reset_pace_segment_start(line, phrase_start)
            };
            ranges.push((start, end, color));
            cursor = end;
        }
    }
    ranges
}

pub(super) fn reset_pace_segment_start(line: &str, phrase_start: usize) -> usize {
    let mut cursor = phrase_start;
    cursor = scan_back_while(line, cursor, |character| {
        character.is_ascii_digit() || character == '.' || character == 'x'
    });
    cursor = scan_back_while(line, cursor, char::is_whitespace);
    scan_back_while(line, cursor, |character| {
        matches!(character, '□' | '■' | '│')
    })
}

pub(super) fn scan_back_while(
    line: &str,
    cursor: usize,
    mut predicate: impl FnMut(char) -> bool,
) -> usize {
    let mut start = cursor;
    let Some(prefix) = line.get(..cursor) else {
        return start;
    };
    for (index, character) in prefix.char_indices().rev() {
        if !predicate(character) {
            break;
        }
        start = index;
    }
    start
}

pub(super) fn reset_pace_summary(reset_pace: &ResetPaceViewModel) -> String {
    if reset_pace.state == ResetPaceState::Unavailable {
        return format!(
            "{}  {}",
            reset_pace_meter(reset_pace),
            reset_pace.semantic_label
        );
    }
    if let Some(impact_label) = &reset_pace.impact_label {
        if is_depleted_quota_label(impact_label) {
            return impact_label.clone();
        }
        return format!("{}  {}", reset_pace_meter(reset_pace), impact_label);
    }
    format!(
        "{}  {} {}",
        reset_pace_meter(reset_pace),
        reset_pace.multiple_label,
        reset_pace.semantic_label
    )
}

pub(super) fn is_depleted_quota_label(value: &str) -> bool {
    value == "Exhausted"
}

pub(super) fn reset_pace_meter(reset_pace: &ResetPaceViewModel) -> String {
    reset_pace_meter_slots(
        reset_pace.meter_left_segments.filled,
        reset_pace.center_marker,
        reset_pace.meter_right_segments.filled,
    )
}

pub(super) fn reset_pace_meter_slots(
    left_filled: usize,
    center_marker: char,
    right_filled: usize,
) -> String {
    const RESET_PACE_METER_SIDE_WIDTH: usize = 7;
    const RESET_PACE_METER_EMPTY: char = '□';
    const RESET_PACE_METER_FILLED: char = '■';
    let mut left_slots = [RESET_PACE_METER_EMPTY; RESET_PACE_METER_SIDE_WIDTH];
    let mut right_slots = [RESET_PACE_METER_EMPTY; RESET_PACE_METER_SIDE_WIDTH];
    for slot in left_slots
        .iter_mut()
        .rev()
        .take(left_filled.min(RESET_PACE_METER_SIDE_WIDTH))
    {
        *slot = RESET_PACE_METER_FILLED;
    }
    for slot in right_slots
        .iter_mut()
        .take(right_filled.min(RESET_PACE_METER_SIDE_WIDTH))
    {
        *slot = RESET_PACE_METER_FILLED;
    }

    left_slots
        .into_iter()
        .chain(std::iter::once(center_marker))
        .chain(right_slots)
        .collect()
}

pub(super) fn sample_metadata_summary(sample_metadata: &SampleMetadata) -> String {
    if sample_metadata.confidence == SampleConfidence::Unknown {
        return sample_metadata.semantic_label.to_owned();
    }
    format!(
        "{} {}",
        sample_metadata.semantic_label, sample_metadata.age_label
    )
}

pub(super) fn quota_gap() -> AnyElement<'static> {
    element! {
        View(height: 1) {
            Text(content: "")
        }
    }
    .into_any()
}

pub(super) fn fit_line(value: &str, width: usize) -> String {
    let line = value.replace('\n', " ");
    let char_count = line.chars().count();
    if char_count <= width {
        return format!("{line:<width$}");
    }
    if width <= 1 {
        return "…".to_owned();
    }
    let keep = width - 1;
    format!("{}…", line.chars().take(keep).collect::<String>())
}

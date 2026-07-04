//! Quota status terminal presentation.

use std::io;
use std::io::Write;
use std::time::Duration;

use crossterm::terminal;
use iocraft::prelude::*;

const MIN_QUOTA_WIDTH: usize = 48;
const SIDECAR_QUOTA_WIDTH: usize = 160;
const NARROW_QUOTA_WIDTH: usize = MIN_QUOTA_WIDTH;
const DETAIL_LABEL_WIDTH: usize = 10;
const LIVE_QUOTA_WIDTH_POLL_INTERVAL: Duration = Duration::from_millis(80);

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct QuotaStatusViewModel {
    pub(crate) width: usize,
    pub(crate) route_line: String,
    pub(crate) why_line: String,
    pub(crate) rows: Vec<QuotaStatusAccountViewModel>,
    pub(crate) selected: Option<QuotaSelectedAccountViewModel>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct QuotaStatusAccountViewModel {
    pub(crate) selected: bool,
    pub(crate) account: String,
    pub(crate) status: String,
    pub(crate) active_clients: String,
    pub(crate) reason: String,
    pub(crate) weekly_window: String,
    pub(crate) burn_meter: String,
    pub(crate) sample_metadata: SampleMetadata,
    pub(crate) reset_pace: ResetPaceViewModel,
    pub(crate) weekly_pace: String,
    pub(crate) details: QuotaSelectedAccountViewModel,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum SampleConfidence {
    #[default]
    Unknown,
    Fresh,
    Stale,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SampleMetadata {
    pub(crate) confidence: SampleConfidence,
    pub(crate) age_label: String,
    pub(crate) age_seconds: Option<u64>,
    pub(crate) semantic_label: &'static str,
}

impl Default for SampleMetadata {
    fn default() -> Self {
        Self {
            confidence: SampleConfidence::Unknown,
            age_label: "unknown".to_owned(),
            age_seconds: None,
            semantic_label: "sample unknown",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum ResetPaceState {
    UnderBurning,
    #[default]
    Healthy,
    OverBurning,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct ResetPaceMeterSegments {
    pub(crate) filled: usize,
    pub(crate) empty: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ResetPaceViewModel {
    pub(crate) state: ResetPaceState,
    pub(crate) multiple_label: String,
    pub(crate) semantic_label: &'static str,
    pub(crate) meter_left_segments: ResetPaceMeterSegments,
    pub(crate) meter_right_segments: ResetPaceMeterSegments,
    pub(crate) center_marker: char,
    pub(crate) unavailable_reason: Option<String>,
}

impl Default for ResetPaceViewModel {
    fn default() -> Self {
        Self {
            state: ResetPaceState::Unavailable,
            multiple_label: "burn unavailable".to_owned(),
            semantic_label: "burn unavailable",
            meter_left_segments: ResetPaceMeterSegments {
                filled: 0,
                empty: 7,
            },
            meter_right_segments: ResetPaceMeterSegments {
                filled: 0,
                empty: 7,
            },
            center_marker: '│',
            unavailable_reason: Some("reset pace unavailable".to_owned()),
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct QuotaSelectedAccountViewModel {
    pub(crate) account: String,
    pub(crate) status: String,
    pub(crate) reason: String,
    pub(crate) short_window: String,
    pub(crate) weekly_window: String,
    pub(crate) burn_meter: String,
    pub(crate) burn_pace: String,
    pub(crate) sample_metadata: SampleMetadata,
    pub(crate) reset_pace: ResetPaceViewModel,
    pub(crate) total_rate: String,
    pub(crate) connection_rate: String,
    pub(crate) active_clients: String,
    pub(crate) guards: String,
    pub(crate) reset: String,
    pub(crate) note: String,
}

pub(crate) fn write_quota_status_view(
    writer: &mut impl Write,
    view_model: QuotaStatusViewModel,
    ansi: bool,
) -> io::Result<()> {
    let width = view_model.width.max(MIN_QUOTA_WIDTH);
    let mut element = element! {
        QuotaStatusComponent(view_model: view_model, width: width)
    };
    let canvas = element.render(None);
    if ansi {
        let mut output = Vec::new();
        canvas.write(&mut output)?;
        let text = String::from_utf8(output)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        writer.write_all(colorize_reset_pace_ansi(&text).as_bytes())
    } else {
        canvas.write(writer)
    }
}

pub(crate) fn run_quota_status_view(view_model: QuotaStatusViewModel) -> io::Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(
        element! {
            QuotaStatusComponent(
                view_model: view_model,
                width: 0usize,
            )
        }
        .render_loop()
        .ignore_ctrl_c(),
    )
}

#[derive(Default, Props)]
struct QuotaStatusComponentProps {
    view_model: QuotaStatusViewModel,
    width: usize,
}

#[component]
fn QuotaStatusComponent(
    props: &mut QuotaStatusComponentProps,
    mut hooks: Hooks,
) -> impl Into<AnyElement<'static>> {
    let mut system = hooks.use_context_mut::<SystemContext>();
    let live_terminal_width = props.width == 0;
    let row_count = props.view_model.rows.len();
    let initial_focused_row_index = props
        .view_model
        .rows
        .iter()
        .position(|row| row.selected)
        .unwrap_or(0);
    let observed_width = hooks.use_state(|| {
        current_terminal_width()
            .unwrap_or(props.view_model.width)
            .max(MIN_QUOTA_WIDTH)
    });
    let focused_row_index = hooks.use_state(|| initial_focused_row_index);
    let mut should_exit = hooks.use_state(|| false);
    hooks.use_terminal_events({
        let mut observed_width = observed_width;
        let mut focused_row_index = focused_row_index;
        move |event| match event {
            TerminalEvent::Resize(width, _) if live_terminal_width => {
                let mut width_value = observed_width.get();
                if apply_live_terminal_width_sample(&mut width_value, Some(usize::from(width))) {
                    observed_width.set(width_value);
                }
            }
            TerminalEvent::Key(KeyEvent {
                code,
                kind,
                modifiers,
                ..
            }) => {
                if kind == KeyEventKind::Release {
                    return;
                }
                match code {
                    KeyCode::Up => {
                        let mut index = focused_row_index.get();
                        if move_quota_focus(&mut index, row_count, QuotaFocusMove::Previous) {
                            focused_row_index.set(index);
                        }
                    }
                    KeyCode::Down => {
                        let mut index = focused_row_index.get();
                        if move_quota_focus(&mut index, row_count, QuotaFocusMove::Next) {
                            focused_row_index.set(index);
                        }
                    }
                    KeyCode::Esc | KeyCode::Char('q') => should_exit.set(true),
                    KeyCode::Char('c' | 'd') if modifiers.contains(KeyModifiers::CONTROL) => {
                        should_exit.set(true);
                    }
                    KeyCode::Char('\u{3}' | '\u{4}') => should_exit.set(true),
                    _ => {}
                }
            }
            _ => {}
        }
    });
    hooks.use_future({
        let mut observed_width = observed_width;
        async move {
            if !live_terminal_width {
                return;
            }
            let mut interval = tokio::time::interval(LIVE_QUOTA_WIDTH_POLL_INTERVAL);
            interval.tick().await;
            loop {
                interval.tick().await;
                let mut width_value = observed_width.get();
                if apply_live_terminal_width_sample(&mut width_value, current_terminal_width()) {
                    observed_width.set(width_value);
                }
            }
        }
    });
    if *should_exit.read() {
        system.exit();
    }
    let width = if live_terminal_width {
        observed_width.get()
    } else {
        props.width
    }
    .max(MIN_QUOTA_WIDTH);
    let content_width = width.saturating_sub(4).max(44);
    let focused_row_index_value = focused_row_index_value(focused_row_index.get(), row_count);
    let focused_details = props.view_model.selected.as_ref().and_then(|selected| {
        focused_row_index_value
            .and_then(|index| props.view_model.rows.get(index).map(|row| &row.details))
            .or(Some(selected))
    });
    let list_height = quota_account_list_height(row_count);
    let details_height = selected_detail_height(focused_details.is_some());
    let sidecar = width >= SIDECAR_QUOTA_WIDTH;
    let body_height = if sidecar {
        list_height.max(details_height)
    } else if width >= NARROW_QUOTA_WIDTH {
        list_height + details_height
    } else {
        list_height
    };
    let component_height = quota_status_height(body_height);
    let body = if sidecar {
        let list_width = (content_width.saturating_sub(2) * 3 / 5)
            .max(58)
            .min(content_width.saturating_sub(44));
        let details_width = content_width.saturating_sub(list_width + 2).max(34);
        element! {
            View(width: 100pct, height: body_height as u32) {
                #(render_account_list(&props.view_model.rows, list_width, body_height, focused_row_index_value))
                View(width: 2) { Text(content: "") }
                #(render_selected_panel(focused_details, details_width, body_height))
            }
        }
        .into_any()
    } else if width >= NARROW_QUOTA_WIDTH {
        element! {
            View(width: content_width as u32, flex_direction: FlexDirection::Column) {
                #(render_account_list(&props.view_model.rows, content_width, list_height, focused_row_index_value))
                #(render_selected_panel(focused_details, content_width, details_height))
            }
        }
        .into_any()
    } else {
        render_account_list(
            &props.view_model.rows,
            content_width,
            list_height,
            focused_row_index_value,
        )
    };

    element! {
        View(
            width: width as u32,
            height: component_height as u32,
            border_style: BorderStyle::Round,
            border_color: Color::Cyan,
            overflow: Overflow::Hidden,
            padding_left: 1,
            padding_right: 1,
            padding_top: 0,
            padding_bottom: 0,
            flex_direction: FlexDirection::Column,
        ) {
            Text(content: "Quota status", color: Color::Cyan, weight: Weight::Bold)
            Text(content: fit_line(&props.view_model.route_line, content_width), color: Color::Yellow, weight: Weight::Bold, wrap: TextWrap::NoWrap)
            Text(content: fit_line(&props.view_model.why_line, content_width), color: Color::White, wrap: TextWrap::NoWrap)
            #(body)
        }
    }
}

fn quota_status_height(body_height: usize) -> usize {
    let root_border_height = 2;
    let title_and_summary_height = 3;
    root_border_height + title_and_summary_height + body_height
}

fn quota_account_list_height(row_count: usize) -> usize {
    let header_height = 2;
    let row_height = row_count * 3;
    let row_gap_height = row_count.saturating_sub(1);
    let border_and_padding_height = 4;
    border_and_padding_height + header_height + row_height + row_gap_height
}

fn selected_detail_height(has_selected_details: bool) -> usize {
    if has_selected_details { 20 } else { 4 }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum QuotaFocusMove {
    Previous,
    Next,
}

fn focused_row_index_value(index: usize, row_count: usize) -> Option<usize> {
    if row_count == 0 {
        None
    } else {
        Some(index.min(row_count - 1))
    }
}

fn move_quota_focus(index: &mut usize, row_count: usize, movement: QuotaFocusMove) -> bool {
    let Some(current_index) = focused_row_index_value(*index, row_count) else {
        return false;
    };
    let next_index = match movement {
        QuotaFocusMove::Previous => current_index.saturating_sub(1),
        QuotaFocusMove::Next => (current_index + 1).min(row_count - 1),
    };
    if next_index == current_index {
        return false;
    }
    *index = next_index;
    true
}

fn current_terminal_width() -> Option<usize> {
    terminal::size().ok().map(|(width, _)| usize::from(width))
}

fn apply_live_terminal_width_sample(
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

fn render_account_list(
    rows: &[QuotaStatusAccountViewModel],
    width: usize,
    height: usize,
    focused_row_index: Option<usize>,
) -> AnyElement<'static> {
    let row_width = width.saturating_sub(6).max(32);
    let mut children = vec![render_table_header(row_width)];
    for (index, row) in rows.iter().enumerate() {
        if index > 0 {
            children.push(quota_gap());
        }
        children.push(render_account_row(
            row,
            row_width,
            focused_row_index == Some(index),
        ));
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
            padding_top: 1,
            padding_bottom: 1,
        ) {
            #(children)
        }
    }
    .into_any()
}

fn render_table_header(width: usize) -> AnyElement<'static> {
    let (account_width, status_width, pace_width) = quota_list_columns(width);
    let header = format!(
        "{}{}{}",
        fit_line("  Account", account_width),
        fit_line("Status", status_width),
        fit_line("Weekly pace", pace_width),
    );
    element! {
        View(
            width: width as u32,
            border_style: BorderStyle::Single,
            border_edges: Edges::Bottom,
            border_color: Color::DarkGrey,
        ) {
            Text(content: fit_line(&header, width), color: Color::Cyan, weight: Weight::Bold, wrap: TextWrap::NoWrap)
        }
    }
    .into_any()
}

fn quota_list_columns(width: usize) -> (usize, usize, usize) {
    let account_width = if width < 74 { 15 } else { 17 };
    let status_width = if width < 74 { 14 } else { 18 };
    let pace_width = width.saturating_sub(account_width + status_width).max(16);
    (account_width, status_width, pace_width)
}

fn render_account_row(
    row: &QuotaStatusAccountViewModel,
    width: usize,
    focused: bool,
) -> AnyElement<'static> {
    let (account_width, status_width, pace_width) = quota_list_columns(width);
    let marker = if focused { "❯" } else { " " };
    let account = fit_line(&format!("{marker} {}", row.account), account_width);
    let status_color = if focused { Color::Yellow } else { Color::White };
    let metadata_color = if focused { Color::Yellow } else { Color::Grey };
    let compact_pace = pace_width < 26;
    let compact_status_pace = if compact_pace {
        element! {
            Text(content: fit_line(row.reset_pace.semantic_label, status_width), color: Some(reset_pace_color(row.reset_pace.state)), wrap: TextWrap::NoWrap)
        }
        .into_any()
    } else {
        element! {
            Text(content: fit_line("", status_width), color: Color::Grey, wrap: TextWrap::NoWrap)
        }
        .into_any()
    };
    let reset_sample_pace = if compact_pace {
        element! {
            Text(content: fit_line(sample_metadata_compact_summary(&row.sample_metadata), pace_width), color: metadata_color, wrap: TextWrap::NoWrap)
        }
        .into_any()
    } else {
        reset_pace_row_line(
            &row.reset_pace,
            &row.sample_metadata,
            pace_width,
            metadata_color,
        )
    };

    element! {
        View(
            width: width as u32,
            flex_direction: FlexDirection::Column,
            padding_left: 1,
            padding_right: 1,
        ) {
            View(width: width.saturating_sub(2) as u32) {
                Text(content: account, color: if focused { Color::Yellow } else { Color::White }, weight: Weight::Bold, wrap: TextWrap::NoWrap)
                Text(content: fit_line(&row.status, status_width), color: status_color, wrap: TextWrap::NoWrap)
                Text(content: fit_line(&row.reason, pace_width), color: metadata_color, wrap: TextWrap::NoWrap)
            }
            View(width: width.saturating_sub(2) as u32) {
                Text(content: " ".repeat(account_width), wrap: TextWrap::NoWrap)
                Text(content: fit_line(&row.active_clients, status_width), color: Color::Grey, wrap: TextWrap::NoWrap)
                Text(content: fit_line(&row.weekly_window, pace_width), color: Color::White, wrap: TextWrap::NoWrap)
            }
            View(width: width.saturating_sub(2) as u32) {
                Text(content: " ".repeat(account_width), wrap: TextWrap::NoWrap)
                #(compact_status_pace)
                #(reset_sample_pace)
            }
        }
    }
    .into_any()
}

fn render_selected_panel(
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
            padding_top: 1,
            padding_bottom: 1,
        ) {
            #(details)
        }
    }
    .into_any()
}

fn render_selected_details(
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
            Text(content: fit_line(&format!("{}    {}    {}", selected.account, selected.status, selected.reason), detail_width), color: Color::Yellow, weight: Weight::Bold, wrap: TextWrap::NoWrap)
            #(quota_gap())
            Text(content: "Quota windows", color: Color::Cyan, weight: Weight::Bold)
            #(detail_line("5h", &selected.short_window, detail_width, Color::White))
            #(detail_line("weekly", &selected.weekly_window, detail_width, Color::White))
            #(quota_gap())
            Text(content: "Reset pace", color: Color::Cyan, weight: Weight::Bold)
            #(reset_pace_detail_line("current", &selected.reset_pace, detail_width))
            #(detail_line("sample", &sample_metadata_summary(&selected.sample_metadata), detail_width, Color::White))
            #(detail_line("rate", &selected.total_rate, detail_width, Color::White))
            #(detail_line("conn", &selected.connection_rate, detail_width, Color::White))
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

fn reset_pace_detail_line(
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

fn detail_line(label: &str, value: &str, width: usize, color: Color) -> AnyElement<'static> {
    let value_width = width.saturating_sub(DETAIL_LABEL_WIDTH).max(12);
    element! {
        View(width: width as u32) {
            Text(content: fit_line(label, DETAIL_LABEL_WIDTH), color: Color::Grey, wrap: TextWrap::NoWrap)
            Text(content: fit_line(value, value_width), color, wrap: TextWrap::NoWrap)
        }
    }
    .into_any()
}

fn reset_pace_row_line(
    reset_pace: &ResetPaceViewModel,
    sample_metadata: &SampleMetadata,
    width: usize,
    sample_color: Color,
) -> AnyElement<'static> {
    let sample = sample_metadata_summary(sample_metadata);
    let sample_width = sample.chars().count().min(width / 3).max(12);
    let reset_width = width.saturating_sub(sample_width).saturating_sub(2);
    let reset_pace_text = reset_pace_summary_for_width(reset_pace, reset_width);
    element! {
        View(width: width as u32) {
            MixedText(
                contents: vec![
                    MixedTextContent::new(reset_pace_text)
                        .color(reset_pace_color(reset_pace.state)),
                    MixedTextContent::new("  "),
                    MixedTextContent::new(fit_line(&sample, sample_width)).color(sample_color),
                ],
                wrap: TextWrap::NoWrap,
            )
        }
    }
    .into_any()
}

const fn reset_pace_color(state: ResetPaceState) -> Color {
    match state {
        ResetPaceState::UnderBurning => Color::Yellow,
        ResetPaceState::Healthy => Color::Green,
        ResetPaceState::OverBurning => Color::Red,
        ResetPaceState::Unavailable => Color::Grey,
    }
}

fn colorize_reset_pace_ansi(text: &str) -> String {
    text.lines()
        .map(colorize_reset_pace_line_ansi)
        .collect::<Vec<_>>()
        .join("\n")
}

fn colorize_reset_pace_line_ansi(line: &str) -> String {
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

fn reset_pace_ansi_ranges(line: &str) -> Vec<(usize, usize, &'static str)> {
    let mut ranges = Vec::new();
    for (needle, color) in [
        (" reset pace healthy", "\u{1b}[38;5;10m"),
        (" reset pace under", "\u{1b}[38;5;11m"),
        (" reset pace over", "\u{1b}[38;5;9m"),
    ] {
        let mut cursor = 0;
        while let Some(relative_index) = line.get(cursor..).and_then(|suffix| suffix.find(needle)) {
            let phrase_start = cursor + relative_index;
            let end = phrase_start + needle.len();
            let start = reset_pace_segment_start(line, phrase_start);
            ranges.push((start, end, color));
            cursor = end;
        }
    }
    ranges
}

fn reset_pace_segment_start(line: &str, phrase_start: usize) -> usize {
    let mut cursor = phrase_start;
    cursor = scan_back_while(line, cursor, |character| {
        character.is_ascii_digit() || character == '.' || character == 'x'
    });
    cursor = scan_back_while(line, cursor, char::is_whitespace);
    scan_back_while(line, cursor, |character| {
        matches!(character, '▱' | '▰' | '│')
    })
}

fn scan_back_while(line: &str, cursor: usize, mut predicate: impl FnMut(char) -> bool) -> usize {
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

fn reset_pace_summary_for_width(reset_pace: &ResetPaceViewModel, width: usize) -> String {
    let summary = reset_pace_summary(reset_pace);
    if summary.chars().count() <= width {
        return fit_line(&summary, width);
    }
    if reset_pace.state == ResetPaceState::Unavailable {
        return fit_line(&summary, width);
    }
    fit_line(
        &format!(
            "{} {}",
            reset_pace.multiple_label, reset_pace.semantic_label
        ),
        width,
    )
}

fn reset_pace_summary(reset_pace: &ResetPaceViewModel) -> String {
    if reset_pace.state == ResetPaceState::Unavailable {
        return format!(
            "{}  {}",
            reset_pace_meter(reset_pace),
            reset_pace.semantic_label
        );
    }
    format!(
        "{}  {} {}",
        reset_pace_meter(reset_pace),
        reset_pace.multiple_label,
        reset_pace.semantic_label
    )
}

fn reset_pace_meter(reset_pace: &ResetPaceViewModel) -> String {
    format!(
        "{}{}{}{}{}",
        "▱".repeat(reset_pace.meter_left_segments.empty),
        "▰".repeat(reset_pace.meter_left_segments.filled),
        reset_pace.center_marker,
        "▰".repeat(reset_pace.meter_right_segments.filled),
        "▱".repeat(reset_pace.meter_right_segments.empty),
    )
}

fn sample_metadata_summary(sample_metadata: &SampleMetadata) -> String {
    if sample_metadata.confidence == SampleConfidence::Unknown {
        return sample_metadata.semantic_label.to_owned();
    }
    format!(
        "{} {}",
        sample_metadata.semantic_label, sample_metadata.age_label
    )
}

fn sample_metadata_compact_summary(sample_metadata: &SampleMetadata) -> &'static str {
    match sample_metadata.confidence {
        SampleConfidence::Fresh => "fresh",
        SampleConfidence::Stale => "stale",
        SampleConfidence::Unknown => "unknown",
    }
}

fn quota_gap() -> AnyElement<'static> {
    element! {
        View(height: 1) {
            Text(content: "")
        }
    }
    .into_any()
}

fn fit_line(value: &str, width: usize) -> String {
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

#[cfg(test)]
mod tests {
    use futures_util::StreamExt;
    use iocraft::prelude::*;

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

    #[test]
    fn quota_status_without_authoritative_selection_does_not_show_selected_details() {
        let mut view_model = quota_view_model();
        view_model.route_line =
            "responses -> none    [blocked]    no selectable account".to_owned();
        view_model.why_line = "why: no usable accounts".to_owned();
        view_model.selected = None;
        for row in &mut view_model.rows {
            row.selected = false;
        }

        let text = render_quota_static_capture(view_model, 160, false);

        assert!(text.contains("No selectable account"), "{text}");
        assert!(
            !text.contains("Selected account"),
            "degraded status must not turn a focused row into an authoritative selection:\n{text}"
        );
    }

    #[test]
    fn quota_status_narrow_rows_preserve_reset_and_sample_semantics() {
        let text = render_quota_static_capture(quota_view_model(), 48, false);

        assert!(text.contains("healthy"), "{text}");
        assert!(text.contains("sample fresh"), "{text}");
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
            text.contains("▱▱▱▱▱▱▱│▱▱▱▱▱▱▱"),
            "unavailable reset pace must keep the visible center-marker meter:\n{text}"
        );
        assert!(text.contains("burn unavailable"), "{text}");
    }

    #[test]
    fn quota_status_ansi_colors_reset_pace_by_state() {
        let view_model = quota_state_color_view_model();
        let text = render_quota_static_capture(view_model, 160, true);

        assert!(
            text.contains("\u{1b}[38;5;10m") && text.contains("1.00x reset pace healthy"),
            "healthy reset pace should render green:\n{text:?}"
        );
        assert!(
            text.contains("\u{1b}[38;5;11m") && text.contains("0.50x reset pace under"),
            "under-burning reset pace should render yellow:\n{text:?}"
        );
        assert!(
            text.contains("\u{1b}[38;5;9m") && text.contains("1.50x reset pace over"),
            "over-burning reset pace should render red:\n{text:?}"
        );
    }

    #[tokio::test]
    async fn quota_status_down_arrow_focuses_next_account_details() {
        let frames = element! {
            QuotaStatusComponent(
                view_model: quota_two_account_view_model(),
                width: 120usize,
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
    async fn quota_status_up_arrow_focuses_previous_account_details() {
        let frames = element! {
            QuotaStatusComponent(
                view_model: quota_two_account_view_model(),
                width: 120usize,
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
        let selected_details = selected_account_details("ssdev", "safest quota");
        let view_model = QuotaStatusViewModel {
            width: 120,
            route_line: "responses -> ssdev    [usable]    refreshed ok".to_owned(),
            why_line: "why: safest quota".to_owned(),
            rows: vec![QuotaStatusAccountViewModel {
                selected: true,
                account: "ssdev".to_owned(),
                status: "[usable]".to_owned(),
                active_clients: "1 client".to_owned(),
                reason: "safest quota".to_owned(),
                weekly_window: "weekly █████ 83%".to_owned(),
                burn_meter: "legacy-meter-sentinel".to_owned(),
                sample_metadata: SampleMetadata {
                    confidence: SampleConfidence::Stale,
                    age_label: "15m 1s".to_owned(),
                    age_seconds: Some(901),
                    semantic_label: "sample stale",
                },
                reset_pace: ResetPaceViewModel {
                    state: ResetPaceState::OverBurning,
                    multiple_label: "1.21x reset pace".to_owned(),
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
                },
                weekly_pace: "legacy safe pace sentinel".to_owned(),
                details: selected_details.clone(),
            }],
            selected: Some(selected_details),
        };

        let frames = element! {
            QuotaStatusComponent(view_model: view_model, width: 120usize)
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
        assert!(text.contains("│▰▰▰"), "{text}");
        assert!(
            !text.contains("legacy safe pace sentinel")
                && !text.contains("legacy-meter-sentinel")
                && !text.contains("conflicting unavailable sentinel"),
            "renderer must use typed reset-pace/sample fields instead of parsing legacy strings:\n{text}"
        );
    }

    async fn render_quota_capture(width: usize) -> String {
        let frames = element! {
            QuotaStatusComponent(
                view_model: quota_view_model(),
                width,
            )
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

        frames
            .last()
            .cloned()
            .unwrap_or_else(|| panic!("quota status should render at least one frame"))
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
            route_line: "responses -> ssdev    [usable]    refreshed ok".to_owned(),
            why_line: "why: safest quota".to_owned(),
            rows: vec![QuotaStatusAccountViewModel {
                selected: true,
                account: "ssdev".to_owned(),
                status: "[usable]".to_owned(),
                active_clients: "1 client".to_owned(),
                reason: "safest quota".to_owned(),
                weekly_window: "weekly █████ 83% left, reset 7d".to_owned(),
                burn_meter: "▰▱▱▱".to_owned(),
                sample_metadata: SampleMetadata {
                    confidence: SampleConfidence::Fresh,
                    age_label: "14s".to_owned(),
                    age_seconds: Some(14),
                    semantic_label: "sample fresh",
                },
                reset_pace: ResetPaceViewModel {
                    state: ResetPaceState::Healthy,
                    multiple_label: "1.00x reset pace".to_owned(),
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

    fn quota_two_account_view_model() -> QuotaStatusViewModel {
        let alpha_details = selected_account_details("alpha", "alpha detail");
        let beta_details = selected_account_details("beta", "beta detail");
        QuotaStatusViewModel {
            width: 120,
            route_line: "responses -> alpha    [usable]    refreshed ok".to_owned(),
            why_line: "why: alpha detail".to_owned(),
            rows: vec![
                QuotaStatusAccountViewModel {
                    selected: true,
                    account: "alpha".to_owned(),
                    status: "[usable]".to_owned(),
                    active_clients: "1 client".to_owned(),
                    reason: "alpha detail".to_owned(),
                    weekly_window: "weekly █████ 83% left, reset 7d".to_owned(),
                    burn_meter: "▰▱▱▱".to_owned(),
                    sample_metadata: SampleMetadata::default(),
                    reset_pace: ResetPaceViewModel::default(),
                    weekly_pace: "ahead reset by 2d".to_owned(),
                    details: alpha_details.clone(),
                },
                QuotaStatusAccountViewModel {
                    selected: false,
                    account: "beta".to_owned(),
                    status: "[usable]".to_owned(),
                    active_clients: "0 clients".to_owned(),
                    reason: "beta detail".to_owned(),
                    weekly_window: "weekly ████ 75% left, reset 6d".to_owned(),
                    burn_meter: "▰▰▱▱".to_owned(),
                    sample_metadata: SampleMetadata::default(),
                    reset_pace: ResetPaceViewModel::default(),
                    weekly_pace: "behind reset by 1d".to_owned(),
                    details: beta_details,
                },
            ],
            selected: Some(alpha_details),
        }
    }

    fn selected_account_details(account: &str, reason: &str) -> QuotaSelectedAccountViewModel {
        QuotaSelectedAccountViewModel {
            account: account.to_owned(),
            status: "[usable]".to_owned(),
            reason: reason.to_owned(),
            short_window: "█████ 99% left, reset 5h".to_owned(),
            weekly_window: "████ 83% left, reset 7d".to_owned(),
            burn_meter: "▰▱▱▱".to_owned(),
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
            route_line: "responses -> healthy    [usable]    refreshed ok".to_owned(),
            why_line: "why: reset pace colors".to_owned(),
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
        let meter_segments = if state == ResetPaceState::Healthy {
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
        } else if state == ResetPaceState::UnderBurning {
            (
                ResetPaceMeterSegments {
                    filled: 4,
                    empty: 3,
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
            semantic_label,
            meter_left_segments: meter_segments.0,
            meter_right_segments: meter_segments.1,
            center_marker: '│',
            unavailable_reason: None,
        };
        QuotaStatusAccountViewModel {
            selected,
            account: account.to_owned(),
            status: "[usable]".to_owned(),
            active_clients: "0 clients".to_owned(),
            reason: semantic_label.to_owned(),
            weekly_window: "weekly █████ 83% left, reset 7d".to_owned(),
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
}

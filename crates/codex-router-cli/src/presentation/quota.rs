//! Quota status terminal presentation.

use std::io;
use std::io::Write;
use std::sync::Arc;
use std::time::Duration;

use crossterm::terminal;
use iocraft::prelude::*;

const MIN_QUOTA_WIDTH: usize = 48;
const MIN_RENDER_HEIGHT: usize = 24;
const SIDECAR_QUOTA_WIDTH: usize = 160;
const NARROW_QUOTA_WIDTH: usize = MIN_QUOTA_WIDTH;
const DETAIL_LABEL_WIDTH: usize = 10;
const LIVE_QUOTA_WIDTH_POLL_INTERVAL: Duration = Duration::from_millis(80);
const LIVE_QUOTA_STATUS_RELOAD_INTERVAL: Duration = Duration::from_secs(60);
const LIVE_QUOTA_STATUS_SPINNER_INTERVAL: Duration = Duration::from_millis(120);
const QUOTA_STATUS_SPINNER_TICKS: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub(crate) type QuotaStatusViewModelLoader =
    Arc<dyn Fn() -> Option<QuotaStatusViewModel> + Send + Sync>;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct QuotaStatusViewModel {
    pub(crate) width: usize,
    pub(crate) route_line: String,
    pub(crate) why_line: String,
    pub(crate) serving_clients: Option<u32>,
    pub(crate) rows: Vec<QuotaStatusAccountViewModel>,
    pub(crate) selected: Option<QuotaSelectedAccountViewModel>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct QuotaStatusAccountViewModel {
    pub(crate) selected: bool,
    pub(crate) account: String,
    pub(crate) status: String,
    pub(crate) active_clients: String,
    pub(crate) reset_credits: String,
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
    pub(crate) impact_label: Option<String>,
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
            impact_label: None,
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
    pub(crate) short_reset_pace: ResetPaceViewModel,
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
    let height = quota_static_render_height(&view_model, width);
    let mut element = element! {
        QuotaStatusComponent(view_model: view_model, width: width, height: height)
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

fn quota_static_render_height(view_model: &QuotaStatusViewModel, width: usize) -> usize {
    let row_count = view_model.rows.len();
    let focused_row_index = view_model
        .rows
        .iter()
        .position(|row| row.selected)
        .or_else(|| (row_count > 0).then_some(0));
    let list_height = quota_account_list_height(row_count, focused_row_index, row_count);
    let details_height =
        selected_detail_height(focused_row_index.is_some() || view_model.selected.is_some());
    let body_height = if width >= SIDECAR_QUOTA_WIDTH {
        list_height.max(details_height)
    } else if width >= NARROW_QUOTA_WIDTH {
        list_height + details_height
    } else {
        list_height
    };
    let root_border_height = 2;
    let title_and_summary_height = 2;
    root_border_height + title_and_summary_height + body_height
}

pub(crate) fn run_quota_status_view(
    view_model: QuotaStatusViewModel,
    reload_view_model: Option<QuotaStatusViewModelLoader>,
) -> io::Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(
        element! {
            QuotaStatusComponent(
                view_model: view_model,
                width: 0usize,
                height: 0usize,
                reload_view_model,
                reload_interval: LIVE_QUOTA_STATUS_RELOAD_INTERVAL,
                spinner_interval: LIVE_QUOTA_STATUS_SPINNER_INTERVAL,
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
    height: usize,
    reload_view_model: Option<QuotaStatusViewModelLoader>,
    reload_interval: Duration,
    spinner_interval: Duration,
}

#[component]
fn QuotaStatusComponent(
    props: &mut QuotaStatusComponentProps,
    mut hooks: Hooks,
) -> impl Into<AnyElement<'static>> {
    let mut system = hooks.use_context_mut::<SystemContext>();
    let (terminal_width, terminal_height) = hooks.use_terminal_size();
    let live_terminal_width = props.width == 0;
    let live_terminal_height = props.height == 0 && live_terminal_width;
    let view_model = hooks.use_state(|| props.view_model.clone());
    let row_count = view_model.read().rows.len();
    let initial_focused_row_index = props
        .view_model
        .rows
        .iter()
        .position(|row| row.selected)
        .unwrap_or(0);
    let observed_width = hooks.use_state(|| {
        if live_terminal_width {
            current_terminal_width()
                .or_else(|| Some(usize::from(terminal_width)))
                .unwrap_or(props.view_model.width)
                .max(MIN_QUOTA_WIDTH)
        } else {
            props.width.max(MIN_QUOTA_WIDTH)
        }
    });
    let observed_height = hooks.use_state(|| {
        if live_terminal_height {
            usize::from(terminal_height).max(MIN_RENDER_HEIGHT)
        } else if props.height == 0 {
            MIN_RENDER_HEIGHT
        } else {
            props.height.max(1)
        }
    });
    let focused_row_index = hooks.use_state(|| initial_focused_row_index);
    let spinner_tick = hooks.use_state(|| 0usize);
    let mut should_exit = hooks.use_state(|| false);
    hooks.use_terminal_events({
        let mut observed_width = observed_width;
        let mut observed_height = observed_height;
        let mut focused_row_index = focused_row_index;
        move |event| match event {
            TerminalEvent::Resize(width, height) => {
                if live_terminal_width {
                    let mut width_value = observed_width.get();
                    if apply_live_terminal_width_sample(&mut width_value, Some(usize::from(width)))
                    {
                        observed_width.set(width_value);
                    }
                }
                if live_terminal_height {
                    observed_height.set(usize::from(height).max(MIN_RENDER_HEIGHT));
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
        let mut view_model = view_model;
        let reload_view_model = props.reload_view_model.clone();
        let reload_interval = if props.reload_interval.is_zero() {
            LIVE_QUOTA_STATUS_RELOAD_INTERVAL
        } else {
            props.reload_interval
        };
        async move {
            let Some(reload_view_model) = reload_view_model else {
                return;
            };
            let mut interval = tokio::time::interval(reload_interval);
            interval.tick().await;
            loop {
                interval.tick().await;
                let reload_view_model = reload_view_model.clone();
                if let Ok(Some(next_view_model)) =
                    tokio::task::spawn_blocking(move || reload_view_model()).await
                {
                    view_model.set(next_view_model);
                }
            }
        }
    });
    hooks.use_future({
        let mut spinner_tick = spinner_tick;
        let spinner_interval = if props.spinner_interval.is_zero() {
            LIVE_QUOTA_STATUS_SPINNER_INTERVAL
        } else {
            props.spinner_interval
        };
        async move {
            let mut interval = tokio::time::interval(spinner_interval);
            interval.tick().await;
            loop {
                interval.tick().await;
                spinner_tick += 1;
            }
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
    let height = if live_terminal_height {
        observed_height.get()
    } else if props.height == 0 {
        MIN_RENDER_HEIGHT
    } else {
        props.height.max(1)
    };
    let content_width = width.saturating_sub(4).max(44);
    let view_model = view_model.read();
    let focused_row_index_value = focused_row_index_value(focused_row_index.get(), row_count);
    let focused_details = focused_row_index_value
        .and_then(|index| view_model.rows.get(index).map(|row| &row.details))
        .or(view_model.selected.as_ref());
    let body_budget = quota_body_budget(height);
    let details_content_height = selected_detail_height(focused_details.is_some());
    let sidecar = width >= SIDECAR_QUOTA_WIDTH;
    let stacked_details = !sidecar
        && width >= NARROW_QUOTA_WIDTH
        && (focused_details.is_some() || props.view_model.selected.is_none());
    let details_height = details_content_height.min(body_budget);
    let list_budget = if sidecar {
        body_budget
    } else if stacked_details {
        let minimum_list_height = if row_count == 0 {
            quota_account_list_height(row_count, None, 0)
        } else {
            quota_account_list_height(row_count, focused_row_index_value, 1)
        };
        if minimum_list_height + details_content_height <= body_budget {
            body_budget.saturating_sub(details_content_height)
        } else {
            minimum_list_height.min(body_budget)
        }
    } else {
        body_budget
    };
    let visible_account_budget =
        quota_visible_account_budget(row_count, focused_row_index_value, list_budget);
    let list_height =
        quota_account_list_height(row_count, focused_row_index_value, visible_account_budget);
    let stacked_details_height = if stacked_details {
        body_budget.saturating_sub(list_height)
    } else {
        0
    };
    let show_stacked_details = stacked_details && stacked_details_height > 0;
    let body_height = if sidecar {
        list_height.max(details_height)
    } else if show_stacked_details {
        list_height + stacked_details_height
    } else {
        list_height
    };
    let component_height = quota_status_height(height);
    let body = if sidecar {
        let list_width = (content_width.saturating_sub(2) * 3 / 5)
            .max(58)
            .min(content_width.saturating_sub(44));
        let details_width = content_width.saturating_sub(list_width + 2).max(34);
        element! {
            View(width: 100pct, height: body_height as u32) {
                #(render_account_list(&view_model.rows, list_width, list_height, focused_row_index_value, visible_account_budget))
                View(width: 2) { Text(content: "") }
                #(render_selected_panel(focused_details, details_width, details_height))
            }
        }
        .into_any()
    } else if show_stacked_details {
        element! {
            View(width: content_width as u32, flex_direction: FlexDirection::Column) {
                #(render_account_list(&view_model.rows, content_width, list_height, focused_row_index_value, visible_account_budget))
                #(render_selected_panel(focused_details, content_width, stacked_details_height))
            }
        }
        .into_any()
    } else {
        render_account_list(
            &view_model.rows,
            content_width,
            list_height,
            focused_row_index_value,
            visible_account_budget,
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
            Text(content: quota_title_line(&view_model, content_width, spinner_tick.get()), color: Color::Cyan, weight: Weight::Bold, wrap: TextWrap::NoWrap)
            Text(content: fit_line(&view_model.route_line, content_width), color: Color::White, weight: Weight::Bold, wrap: TextWrap::NoWrap)
            #(body)
        }
    }
}

fn quota_title_line(
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

fn quota_status_height(height: usize) -> usize {
    height.max(1)
}

fn quota_body_budget(height: usize) -> usize {
    let root_border_height = 2;
    let title_and_summary_height = 2;
    height
        .max(1)
        .saturating_sub(root_border_height + title_and_summary_height)
}

fn quota_account_list_height(
    row_count: usize,
    focused_row_index: Option<usize>,
    visible_rows: usize,
) -> usize {
    let visible_rows = visible_rows.min(row_count);
    let window_start = visible_account_window_start(focused_row_index, row_count, visible_rows);
    let visible_count = row_count.saturating_sub(window_start).min(visible_rows);
    let remaining = row_count.saturating_sub(window_start + visible_rows);
    let header_height = 2;
    let row_height = visible_count * 5;
    let row_gap_height = visible_count.saturating_sub(1);
    let more_above_height = if window_start > 0 { 2 } else { 0 };
    let more_below_height = if remaining > 0 { 2 } else { 0 };
    let border_and_padding_height = 2;
    border_and_padding_height
        + header_height
        + more_above_height
        + row_height
        + row_gap_height
        + more_below_height
}

fn selected_detail_height(has_selected_details: bool) -> usize {
    if has_selected_details { 21 } else { 3 }
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

fn quota_visible_account_budget(
    row_count: usize,
    focused_row_index: Option<usize>,
    available_height: usize,
) -> usize {
    if row_count == 0 {
        return 0;
    }
    for candidate in (1..=row_count).rev() {
        if quota_account_list_height(row_count, focused_row_index, candidate) <= available_height {
            return candidate;
        }
    }
    1
}

fn visible_account_window_start(
    focused_row_index: Option<usize>,
    row_count: usize,
    visible_rows: usize,
) -> usize {
    if row_count == 0 || visible_rows == 0 || row_count <= visible_rows {
        return 0;
    }
    let focused_row_index = focused_row_index.unwrap_or(0).min(row_count - 1);
    focused_row_index
        .saturating_add(1)
        .saturating_sub(visible_rows)
        .min(row_count.saturating_sub(visible_rows))
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
    visible_rows: usize,
) -> AnyElement<'static> {
    let row_width = width.saturating_sub(4).max(32);
    let mut children = vec![render_table_header(row_width)];
    let window_start = visible_account_window_start(focused_row_index, rows.len(), visible_rows);
    if window_start > 0 {
        children.push(quota_more_marker(format!("+{window_start} more above")));
        children.push(quota_gap());
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
        children.push(quota_gap());
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

fn quota_more_marker(content: String) -> AnyElement<'static> {
    element! {
        Text(content, color: Color::DarkGrey, weight: Weight::Light)
    }
    .into_any()
}

fn render_table_header(width: usize) -> AnyElement<'static> {
    let (account_width, status_width, pace_width) = quota_list_columns(width);
    let header = format!(
        "{}{}{}",
        fit_line("  Account", account_width),
        fit_line("Status", status_width),
        fit_line("Pace", pace_width),
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
    let account_width = if width < 74 { 13 } else { 17 };
    let status_width = if width < 74 { 13 } else { 18 };
    let pace_width = width.saturating_sub(account_width + status_width);
    (account_width, status_width, pace_width)
}

fn render_account_row(
    row: &QuotaStatusAccountViewModel,
    width: usize,
    focused: bool,
) -> AnyElement<'static> {
    let inner_width = width.saturating_sub(2);
    let (account_width, status_width, pace_width) = quota_list_columns(inner_width);
    let marker = if focused { "❯" } else { " " };
    let account = fit_line(&format!("{marker} {}", row.account), account_width);
    let status_color = if focused { Color::Yellow } else { Color::White };
    let metadata_color = if focused { Color::Yellow } else { Color::Grey };
    let compact = inner_width < 74;
    let weekly_line = if compact {
        element! {
            Text(content: fit_line(&format!("weekly  {}", row.weekly_window), inner_width), color: Color::White, wrap: TextWrap::NoWrap)
        }
        .into_any()
    } else {
        element! {
            View(width: inner_width as u32) {
                Text(content: " ".repeat(account_width), wrap: TextWrap::NoWrap)
                Text(content: fit_line("weekly", status_width), color: Color::Grey, wrap: TextWrap::NoWrap)
                Text(content: fit_line(&row.weekly_window, pace_width), color: Color::White, wrap: TextWrap::NoWrap)
            }
        }
        .into_any()
    };
    let short_window_line = if compact {
        element! {
            Text(content: fit_line(&format!("5h      {}", row.details.short_window), inner_width), color: Color::White, wrap: TextWrap::NoWrap)
        }
        .into_any()
    } else {
        element! {
            View(width: inner_width as u32) {
                Text(content: " ".repeat(account_width), wrap: TextWrap::NoWrap)
                Text(content: fit_line("5h", status_width), color: Color::Grey, wrap: TextWrap::NoWrap)
                Text(content: fit_line(&row.details.short_window, pace_width), color: Color::White, wrap: TextWrap::NoWrap)
            }
        }
        .into_any()
    };
    let forecast_line = if compact {
        element! {
            Text(content: list_pace_summary_for_width(&row.reset_pace, inner_width), color: reset_pace_color(row.reset_pace.state), wrap: TextWrap::NoWrap)
        }
        .into_any()
    } else {
        reset_pace_row_line(&row.reset_pace, pace_width)
    };
    let connection_line = if compact {
        element! {
            Text(content: fit_line(&format!("conn    {} · {}", row.active_clients, row.reset_credits), inner_width), color: Color::Grey, wrap: TextWrap::NoWrap)
        }
        .into_any()
    } else {
        element! {
            View(width: inner_width as u32) {
                Text(content: " ".repeat(account_width), wrap: TextWrap::NoWrap)
                Text(content: fit_line("conn", status_width), color: Color::Grey, wrap: TextWrap::NoWrap)
                Text(content: fit_line(&format!("{} · {}", row.active_clients, row.reset_credits), pace_width), color: Color::Grey, wrap: TextWrap::NoWrap)
            }
        }
        .into_any()
    };

    element! {
        View(
            width: width as u32,
            flex_direction: FlexDirection::Column,
            padding_left: 1,
            padding_right: 1,
        ) {
            View(width: inner_width as u32) {
                Text(content: account, color: if focused { Color::Yellow } else { Color::White }, weight: Weight::Bold, wrap: TextWrap::NoWrap)
                Text(content: fit_line(&row.status, status_width), color: status_color, wrap: TextWrap::NoWrap)
                Text(content: fit_line(&row.reason, pace_width), color: metadata_color, wrap: TextWrap::NoWrap)
            }
            #(weekly_line)
            #(short_window_line)
            #(forecast_line)
            #(connection_line)
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
            padding_top: 0,
            padding_bottom: 0,
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
            Text(content: fit_line(&format!("{}    {}    {}", selected.account, selected.status, selected.reason), detail_width), color: Color::White, weight: Weight::Bold, wrap: TextWrap::NoWrap)
            #(quota_gap())
            Text(content: "Quota windows", color: Color::Cyan, weight: Weight::Bold)
            #(detail_line("5h", &selected.short_window, detail_width, Color::White))
            #(detail_line("weekly", &selected.weekly_window, detail_width, Color::White))
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

fn reset_pace_row_line(reset_pace: &ResetPaceViewModel, width: usize) -> AnyElement<'static> {
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

fn list_pace_summary_for_width(reset_pace: &ResetPaceViewModel, width: usize) -> String {
    let summary = if reset_pace.state == ResetPaceState::Unavailable {
        format!(
            "{}  {}",
            reset_pace_meter(reset_pace),
            reset_pace.semantic_label
        )
    } else if let Some(impact_label) = &reset_pace.impact_label {
        format!("{}  weekly {impact_label}", reset_pace_meter(reset_pace))
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

fn reset_pace_segment_start(line: &str, phrase_start: usize) -> usize {
    let mut cursor = phrase_start;
    cursor = scan_back_while(line, cursor, |character| {
        character.is_ascii_digit() || character == '.' || character == 'x'
    });
    cursor = scan_back_while(line, cursor, char::is_whitespace);
    scan_back_while(line, cursor, |character| {
        matches!(character, '□' | '■' | '│')
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

fn reset_pace_summary(reset_pace: &ResetPaceViewModel) -> String {
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

fn is_depleted_quota_label(value: &str) -> bool {
    value == "Exhausted"
}

fn reset_pace_meter(reset_pace: &ResetPaceViewModel) -> String {
    reset_pace_meter_slots(
        reset_pace.meter_left_segments.filled,
        reset_pace.center_marker,
        reset_pace.meter_right_segments.filled,
    )
}

fn reset_pace_meter_slots(left_filled: usize, center_marker: char, right_filled: usize) -> String {
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

fn sample_metadata_summary(sample_metadata: &SampleMetadata) -> String {
    if sample_metadata.confidence == SampleConfidence::Unknown {
        return sample_metadata.semantic_label.to_owned();
    }
    format!(
        "{} {}",
        sample_metadata.semantic_label, sample_metadata.age_label
    )
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
    use std::path::Path;
    use std::path::PathBuf;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;
    use std::time::Duration;

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
            first_panel_content.contains("Account")
                && first_panel_content.contains("Selected account"),
            "account and selected headers should sit directly below panel borders:\n{text}"
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
                selected: true,
                account: "ssdev".to_owned(),
                status: "[usable]".to_owned(),
                active_clients: "1 client".to_owned(),
                reset_credits: "2 resets".to_owned(),
                reason: "safest quota".to_owned(),
                weekly_window: "█████ 83%".to_owned(),
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
        view_model.rows[0].weekly_window = "██████████ 94% left, resets 6d 19h".to_owned();
        view_model.rows[0].details.short_window = "████████░░ 72% left, resets 3h 12m".to_owned();
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

        assert!(text.contains("Pace"), "{text}");
        assert!(text.contains("weekly"), "{text}");
        assert!(text.contains("5h"), "{text}");
        assert!(text.contains("weekly runs out 2d 4h"), "{text}");
        assert!(!text.contains("Weekly pace"), "{text}");
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
        let frames = element! {
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
        .await;
        frames
            .last()
            .cloned()
            .unwrap_or_else(|| panic!("quota status should render at least one frame"))
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
                selected: true,
                account: "ssdev".to_owned(),
                status: "[usable]".to_owned(),
                active_clients: "1 client".to_owned(),
                reset_credits: "2 resets".to_owned(),
                reason: "safest quota".to_owned(),
                weekly_window: "█████ 83% left, reset 7d".to_owned(),
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
                    selected: true,
                    account: "alpha".to_owned(),
                    status: "[usable]".to_owned(),
                    active_clients: "1 client".to_owned(),
                    reset_credits: "2 resets".to_owned(),
                    reason: "alpha detail".to_owned(),
                    weekly_window: "█████ 83% left, reset 7d".to_owned(),
                    burn_meter: "■□□□".to_owned(),
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
                    reset_credits: "2 resets".to_owned(),
                    reason: "beta detail".to_owned(),
                    weekly_window: "████ 75% left, reset 6d".to_owned(),
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
                selected: index == 0,
                account,
                status: "[usable]".to_owned(),
                active_clients: format!("{index} clients"),
                reset_credits: "2 resets".to_owned(),
                reason: format!("account {index:02} detail"),
                weekly_window: "█████ 83% left, reset 7d".to_owned(),
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
        view_model.route_line =
            "responses -> none    [blocked]    no selectable account".to_owned();
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
            selected,
            account: account.to_owned(),
            status: "[usable]".to_owned(),
            active_clients: "0 clients".to_owned(),
            reset_credits: "2 resets".to_owned(),
            reason: semantic_label.to_owned(),
            weekly_window: "█████ 83% left, reset 7d".to_owned(),
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

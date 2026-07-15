use std::io;
use std::io::Write;
use std::time::Duration;

use crossterm::terminal;
use iocraft::prelude::*;

use super::model::*;
use super::render::*;

const MIN_QUOTA_WIDTH: usize = 48;
pub(super) const MIN_RENDER_HEIGHT: usize = 24;
const SIDECAR_QUOTA_WIDTH: usize = 160;
const NARROW_QUOTA_WIDTH: usize = MIN_QUOTA_WIDTH;
const LIVE_QUOTA_WIDTH_POLL_INTERVAL: Duration = Duration::from_millis(80);
const LIVE_QUOTA_STATUS_RELOAD_INTERVAL: Duration = Duration::from_secs(60);
const LIVE_QUOTA_STATUS_SPINNER_INTERVAL: Duration = Duration::from_millis(120);
const QUOTA_STATUS_SPINNER_TICKS: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

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

pub(super) fn quota_static_render_height(view_model: &QuotaStatusViewModel, width: usize) -> usize {
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

pub(crate) async fn run_quota_status_view(
    view_model: QuotaStatusViewModel,
    reload_view_model: Option<QuotaStatusViewModelLoader>,
) -> io::Result<()> {
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
    .ignore_ctrl_c()
    .await
}

#[derive(Default, Props)]
pub(super) struct QuotaStatusComponentProps {
    pub(super) view_model: QuotaStatusViewModel,
    pub(super) width: usize,
    pub(super) height: usize,
    pub(super) reload_view_model: Option<QuotaStatusViewModelLoader>,
    pub(super) reload_interval: Duration,
    pub(super) spinner_interval: Duration,
}

#[component]
pub(super) fn QuotaStatusComponent(
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
                if let Some(next_view_model) = reload_view_model().await {
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
    let row_height = visible_count * 4;
    let row_gap_height = visible_count.saturating_sub(1);
    let more_above_height = if window_start > 0 { 2 } else { 0 };
    let more_below_height = if remaining > 0 { 2 } else { 0 };
    let border_and_padding_height = 2;
    border_and_padding_height + more_above_height + row_height + row_gap_height + more_below_height
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

pub(super) fn visible_account_window_start(
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

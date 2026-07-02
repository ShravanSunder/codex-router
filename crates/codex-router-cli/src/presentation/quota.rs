//! Quota status terminal presentation.

use std::io;
use std::io::Write;

use iocraft::prelude::*;

const MIN_QUOTA_WIDTH: usize = 48;
const SIDECAR_QUOTA_WIDTH: usize = 112;
const NARROW_QUOTA_WIDTH: usize = 72;
const DETAIL_LABEL_WIDTH: usize = 10;

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
    pub(crate) weekly_pace: String,
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
        canvas.write_ansi(writer)
    } else {
        canvas.write(writer)
    }
}

#[derive(Default, Props)]
struct QuotaStatusComponentProps {
    view_model: QuotaStatusViewModel,
    width: usize,
}

#[component]
fn QuotaStatusComponent(props: &mut QuotaStatusComponentProps) -> impl Into<AnyElement<'static>> {
    let width = props.width.max(MIN_QUOTA_WIDTH);
    let content_width = width.saturating_sub(4).max(44);
    let row_count = props.view_model.rows.len();
    let list_height = quota_account_list_height(row_count);
    let details_height = selected_detail_height(props.view_model.selected.is_some());
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
                #(render_account_list(&props.view_model.rows, list_width, body_height))
                View(width: 2) { Text(content: "") }
                #(render_selected_panel(props.view_model.selected.as_ref(), details_width, body_height))
            }
        }
        .into_any()
    } else if width >= NARROW_QUOTA_WIDTH {
        element! {
            View(width: content_width as u32, flex_direction: FlexDirection::Column) {
                #(render_account_list(&props.view_model.rows, content_width, list_height))
                #(render_selected_panel(props.view_model.selected.as_ref(), content_width, details_height))
            }
        }
        .into_any()
    } else {
        render_account_list(&props.view_model.rows, content_width, list_height)
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

fn render_account_list(
    rows: &[QuotaStatusAccountViewModel],
    width: usize,
    height: usize,
) -> AnyElement<'static> {
    let row_width = width.saturating_sub(6).max(32);
    let mut children = vec![render_table_header(row_width)];
    for (index, row) in rows.iter().enumerate() {
        if index > 0 {
            children.push(quota_gap());
        }
        children.push(render_account_row(row, row_width));
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

fn render_account_row(row: &QuotaStatusAccountViewModel, width: usize) -> AnyElement<'static> {
    let (account_width, status_width, pace_width) = quota_list_columns(width);
    let marker = if row.selected { "❯" } else { " " };
    let account = fit_line(&format!("{marker} {}", row.account), account_width);
    let status_color = if row.selected {
        Color::Yellow
    } else {
        Color::White
    };
    let metadata_color = if row.selected {
        Color::Yellow
    } else {
        Color::Grey
    };
    let pace_line = fit_line(
        &format!("burn {}  {}", row.burn_meter, row.weekly_pace),
        pace_width,
    );

    element! {
        View(
            width: width as u32,
            flex_direction: FlexDirection::Column,
            padding_left: 1,
            padding_right: 1,
        ) {
            View(width: width.saturating_sub(2) as u32) {
                Text(content: account, color: if row.selected { Color::Yellow } else { Color::White }, weight: Weight::Bold, wrap: TextWrap::NoWrap)
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
                Text(content: fit_line("", status_width), color: Color::Grey, wrap: TextWrap::NoWrap)
                Text(content: pace_line, color: metadata_color, wrap: TextWrap::NoWrap)
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
            Text(content: "Burn pace", color: Color::Cyan, weight: Weight::Bold)
            #(detail_line("current", &format!("{}  {}", selected.burn_meter, selected.burn_pace), detail_width, Color::White))
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

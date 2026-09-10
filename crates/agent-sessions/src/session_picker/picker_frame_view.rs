//! Picker frame view.
use super::{
    COMPACT_PICKER_WIDTH, MIN_PICKER_WIDTH, NARROW_PICKER_WIDTH, SIDECAR_PICKER_WIDTH,
    SessionConversationPreview, SessionsPickerModel, SessionsPickerOutcome, fit_line, footer_lines,
    picker_body_budget, render_details, render_session_list, render_start_new_details, root_label,
    runtime_view_label, session_visible_row_budget, sort_label, stacked_panel_heights,
};
use iocraft::prelude::*;
use unicode_width::UnicodeWidthStr;

pub(super) fn render_picker_view(
    model: &SessionsPickerModel,
    model_state: State<SessionsPickerModel>,
    selected_outcome: State<Option<SessionsPickerOutcome>>,
    selected_conversation: Option<&SessionConversationPreview>,
    height: usize,
    minimum_render_height: usize,
) -> Element<'static, View> {
    let content_width = model.width.saturating_sub(4).max(MIN_PICKER_WIDTH);
    let mut filter_controls = render_filter_controls(model, content_width);
    if let Some(notice) = model.runtime_coverage.notice() {
        filter_controls.push(element! {
            Text(content: fit_line(notice, content_width), color: Color::Yellow, wrap: TextWrap::NoWrap)
        }.into_any());
    }
    let control_height = filter_controls.len();
    let footer_lines = footer_lines(content_width, model.show_help);
    let body_budget = picker_body_budget(
        height,
        control_height,
        footer_lines.len(),
        minimum_render_height,
    );
    let mut children = vec![
        element! {
            Text(
                content: "Resume a previous session",
                color: Color::Cyan,
                weight: Weight::Bold,
            )
        }
        .into_any(),
    ];
    children.extend(filter_controls);

    let focused_record = model.focused_record();
    if model.width >= SIDECAR_PICKER_WIDTH {
        let list_width = (content_width.saturating_sub(2) / 2).max(42);
        let detail_width = content_width.saturating_sub(list_width + 2).max(28);
        let visible_row_budget = session_visible_row_budget(model, body_budget);
        children.push(
            element! {
                View(width: 100pct, height: body_budget as u32) {
                    #(render_session_list(model, model_state, selected_outcome, list_width, visible_row_budget, body_budget))
                    View(width: 2) { Text(content: "") }
                    #(focused_record
                        .map(|record| render_details(record, detail_width, selected_conversation, body_budget))
                        .unwrap_or_else(|| render_start_new_details(model, detail_width, body_budget)))
                }
            }
            .into_any(),
        );
    } else {
        let stacked_heights = (model.width >= NARROW_PICKER_WIDTH)
            .then(|| stacked_panel_heights(body_budget))
            .flatten();
        let list_height = stacked_heights.map_or(body_budget, |(list_height, _)| list_height);
        let visible_row_budget = session_visible_row_budget(model, list_height);
        children.push(render_session_list(
            model,
            model_state,
            selected_outcome,
            content_width,
            visible_row_budget,
            list_height,
        ));
        if let Some((_, details_height)) = stacked_heights {
            children.push(
                focused_record
                    .map(|record| {
                        render_details(record, content_width, selected_conversation, details_height)
                    })
                    .unwrap_or_else(|| {
                        render_start_new_details(model, content_width, details_height)
                    }),
            );
        }
    }

    children.push(render_footer(content_width, footer_lines));
    let picker_height = height.max(minimum_render_height);
    element! {
        View(
            width: model.width as u32,
            height: picker_height as u32,
            border_style: BorderStyle::Round,
            border_color: Color::Cyan,
            overflow: Overflow::Hidden,
            padding_left: 1,
            padding_right: 1,
            padding_top: 0,
            padding_bottom: 0,
            flex_direction: FlexDirection::Column,
            row_gap: 0,
        ) {
            #(children)
        }
    }
}

pub(super) fn render_filter_controls(
    model: &SessionsPickerModel,
    width: usize,
) -> Vec<AnyElement<'static>> {
    let filter = format!("Search: [{}]", model.search);
    let scope = format!("[{}]", root_label(model.root));
    let view = format!("View: [{}]", runtime_view_label(model.runtime_view));
    let sort = format!("Sort: [{}]", sort_label(model.sort));

    if one_line_filter_controls_fit_for_parts(width, [&filter, &scope, &view, &sort]) {
        return vec![control_line(vec![filter, scope, view, sort])];
    }

    if width < COMPACT_PICKER_WIDTH {
        return [filter, scope, view, sort]
            .into_iter()
            .map(|line| control_line(vec![line]))
            .collect();
    }

    if width < NARROW_PICKER_WIDTH {
        return vec![
            control_line(vec![filter]),
            control_line(vec![scope, view]),
            control_line(vec![sort]),
        ];
    }

    vec![
        control_line(vec![filter]),
        control_line(vec![scope, view, sort]),
    ]
}

pub(super) fn one_line_filter_controls_fit_for_parts<const N: usize>(
    width: usize,
    parts: [&str; N],
) -> bool {
    UnicodeWidthStr::width(parts.join("    ").as_str()) <= width
}

pub(super) fn control_line(parts: Vec<String>) -> AnyElement<'static> {
    element! {
        Text(
            content: parts.join("    "),
            color: Color::Grey,
            weight: Weight::Normal,
        )
    }
    .into_any()
}

pub(super) fn render_footer(width: usize, lines: Vec<String>) -> AnyElement<'static> {
    let footer_text = lines
        .into_iter()
        .map(|line| {
            element! {
                Text(
                    content: fit_line(&line, width),
                    color: Color::Grey,
                    weight: Weight::Light,
                    wrap: TextWrap::NoWrap,
                )
            }
            .into_any()
        })
        .collect::<Vec<_>>();
    element! {
        View(
            width: 100pct,
            flex_grow: 1.0_f32,
            flex_direction: FlexDirection::Column,
            justify_content: JustifyContent::FlexEnd,
        ) {
            View(
                width: 100pct,
                flex_direction: FlexDirection::Column,
                border_style: BorderStyle::Single,
                border_edges: Edges::Top,
                border_color: Color::DarkGrey,
                padding_top: 0,
            ) {
                #(footer_text)
            }
        }
    }
    .into_any()
}

//! Picker frame view.
use super::{
    MIN_PICKER_WIDTH, NARROW_PICKER_WIDTH, SIDECAR_PICKER_WIDTH, SessionConversationPreview,
    SessionsPickerModel, SessionsPickerOutcome, fit_line, footer_lines, picker_body_budget,
    render_details, render_session_list, render_start_new_details, root_label, runtime_view_label,
    session_visible_row_budget, sort_label,
};
use codex_router_tui::{DialogAction, DialogFrame, DialogHost, DialogMargins, DialogPresentation};
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
    let control_width = content_width.saturating_sub(2) / 2;
    let mut filter_controls = vec![element! {
        View(width: 100pct, column_gap: 2, flex_shrink: 0.0_f32) {
            View(width: control_width as u32, min_width: 0, overflow: Overflow::Hidden) {
                #(super::super::picker_machine_view::render_machine_control(model, model_state, control_width))
            }
            View(flex_grow: 1.0_f32, min_width: 0, overflow: Overflow::Hidden) {
                #(super::super::provider_filter_view::render_provider_controls(model, model_state, control_width))
            }
        }
    }.into_any()];
    filter_controls.extend(render_filter_controls(model, content_width));
    if model.machine_controls.filter
        != super::super::picker_machine_controls::PickerMachineFilter::Default
        && !model.source_progress.is_empty()
    {
        let status = model
            .source_progress
            .iter()
            .map(|progress| progress.display_line())
            .collect::<Vec<_>>()
            .join(" | ");
        filter_controls.push(element! { Text(content: fit_line(&status, content_width), color: Color::Grey, wrap: TextWrap::NoWrap) }.into_any());
    }
    if let Some(notice) = model
        .machine_controls
        .all_notice(&model.request.router_registry, &model.source_progress)
    {
        filter_controls.push(element! { Text(content: fit_line(&notice, content_width), color: Color::Yellow, wrap: TextWrap::NoWrap) }.into_any());
    }
    if let Some(notice) = model.runtime_coverage.notice() {
        filter_controls.push(element! {
            Text(content: fit_line(notice, content_width), color: Color::Yellow, wrap: TextWrap::NoWrap)
        }.into_any());
    }
    let control_height = filter_controls.len();
    let inline_details = content_width >= 40;
    let footer_lines = if content_width < NARROW_PICKER_WIDTH && !model.show_help {
        vec!["F1 Help".to_owned()]
    } else {
        footer_lines(
            if inline_details {
                content_width.saturating_sub(12)
            } else {
                content_width
            },
            model.show_help,
        )
    };
    let body_budget = picker_body_budget(
        height,
        control_height,
        footer_lines.len() + usize::from(!inline_details),
        minimum_render_height,
    );
    let mut children = vec![
        element! {
            Text(
                content: format!("Sessions · {}/{}", model.visible_record_len(), model.request.records.len()),
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
                        .map(|record| render_details(record, detail_width, selected_conversation, Size::Length(body_budget as u32)))
                        .unwrap_or_else(|| render_start_new_details(model, detail_width, body_budget)))
                }
            }
            .into_any(),
        );
    } else {
        let list_height = body_budget;
        let visible_row_budget = session_visible_row_budget(model, list_height);
        children.push(render_session_list(
            model,
            model_state,
            selected_outcome,
            content_width,
            visible_row_budget,
            list_height,
        ));
    }

    let mut details_model = model_state;
    children.push(render_footer(content_width, footer_lines, move |_| {
        details_model.write().open_details_dialog()
    }));
    let picker_height = height.max(minimum_render_height);
    let background = element! {
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
    .into_any();
    let viewport_width = u16::try_from(model.width).unwrap_or(u16::MAX);
    let viewport_height = u16::try_from(picker_height).unwrap_or(u16::MAX);
    let dialog = if model.provider_filter_draft.is_some() {
        Some(element! {
            DialogFrame(width: viewport_width, height: viewport_height,
                title: "Session filters".to_owned(),
                presentation: if model.width < NARROW_PICKER_WIDTH { DialogPresentation::FullScreen } else { DialogPresentation::Framed { maximum_width: 72, maximum_height: 16 } },
                margins: DialogMargins { top: 1, right: 2, bottom: 1, left: 2 },
                actions: super::super::provider_filter_view::render_provider_actions(model, model_state)) {
                #(super::super::provider_filter_view::render_provider_chooser(model, model_state))
            }
        }.into_any())
    } else if let Some(record) = model.details_dialog.as_ref() {
        let mut cancel_model = model_state;
        Some(element! {
            DialogFrame(width: viewport_width, height: viewport_height, title: format!("Session details · {}", record.title),
                presentation: if model.width < NARROW_PICKER_WIDTH { DialogPresentation::FullScreen } else { DialogPresentation::Framed { maximum_width: 100, maximum_height: 28 } },
                margins: DialogMargins { top: 1, right: 2, bottom: 1, left: 2 }, keyboard_scroll: true,
                actions: vec![element!(DialogAction(label: "Close · Esc", handler: move |_| cancel_model.write().details_dialog = None)).into_any()]) {
                #(render_details(record, content_width.min(90).saturating_sub(4), selected_conversation, Size::Auto))
            }
        }.into_any())
    } else {
        None
    };
    element! {
        View(width: model.width as u32, height: picker_height as u32) {
            DialogHost(width: viewport_width, height: viewport_height, background: vec![background], dialog)
        }
    }
}

pub(super) fn render_filter_controls(
    model: &SessionsPickerModel,
    width: usize,
) -> Vec<AnyElement<'static>> {
    let mut controls = Vec::new();
    if !model.search.is_empty() {
        controls.push(control_line(vec![fit_line(
            &format!("Search: [{}]", model.search),
            width,
        )]));
    }
    let mut settings = Vec::new();
    if model.root != super::SessionsPickerRoot::Any {
        settings.push(format!("[{}]", root_label(model.root)));
    }
    if model.runtime_view != super::super::picker_model::SessionsPickerRuntimeView::All {
        settings.push(format!(
            "View: [{}]",
            runtime_view_label(model.runtime_view)
        ));
    }
    if model.sort != super::SessionsSort::Updated {
        settings.push(format!("Sort: [{}]", sort_label(model.sort)));
    }
    if settings.is_empty() {
        return controls;
    }
    if one_line_filter_controls_fit_for_parts(width, settings.iter().map(String::as_str)) {
        controls.push(control_line(settings));
    } else {
        controls.extend(
            settings
                .into_iter()
                .map(|setting| control_line(vec![setting])),
        );
    }
    controls
}

pub(super) fn one_line_filter_controls_fit_for_parts<'a>(
    width: usize,
    parts: impl IntoIterator<Item = &'a str>,
) -> bool {
    let parts = parts.into_iter().collect::<Vec<_>>();
    parts
        .iter()
        .map(|part| UnicodeWidthStr::width(*part))
        .sum::<usize>()
        + parts.len().saturating_sub(1) * 3
        <= width
}

pub(super) fn control_line(parts: Vec<String>) -> AnyElement<'static> {
    let children = parts
        .into_iter()
        .map(|part| {
            element! {
                Text(content: part, color: Color::Grey, wrap: TextWrap::NoWrap)
            }
            .into_any()
        })
        .collect::<Vec<_>>();
    element! { View(column_gap: 3, flex_shrink: 0.0_f32) { #(children) } }.into_any()
}

pub(super) fn render_footer(
    width: usize,
    lines: Vec<String>,
    details_handler: impl FnMut(()) + Send + Sync + 'static,
) -> AnyElement<'static> {
    let footer_text = lines
        .into_iter()
        .map(|line| {
            element! {
                Text(
                    content: line,
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
                View(column_gap: 2, flex_direction: if width >= 40 { FlexDirection::Row } else { FlexDirection::Column }) {
                    super::InteractiveSessionChoiceRow(activates_on_click: true, activation_handler: details_handler) {
                        Text(content: "Details F4", color: Color::Cyan, wrap: TextWrap::NoWrap)
                    }
                    View(flex_direction: FlexDirection::Column, flex_shrink: 1.0_f32, min_width: 0) { #(footer_text) }
                }
            }
        }
    }
    .into_any()
}

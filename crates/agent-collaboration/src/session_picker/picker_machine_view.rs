//! Content-sized machine controls and choices in the existing iocraft visual language.
use super::interactive_row::InteractiveSessionChoiceRow;
use super::picker_machine_controls::{MachineChoicePurpose, PickerMachineStage};
use super::picker_model::SessionsPickerModel;
use iocraft::prelude::*;

pub(super) fn render_machine_control(
    model: &SessionsPickerModel,
    mut model_state: State<SessionsPickerModel>,
    _width: usize,
) -> AnyElement<'static> {
    let label = format!(
        "Machine: {}",
        model.machine_controls.label(model.request.machine_mode)
    );
    let focused = model.machine_controls.control_focused;
    element! {
        InteractiveSessionChoiceRow(
            focus_handler: move |_| { model_state.write().machine_controls.control_focused = true; },
            activation_handler: move |_| {
                let mut model = model_state.write();
                let registry = model.request.router_registry.clone();
                model.machine_controls.open_browse(&registry);
            },
            activates_on_click: true,
        ) {
            View(width: 100pct, column_gap: 1) {
                Text(content: label, color: if focused { Color::Yellow } else { Color::Grey }, weight: Weight::Bold, wrap: TextWrap::NoWrap)
                Text(content: "Ctrl+G / F2", color: Color::DarkGrey, wrap: TextWrap::NoWrap)
            }
        }
    }.into_any()
}

pub(super) fn render_machine_choices(
    model: &SessionsPickerModel,
    height: usize,
) -> Element<'static, View> {
    let PickerMachineStage::Choosing {
        purpose,
        focus,
        notice,
    } = model.machine_controls.stage.clone()
    else {
        return element! { View {} };
    };
    let title = match purpose {
        MachineChoicePurpose::Browse => "Choose machine",
        MachineChoicePurpose::NewSession => "Choose machine for new session",
    };
    let choices = model
        .machine_controls
        .choices(&model.request.router_registry, model.request.machine_mode);
    let available_rows = height.saturating_sub(10).max(1);
    let window_start = focus.saturating_sub(available_rows.saturating_sub(1));
    let rows = choices.iter().enumerate().skip(window_start).take(available_rows).map(|(index, label)| {
        let selected = index == focus;
        element! {
            View(column_gap: 1) {
                Text(content: if selected { "❯" } else { " " }, color: Color::Yellow, wrap: TextWrap::NoWrap)
                Text(content: label.clone(), color: if selected { Color::Yellow } else { Color::Grey }, wrap: TextWrap::NoWrap)
            }
        }.into_any()
    }).collect::<Vec<_>>();
    let detail = if matches!(purpose, MachineChoicePurpose::NewSession) && focus > 0 {
        if let crate::sessions::RouterRegistryRead::Ready(registry) = &model.request.router_registry
        {
            registry
                .routers
                .get(focus.saturating_sub(1))
                .map(|profile| {
                    profile.default_remote_cwd.as_ref().map_or_else(
                        || "Working directory: not configured".to_owned(),
                        |cwd| format!("Working directory: {}", cwd.as_str()),
                    )
                })
        } else {
            None
        }
    } else {
        None
    };
    let explanation = notice.as_ref().map(|notice| notice.message());
    element! {
        View(width: model.width as u32, height: height as u32, padding_left: 1, padding_right: 1,
            flex_direction: FlexDirection::Column, border_style: BorderStyle::Round,
            border_color: Color::Cyan, overflow: Overflow::Hidden) {
            Text(content: title, color: Color::Cyan, weight: Weight::Bold, wrap: TextWrap::NoWrap)
            View(border_style: BorderStyle::Single, border_color: Color::DarkGrey, padding: 1,
                flex_direction: FlexDirection::Column, margin_top: 1) {
                #(rows)
            }
            #(detail.map(|detail| element! { Text(content: detail, color: Color::Grey) }))
            #(explanation.map(|message| element! { Text(content: message, color: Color::Yellow) }))
            View(flex_grow: 1.0_f32) {}
            Text(content: "↑↓ Choose | Enter Select | Esc Back | Ctrl+C Exit", color: Color::Grey)
        }
    }
}

//! Content-sized, pointer-accessible provider controls in the existing picker frame.
use super::{
    interactive_row::InteractiveSessionChoiceRow,
    picker_model::SessionsPickerModel,
    provider_filter_model::{ProviderFilterFocus, SessionProviderFilter},
};
use iocraft::prelude::*;

pub(super) fn render_provider_controls(
    model: &SessionsPickerModel,
    mut model_state: State<SessionsPickerModel>,
    width: usize,
) -> AnyElement<'static> {
    let provider = model.provider_filter.label();
    let label = super::picker_component::fit_line(
        &if width >= 32 {
            format!("Filters · {provider}")
        } else {
            provider.to_owned()
        },
        width.saturating_sub(5),
    );
    element! {
        InteractiveSessionChoiceRow(
            activation_handler: move |_| model_state.write().open_provider_filters(),
            activates_on_click: true,
        ) {
            View(width: 100pct, column_gap: 1) {
                Text(content: label, color: Color::Grey, weight: Weight::Bold, wrap: TextWrap::NoWrap)
                Text(content: "▾ F3", color: Color::DarkGrey, wrap: TextWrap::NoWrap)
            }
        }
    }.into_any()
}

pub(super) fn render_provider_chooser(
    model: &SessionsPickerModel,
    mut model_state: State<SessionsPickerModel>,
) -> Vec<AnyElement<'static>> {
    let Some(draft) = model.provider_filter_draft else {
        return Vec::new();
    };
    let mut controls = vec![
        element!(Text(content: "Session client provider", color: Color::Cyan, weight: Weight::Bold)).into_any(),
        element!(Text(content: format!("Draft: {}", draft.selection.label()), color: Color::Yellow)).into_any(),
        element!(Text(content: format!("Current: {} · {}/{} sessions", model.provider_filter.label(), model.visible_record_len(), model.request.records.len()), color: Color::Grey)).into_any(),
    ];
    for choice in SessionProviderFilter::CHOICES {
        let selected = draft.selection == choice;
        controls.push(element! {
            InteractiveSessionChoiceRow(
                focus_handler: move |_| {
                    if let Some(draft) = model_state.write().provider_filter_draft.as_mut() { draft.focus = ProviderFilterFocus::Provider; }
                },
                activation_handler: move |_| {
                    if let Some(draft) = model_state.write().provider_filter_draft.as_mut() { draft.selection = choice; draft.focus = ProviderFilterFocus::Provider; }
                },
                activates_on_click: true,
            ) {
                View(column_gap: 1) {
                    Text(content: if selected { "●" } else { "○" }, color: if draft.focus == ProviderFilterFocus::Provider { Color::Yellow } else { Color::Grey })
                    Text(content: choice.label(), color: if selected { Color::Yellow } else { Color::Grey })
                }
            }
        }.into_any());
    }
    controls
}

pub(super) fn render_provider_actions(
    model: &SessionsPickerModel,
    mut model_state: State<SessionsPickerModel>,
) -> Vec<AnyElement<'static>> {
    let Some(draft) = model.provider_filter_draft else {
        return Vec::new();
    };
    let mut controls = Vec::new();
    controls.push(element! {
        View(column_gap: 3) {
            InteractiveSessionChoiceRow(
                focus_handler: move |_| {
                    if let Some(draft) = model_state.write().provider_filter_draft.as_mut() { draft.focus = ProviderFilterFocus::Apply; }
                },
                activation_handler: move |_| model_state.write().apply_provider_filters(),
                activates_on_click: true,
            ) { Text(content: "Apply", weight: Weight::Bold, color: if draft.focus == ProviderFilterFocus::Apply { Color::Yellow } else { Color::Grey }) }
            InteractiveSessionChoiceRow(
                focus_handler: move |_| {
                    if let Some(draft) = model_state.write().provider_filter_draft.as_mut() { draft.focus = ProviderFilterFocus::Cancel; }
                },
                activation_handler: move |_| model_state.write().provider_filter_draft = None,
                activates_on_click: true,
            ) { Text(content: "Cancel", weight: Weight::Bold, color: if draft.focus == ProviderFilterFocus::Cancel { Color::Yellow } else { Color::Grey }) }
        }
    }.into_any());
    controls.push(
        element! { Text(content: "↑/↓ provider · tab focus", color: Color::DarkGrey) }.into_any(),
    );
    controls.push(
        element! { Text(content: "enter apply · esc cancel", color: Color::DarkGrey) }.into_any(),
    );
    controls
}

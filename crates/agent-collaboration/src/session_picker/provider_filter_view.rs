//! Content-sized, pointer-accessible provider controls in the existing picker frame.
use super::{
    interactive_row::InteractiveSessionChoiceRow,
    picker_model::SessionsPickerModel,
    provider_filter_model::{ProviderFilterFocus, SessionProviderFilter},
};
use codex_router_tui::{DialogAction, UiHeading, UiHeadingLevel, UiText, UiTextRole};
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
        width.saturating_sub(2),
    );
    element! {
        InteractiveSessionChoiceRow(
            activation_handler: move |_| model_state.write().open_provider_filters(),
            activates_on_click: true,
        ) {
            View(width: 100pct, column_gap: 1) {
                Text(content: label, color: Color::Grey, weight: Weight::Bold, wrap: TextWrap::NoWrap)
                Text(content: "▾", color: Color::DarkGrey, wrap: TextWrap::NoWrap)
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
        element!(UiHeading(content: "Session client provider".to_owned(), level: UiHeadingLevel::Section)).into_any(),
        element!(UiText(content: format!("Draft: {}", draft.selection.label()), role: UiTextRole::Selected)).into_any(),
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
                    Text(content: if selected { "●" } else { "○" }, color: if selected { Color::Yellow } else { Color::Grey })
                    UiText(content: choice.label().to_owned(), role: if selected { UiTextRole::Selected } else { UiTextRole::Body })
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
    vec![
        element! {
            View(column_gap: 3) {
                DialogAction(label: "Apply", has_focus: draft.focus == ProviderFilterFocus::Apply,
                    handler: move |_| model_state.write().apply_provider_filters())
                DialogAction(label: "Cancel", has_focus: draft.focus == ProviderFilterFocus::Cancel,
                    handler: move |_| model_state.write().provider_filter_draft = None)
            }
        }
        .into_any(),
    ]
}

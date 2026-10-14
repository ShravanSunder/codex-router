use codex_router_tui::{UiHeading, UiHeadingLevel, UseDialogInteraction};
use iocraft::prelude::*;

use super::quota_provider_filters::*;

const NARROW_FILTER_TOOLBAR_WIDTH: usize = 90;

pub(super) const fn quota_filter_toolbar_height(width: usize) -> usize {
    if width < NARROW_FILTER_TOOLBAR_WIDTH {
        2
    } else {
        1
    }
}

pub(super) fn quota_filter_controls_height(
    mode: QuotaInteractionMode,
    width: usize,
    _chooser_open: bool,
    modal_owns_input: bool,
) -> usize {
    if mode == QuotaInteractionMode::Static || modal_owns_input {
        return 0;
    }
    quota_filter_toolbar_height(width)
}

#[derive(Default, Props)]
struct QuotaFilterButtonProps {
    label: String,
    selected: bool,
    has_focus: bool,
    enabled: bool,
    on_click: HandlerMut<'static, ()>,
}

#[component]
fn QuotaFilterButton(
    props: &mut QuotaFilterButtonProps,
    mut hooks: Hooks,
) -> impl Into<AnyElement<'static>> {
    let effective_enabled = props.enabled && hooks.dialog_input_enabled();
    hooks.use_dialog_local_events({
        let enabled = props.enabled;
        let mut on_click = props.on_click.take();
        move |event| {
            if enabled
                && matches!(
                    event,
                    TerminalEvent::FullscreenMouse(FullscreenMouseEvent {
                        kind: MouseEventKind::Down(MouseButton::Left),
                        ..
                    })
                )
            {
                on_click(());
            }
        }
    });
    element! {
        View(flex_shrink: 0.0_f32) {
            Text(content: props.label.clone(), color: if !effective_enabled { Color::DarkGrey } else if props.selected || props.has_focus { Color::Yellow } else { Color::White }, weight: Weight::Bold, decoration: if effective_enabled && props.has_focus { TextDecoration::Underline } else { TextDecoration::None }, wrap: TextWrap::NoWrap)
        }
    }
}

pub(super) fn render_quota_filter_toolbar(
    filters: QuotaAccountFilters,
    visible_count: usize,
    loaded_count: usize,
    width: usize,
    enabled: bool,
    on_open: impl FnMut(()) + Send + Sync + 'static,
    on_details: impl FnMut(()) + Send + Sync + 'static,
) -> AnyElement<'static> {
    let summary = quota_filter_summary(filters, visible_count, loaded_count);
    let narrow = width < NARROW_FILTER_TOOLBAR_WIDTH;
    element! {
        View(width: width as u32, height: quota_filter_toolbar_height(width) as u32, flex_direction: if narrow { FlexDirection::Column } else { FlexDirection::Row }, column_gap: 2, flex_shrink: 0.0_f32) {
            View(column_gap:2,flex_shrink:0.0_f32) {
                QuotaFilterButton(label: "Filters ▾".to_owned(), enabled, on_click: on_open)
                #(render_quota_details_button(on_details))
            }
            Text(content: if width < 64 {
                format!("{} · Disabled: {} · {visible_count}/{loaded_count} accounts", filters.provider.label(), filters.disabled.label())
            } else { summary }, color: Color::White, wrap: TextWrap::NoWrap)
        }
    }.into_any()
}

fn quota_filter_summary(
    filters: QuotaAccountFilters,
    visible_count: usize,
    loaded_count: usize,
) -> String {
    format!(
        "Provider: {} · Disabled: {} · {visible_count}/{loaded_count} accounts",
        filters.provider.label(),
        filters.disabled.label()
    )
}

pub(super) fn render_quota_filter_chooser(
    draft: QuotaFilterDraft,
    width: usize,
    controller: QuotaFilterController,
) -> AnyElement<'static> {
    let current_filters = controller.state.read().committed;
    let current_summary = {
        let view_model = controller.view_model.read();
        quota_filter_summary(
            current_filters,
            current_filters.visible_count(&view_model.rows),
            view_model.rows.len(),
        )
    };
    let provider_choices = AccountProviderFilter::CHOICES
        .into_iter()
        .map(|provider| {
            let selected = draft.filters.provider == provider;
            element! {
                QuotaFilterButton(
                    label: format!("{} {}", if selected { "●" } else { "○" }, provider.label()),
                    selected,
                    has_focus: selected && draft.focus == QuotaFilterFocus::Provider,
                    enabled: true,
                    on_click: move |_| controller.dispatch(QuotaFilterAction::Provider(provider)),
                )
            }
            .into_any()
        })
        .collect::<Vec<_>>();
    element! {
        View(width: width as u32, flex_direction: FlexDirection::Column) {
            View(column_gap: 1) {
                UiHeading(content: "Account provider".to_owned(), level: UiHeadingLevel::Section)
                Text(content: format!("· Draft: {}", draft.filters.provider.label()), color: Color::Yellow, weight: Weight::Bold, wrap: TextWrap::NoWrap)
            }
            #(if width < 64 {
                element! {
                    View(flex_direction:FlexDirection::Column) {
                        Text(content:format!("Current: Provider: {}",current_filters.provider.label()),color:Color::Grey,wrap:TextWrap::NoWrap)
                        Text(content:format!("Disabled: {} · {}/{} accounts",current_filters.disabled.label(),current_filters.visible_count(&controller.view_model.read().rows),controller.view_model.read().rows.len()),color:Color::Grey,wrap:TextWrap::NoWrap)
                    }
                }.into_any()
            } else {
                element!(Text(content:format!("Current: {current_summary}"),color:Color::Grey,wrap:TextWrap::Wrap)).into_any()
            })
            #(provider_choices)
            QuotaFilterButton(label: format!("Disabled accounts: {}", draft.filters.disabled.label()), has_focus: draft.focus == QuotaFilterFocus::Disabled, enabled: true, on_click: move |_| controller.dispatch(QuotaFilterAction::ToggleDisabled))

        }
    }.into_any()
}

pub(super) fn render_quota_details_button(
    on_open: impl FnMut(()) + Send + Sync + 'static,
) -> AnyElement<'static> {
    element!(QuotaFilterButton(label: "Details".to_owned(), enabled: true, on_click: on_open))
        .into_any()
}

pub(super) fn render_quota_filter_actions(
    draft: QuotaFilterDraft,
    controller: QuotaFilterController,
) -> AnyElement<'static> {
    element! {
        View(flex_direction: FlexDirection::Column) {
View(column_gap: 3, flex_direction: FlexDirection::Row) {
                QuotaFilterButton(label: "Apply".to_owned(), has_focus: draft.focus == QuotaFilterFocus::Apply, enabled: true, on_click: move |_| controller.dispatch(QuotaFilterAction::Apply))
                QuotaFilterButton(label: "Cancel".to_owned(), has_focus: draft.focus == QuotaFilterFocus::Cancel, enabled: true, on_click: move |_| controller.dispatch(QuotaFilterAction::Cancel))
            }
        }
    }.into_any()
}

use std::time::Duration;

use super::picker_machine_controls::{PickerMachineFilter, PickerMachineStage};
use iocraft::prelude::*;

use crate::presentation::session_picker::interactive_row::InteractiveSessionChoiceRow;
use crate::presentation::session_picker::picker_actions::SessionsPickerKey;
use crate::presentation::session_picker::picker_actions::SessionsPickerOutcome;
use crate::presentation::session_picker::picker_model::SessionsPickerModel;
use crate::presentation::session_picker::picker_rendering::MIN_PICKER_WIDTH;
use crate::presentation::session_picker::picker_request::SessionsPickerRecordLoader;
use crate::presentation::session_picker::picker_request::SessionsPickerRequest;
use crate::presentation::session_picker::picker_request::SessionsPickerRoot;
use crate::sessions::SessionConversationPreview;
#[cfg(test)]
use crate::sessions::SessionConversationSource;
use crate::sessions::SessionPickerRecord;
use crate::sessions::SessionsSort;

#[path = "picker_frame_view.rs"]
mod picker_frame_view;
use picker_frame_view::render_picker_view;

#[path = "details_dialog_view.rs"]
mod details_dialog_view;

#[path = "picker_layout_budget.rs"]
mod picker_layout_budget;
use picker_layout_budget::{picker_body_budget, session_visible_row_budget};

#[path = "picker_list_view.rs"]
mod picker_list_view;
use picker_list_view::{no_matching_sessions_label, render_session_list, start_new_args_label};

#[path = "picker_detail_view.rs"]
mod picker_detail_view;
use picker_detail_view::{render_details, render_start_new_details};

#[path = "picker_display_text.rs"]
mod picker_display_text;
use picker_display_text::{compact_age, root_label, runtime_view_label, sort_label, truncate_end};

pub(super) use picker_display_text::fit_line;

const MIN_RENDER_HEIGHT: usize = 24;
const SIDECAR_PICKER_WIDTH: usize = 140;
const NARROW_PICKER_WIDTH: usize = 72;
const START_NEW_DETAILS_HEIGHT: usize = 6;

#[path = "conversation_preview_state.rs"]
mod conversation_preview_state;
use conversation_preview_state::{ConversationPreviewCache, ConversationPreviewLoadRequest};
#[cfg(test)]
use conversation_preview_state::{ConversationPreviewKey, SelectedConversationPreview};

#[derive(Default, Props)]
pub(crate) struct SessionsPickerComponentProps<'a> {
    request: SessionsPickerRequest,
    record_loader: Option<SessionsPickerRecordLoader>,
    source_preparer: Option<super::source_action_worker::SourceActionPreparer>,
    session_id_copier: Option<super::session_id_clipboard::SessionIdCopier>,
    width: usize,
    height: usize,
    selected_outcome_out: Option<&'a mut Option<SessionsPickerOutcome>>,
}

use super::source_reload_port::SessionRecordsReloadPort;
use super::source_reload_worker::run_session_record_reload_worker;

#[component]
pub(crate) fn SessionsPickerComponent<'a>(
    props: &mut SessionsPickerComponentProps<'a>,
    mut hooks: Hooks,
) -> impl Into<AnyElement<'static>> {
    let session_id_copier = props
        .session_id_copier
        .clone()
        .unwrap_or_else(super::session_id_clipboard::unavailable_session_id_copier);
    let mut system = hooks.use_context_mut::<SystemContext>();
    let (terminal_width, terminal_height) = hooks.use_terminal_size();
    let live_terminal_width = props.width == 0;
    let live_terminal_height = props.height == 0 && live_terminal_width;
    let observed_width = hooks.use_state(|| {
        if live_terminal_width {
            let width = usize::from(terminal_width);
            if width == 0 { MIN_PICKER_WIDTH } else { width }
        } else {
            props.width
        }
    });
    let observed_height = hooks.use_state(|| {
        if live_terminal_height {
            let height = usize::from(terminal_height);
            if height == 0 {
                MIN_RENDER_HEIGHT
            } else {
                height
            }
        } else if props.height == 0 {
            MIN_RENDER_HEIGHT
        } else {
            props.height.max(1)
        }
    });
    let width = observed_width.get();
    let height = observed_height.get();
    let mut model = hooks.use_state(|| SessionsPickerModel::new(props.request.clone(), width));
    if model.read().width != width {
        model.write().set_width(width);
    }
    let mut conversation_cache = hooks.use_state(ConversationPreviewCache::default);
    let reload_generation = hooks.use_state(|| 0_u64);
    let reload_port = hooks.use_memo(
        || SessionRecordsReloadPort::new(model.read().data_query(), model.read().source_contexts()),
        (),
    );
    hooks.use_future({
        let receiver = reload_port.take_receiver();
        let record_loader = props.record_loader.clone();
        let mut model = model;
        let reload_port = reload_port.clone();
        let mut reload_generation = reload_generation;
        let mut conversation_cache = conversation_cache;
        async move {
            let (Some(receiver), Some(loader)) = (receiver, record_loader) else {
                return;
            };
            run_session_record_reload_worker(receiver, loader, move |request, update| {
                if reload_generation.get() != request.generation {
                    return;
                }
                let acceptance = model.write().accept_source_reload(&request, update);
                if matches!(
                    acceptance,
                    super::picker_source_view::SourceReloadAcceptance::RestoredIndependentView
                        | super::picker_source_view::SourceReloadAcceptance::RestoredRetiredView
                ) {
                    conversation_cache.write().invalidate();
                    let generation = reload_generation.get().saturating_add(1);
                    reload_generation.set(generation);
                    if acceptance
                        == super::picker_source_view::SourceReloadAcceptance::RestoredIndependentView
                    {
                        reload_port.send(model.write().prepare_source_reload(generation));
                    }
                }
            })
            .await;
        }
    });
    hooks.use_future({
        let reload_port = reload_port.clone();
        async move {
            reload_port.send(model.write().prepare_source_reload(reload_generation.get()));
            let mut refresh_interval = tokio::time::interval(Duration::from_secs(3));
            refresh_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            refresh_interval.tick().await;
            loop {
                refresh_interval.tick().await;
                if model.read().source_action.pending.is_some()
                    || model.read().machine_controls.filter != PickerMachineFilter::Default
                    || model.read().fork_confirmation.is_some()
                    || !matches!(
                        model.read().machine_controls.stage,
                        PickerMachineStage::Browsing
                    )
                {
                    continue;
                }
                reload_port.send(model.write().prepare_source_reload(reload_generation.get()));
            }
        }
    });
    let load_conversation = hooks.use_async_handler({
        let mut conversation_cache = conversation_cache;
        move |request: ConversationPreviewLoadRequest| async move {
            let source = request.key.source.clone();
            let preview = match tokio::task::spawn_blocking(move || {
                SessionConversationPreview::from_rollout_source(Some(&source))
            })
            .await
            {
                Ok(preview) => preview,
                Err(_) => SessionConversationPreview::unavailable("history unavailable"),
            };
            conversation_cache.write().complete_load(&request, preview);
        }
    });
    let mut selected_outcome = hooks.use_state(|| Option::<SessionsPickerOutcome>::None);
    let mut should_cancel = hooks.use_state(|| false);
    super::source_action_worker::use_source_preparation(
        &mut hooks,
        model,
        selected_outcome,
        props.source_preparer.clone(),
    );
    hooks.use_terminal_events({
        let session_id_copier = session_id_copier.clone();
        let mut observed_width = observed_width;
        let mut observed_height = observed_height;
        let mut reload_generation = reload_generation;
        let can_load_source = props.record_loader.is_some();
        move |event| {
            if let TerminalEvent::Resize(width, height) = event {
                if live_terminal_width {
                    observed_width.set(usize::from(width));
                }
                if live_terminal_height {
                    observed_height.set(usize::from(height).max(1));
                }
                return;
            }
            if let TerminalEvent::BracketedPaste(text) = event {
                if width < MIN_PICKER_WIDTH
                    || should_cancel.get()
                    || selected_outcome.read().is_some()
                {
                    return;
                }
                let mut model_value = model.write();
                if model_value.source_action.pending.is_some()
                    || model_value.show_help
                    || model_value.fork_confirmation.is_some()
                    || model_value.provider_filter_draft.is_some()
                    || model_value.details_dialog.is_some()
                    || !matches!(
                        model_value.machine_controls.stage,
                        PickerMachineStage::Browsing
                    )
                {
                    return;
                }
                let previous_query = model_value.data_query();
                model_value.append_search_text(&text);
                let changed = model_value.data_query() != previous_query;
                drop(model_value);
                if changed {
                    conversation_cache.write().invalidate();
                    let generation = reload_generation.get().saturating_add(1);
                    reload_generation.set(generation);
                    reload_port.send(model.write().prepare_source_reload(generation));
                }
                return;
            }
            let TerminalEvent::Key(KeyEvent {
                code,
                kind,
                modifiers,
                ..
            }) = event
            else {
                return;
            };
            if kind == KeyEventKind::Release {
                return;
            }
            if width < MIN_PICKER_WIDTH {
                return;
            }

            let mut model_value = model.write();
            if modifiers.contains(KeyModifiers::CONTROL) && matches!(code, KeyCode::Char('c' | 'd'))
                || matches!(code, KeyCode::Char('\u{3}' | '\u{4}'))
            {
                model_value.cancel_source_action();
                should_cancel.set(true);
                selected_outcome.set(None);
                return;
            }
            if should_cancel.get() || selected_outcome.read().is_some() {
                return;
            }
            if model_value.source_action.pending.is_some() {
                if code == KeyCode::Esc {
                    model_value.cancel_source_action();
                }
                return;
            }
            let help_key = matches!(code, KeyCode::F(1) | KeyCode::Char('\u{1f}'))
                || matches!(code, KeyCode::Char('/' | '_' | '7'))
                    && modifiers.contains(KeyModifiers::CONTROL);
            if help_key {
                model_value.handle_key(SessionsPickerKey::ToggleHelp);
                return;
            }
            if model_value.show_help {
                if code == KeyCode::Esc {
                    model_value.show_help = false;
                }
                return;
            }
            if let Some(confirmation) = model_value.fork_confirmation.as_mut() {
                match code {
                    KeyCode::Up => confirmation.move_destination(-1),
                    KeyCode::Down => confirmation.move_destination(1),
                    KeyCode::Esc => model_value.fork_confirmation = None,
                    KeyCode::Enter
                        if !modifiers.intersects(
                            KeyModifiers::ALT | KeyModifiers::CONTROL | KeyModifiers::SUPER,
                        ) && !model_value.begin_source_fork() =>
                    {
                        if let Some(confirmation) = model_value.fork_confirmation.as_mut() {
                            selected_outcome.set(confirmation.confirm());
                        }
                    }
                    _ => {}
                }
                return;
            }
            if matches!(
                model_value.machine_controls.stage,
                PickerMachineStage::Loading { .. }
            ) {
                if matches!(code, KeyCode::Esc) {
                    model_value.machine_controls.cancel_choice();
                    conversation_cache.write().invalidate();
                    let generation = reload_generation.get().saturating_add(1);
                    reload_generation.set(generation);
                    reload_port.send(model_value.prepare_source_reload(generation));
                }
                return;
            }
            if matches!(
                model_value.machine_controls.stage,
                PickerMachineStage::Choosing { .. }
            ) {
                let registry = model_value.request.router_registry.clone();
                let mode = model_value.request.machine_mode;
                let choice_count = model_value.machine_controls.choices(&registry, mode).len();
                match code {
                    KeyCode::Up => model_value.machine_controls.move_choice(-1, choice_count),
                    KeyCode::Down => model_value.machine_controls.move_choice(1, choice_count),
                    KeyCode::Esc => model_value.machine_controls.cancel_choice(),
                    KeyCode::Enter
                        if !modifiers.intersects(
                            KeyModifiers::ALT | KeyModifiers::CONTROL | KeyModifiers::SUPER,
                        ) =>
                    {
                        if model_value.begin_configured_new() {
                            return;
                        }
                        let previous_filter = model_value.machine_controls.filter.clone();
                        if model_value.machine_controls.select_choice(
                            &registry,
                            mode,
                            can_load_source,
                        ) {
                            selected_outcome.set(Some(SessionsPickerOutcome::StartNewSession));
                        }
                        conversation_cache.write().invalidate();
                        if model_value.machine_controls.filter != previous_filter
                            || matches!(
                                model_value.machine_controls.stage,
                                PickerMachineStage::Loading { .. }
                            )
                        {
                            let generation = reload_generation.get().saturating_add(1);
                            reload_generation.set(generation);
                            reload_port.send(model_value.prepare_source_reload(generation));
                        }
                    }
                    _ => {}
                }
                return;
            }
            if super::provider_filter_input::handle_provider_filter_key(
                &mut model_value,
                code,
                modifiers,
            ) {
                return;
            }
            if model_value.details_dialog.is_some() {
                if let Some(outcome) = super::details_dialog_actions::handle_details_key(
                    &mut model_value,
                    code,
                    modifiers,
                    &session_id_copier,
                ) {
                    selected_outcome.set(Some(outcome));
                }
                return;
            }
            if code == KeyCode::F(4) {
                model_value.open_details_dialog();
                return;
            }
            if matches!(code, KeyCode::F(2))
                || code == KeyCode::Char('g') && modifiers.contains(KeyModifiers::CONTROL)
            {
                let registry = model_value.request.router_registry.clone();
                model_value.machine_controls.open_browse(&registry);
                return;
            }
            if matches!(code, KeyCode::Tab | KeyCode::BackTab) {
                model_value.machine_controls.control_focused =
                    !model_value.machine_controls.control_focused;
                return;
            }
            if model_value.machine_controls.control_focused {
                match code {
                    KeyCode::Enter => {
                        if !modifiers.intersects(
                            KeyModifiers::ALT | KeyModifiers::CONTROL | KeyModifiers::SUPER,
                        ) {
                            let registry = model_value.request.router_registry.clone();
                            model_value.machine_controls.open_browse(&registry);
                        }
                        return;
                    }
                    KeyCode::Up | KeyCode::Down | KeyCode::Esc => {
                        model_value.machine_controls.control_focused = false;
                        return;
                    }
                    _ => {}
                }
            }
            let previous_query = model_value.data_query();
            match code {
                KeyCode::Down => model_value.handle_key(SessionsPickerKey::MoveDown),
                KeyCode::Up => model_value.handle_key(SessionsPickerKey::MoveUp),
                KeyCode::PageDown => model_value.handle_key(SessionsPickerKey::PageDown),
                KeyCode::PageUp => model_value.handle_key(SessionsPickerKey::PageUp),
                KeyCode::Home => model_value.handle_key(SessionsPickerKey::MoveFirst),
                KeyCode::End => model_value.handle_key(SessionsPickerKey::MoveLast),
                KeyCode::Char('n') if modifiers.contains(KeyModifiers::CONTROL) => {
                    selected_outcome.set(model_value.start_new_action());
                }
                KeyCode::Char('s') if modifiers.contains(KeyModifiers::CONTROL) => {
                    model_value.handle_key(SessionsPickerKey::CycleRoot);
                }
                KeyCode::Char('t') if modifiers.contains(KeyModifiers::CONTROL) => {
                    model_value.handle_key(SessionsPickerKey::CycleRuntimeView);
                }
                KeyCode::Char('o') if modifiers.contains(KeyModifiers::CONTROL) => {
                    model_value.handle_key(SessionsPickerKey::CycleSort);
                }
                KeyCode::Char('r') if modifiers.contains(KeyModifiers::CONTROL) => {
                    conversation_cache.write().invalidate();
                    let generation = reload_generation.get().saturating_add(1);
                    reload_generation.set(generation);
                    reload_port.send(model_value.prepare_source_reload(generation));
                }
                KeyCode::Char('c' | 'd') if modifiers.contains(KeyModifiers::CONTROL) => {
                    should_cancel.set(true);
                }
                KeyCode::Char('\u{3}' | '\u{4}') => {
                    should_cancel.set(true);
                }
                KeyCode::Backspace => model_value.handle_key(SessionsPickerKey::SearchBackspace),
                KeyCode::Char(character)
                    if !modifiers.intersects(
                        KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER,
                    ) =>
                {
                    model_value.handle_key(SessionsPickerKey::SearchChar(character));
                }
                KeyCode::Enter
                    if modifiers.contains(KeyModifiers::CONTROL | KeyModifiers::SHIFT)
                        && !modifiers.intersects(KeyModifiers::ALT | KeyModifiers::SUPER) =>
                {
                    model_value.open_fork_confirmation();
                }
                KeyCode::Enter
                    if modifiers.contains(KeyModifiers::CONTROL)
                        && !modifiers.intersects(KeyModifiers::ALT | KeyModifiers::SUPER) =>
                {
                    if model_value.begin_source_resume() {
                        return;
                    }
                    let outcome = if model_value.focused_identity().is_none() {
                        model_value.start_new_action()
                    } else {
                        model_value.activation_outcome_for_focus()
                    };
                    selected_outcome.set(outcome);
                }
                KeyCode::Enter if modifiers.is_empty() => model_value.open_details_dialog(),
                KeyCode::Esc => {
                    if model_value.search.is_empty()
                        && model_value.machine_controls.filter != PickerMachineFilter::Default
                    {
                        let mode = model_value.request.machine_mode;
                        model_value
                            .machine_controls
                            .return_to_default(mode, can_load_source);
                        conversation_cache.write().invalidate();
                        let generation = reload_generation.get().saturating_add(1);
                        reload_generation.set(generation);
                        reload_port.send(model_value.prepare_source_reload(generation));
                    } else if model_value.search.is_empty() {
                        should_cancel.set(true);
                    } else {
                        model_value.handle_key(SessionsPickerKey::ClearSearch);
                    }
                }
                _ => {}
            }
            let next_query = model_value.data_query();
            drop(model_value);

            if next_query != previous_query {
                conversation_cache.write().invalidate();
                let generation = reload_generation.get().saturating_add(1);
                reload_generation.set(generation);
                reload_port.send(model.write().prepare_source_reload(generation));
            }
        }
    });

    if should_cancel.get() {
        selected_outcome.set(None);
    }
    if let Some(selected_outcome) = selected_outcome.read().clone() {
        if let Some(out) = props.selected_outcome_out.as_mut() {
            **out = Some(selected_outcome);
        }
        system.exit();
    } else if *should_cancel.read() {
        if let Some(out) = props.selected_outcome_out.as_mut() {
            **out = None;
        }
        system.exit();
    }

    if width < MIN_PICKER_WIDTH {
        if let Some(out) = props.selected_outcome_out.as_mut() {
            **out = Some(SessionsPickerOutcome::TerminalTooNarrow);
        }
        system.exit();
        return element! {
            View(width: width as u32, flex_direction: FlexDirection::Column) {
                Text(content: "terminal too narrow\n")
            }
        };
    }

    if let Some(background) =
        super::picker_help_view::render_choice_surface(&model.read(), model, height)
    {
        return background;
    }

    let selected_conversation = {
        let model_value = model.read();
        model_value
            .details_dialog
            .as_ref()
            .and_then(|dialog| dialog.record())
            .or_else(|| model_value.focused_record())
            .map(|record| {
                let selected_preview = {
                    let cache = conversation_cache.read();
                    cache.select_record(record)
                };
                if let Some(load_request) = selected_preview.load_request.clone() {
                    conversation_cache.write().start_loading(&load_request);
                    load_conversation(load_request);
                }
                selected_preview.preview
            })
    };

    let minimum_render_height = if live_terminal_height {
        1
    } else {
        MIN_RENDER_HEIGHT.min(height)
    };
    render_picker_view(
        &model.read(),
        model,
        selected_outcome,
        selected_conversation.as_ref(),
        height,
        minimum_render_height,
        session_id_copier,
    )
}

#[path = "picker_entrypoint.rs"]
mod picker_entrypoint;
pub(crate) use picker_entrypoint::run_sessions_picker;
#[cfg(feature = "quota-reset-test-harness")]
pub(crate) use picker_entrypoint::run_sessions_picker_test_harness;

#[cfg(test)]
#[path = "picker_component_tests.rs"]
mod tests;

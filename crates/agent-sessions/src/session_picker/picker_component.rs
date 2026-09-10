use std::collections::BTreeMap;
use std::io;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use iocraft::prelude::*;

use crate::presentation::session_picker::interactive_row::InteractiveSessionChoiceRow;
use crate::presentation::session_picker::picker_actions::SessionsPickerKey;
use crate::presentation::session_picker::picker_actions::SessionsPickerOutcome;
use crate::presentation::session_picker::picker_model::SessionsPickerModel;
use crate::presentation::session_picker::picker_rendering::MIN_PICKER_WIDTH;
use crate::presentation::session_picker::picker_rendering::footer_lines;
use crate::presentation::session_picker::picker_request::SessionsPickerDataQuery;
use crate::presentation::session_picker::picker_request::SessionsPickerRecordLoader;
use crate::presentation::session_picker::picker_request::SessionsPickerRequest;
use crate::presentation::session_picker::picker_request::SessionsPickerRoot;
use crate::sessions::SessionConversationPreview;
use crate::sessions::SessionConversationSource;
use crate::sessions::SessionPickerRecord;
use crate::sessions::SessionsSort;
#[cfg(feature = "quota-reset-test-harness")]
use crate::sessions::SessionsSource;

#[path = "picker_frame_view.rs"]
mod picker_frame_view;
use picker_frame_view::render_picker_view;

#[path = "picker_layout_budget.rs"]
mod picker_layout_budget;
use picker_layout_budget::{picker_body_budget, session_visible_row_budget, stacked_panel_heights};

#[path = "picker_list_view.rs"]
mod picker_list_view;
use picker_list_view::{no_matching_sessions_label, render_session_list, start_new_args_label};

#[path = "picker_detail_view.rs"]
mod picker_detail_view;
use picker_detail_view::{render_details, render_start_new_details};

#[path = "picker_display_text.rs"]
mod picker_display_text;
use picker_display_text::{
    compact_age, fit_line, root_label, runtime_view_label, sort_label, truncate_end,
};

const MIN_RENDER_HEIGHT: usize = 24;
const SIDECAR_PICKER_WIDTH: usize = 160;
const NARROW_PICKER_WIDTH: usize = 72;
const COMPACT_PICKER_WIDTH: usize = 56;
const MIN_STACKED_DETAILS_HEIGHT: usize = 6;
const START_NEW_DETAILS_HEIGHT: usize = 6;

#[derive(Clone, Debug, Eq, PartialEq)]
struct ConversationPreviewLoadRequest {
    session_id: String,
    source: SessionConversationSource,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ConversationPreviewLoadState {
    Loading,
    Loaded(SessionConversationPreview),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SelectedConversationPreview {
    preview: SessionConversationPreview,
    load_request: Option<ConversationPreviewLoadRequest>,
}

#[derive(Default, Props)]
pub(crate) struct SessionsPickerComponentProps<'a> {
    request: SessionsPickerRequest,
    record_loader: Option<SessionsPickerRecordLoader>,
    width: usize,
    height: usize,
    selected_outcome_out: Option<&'a mut Option<SessionsPickerOutcome>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SessionRecordsReloadRequest {
    generation: u64,
    query: SessionsPickerDataQuery,
}

#[derive(Clone)]
struct SessionRecordsReloadPort {
    sender: tokio::sync::watch::Sender<SessionRecordsReloadRequest>,
    receiver: Arc<Mutex<Option<tokio::sync::watch::Receiver<SessionRecordsReloadRequest>>>>,
}

impl SessionRecordsReloadPort {
    fn new(initial_query: SessionsPickerDataQuery) -> Self {
        let (sender, receiver) = tokio::sync::watch::channel(SessionRecordsReloadRequest {
            generation: 0,
            query: initial_query,
        });
        Self {
            sender,
            receiver: Arc::new(Mutex::new(Some(receiver))),
        }
    }

    fn send(&self, request: SessionRecordsReloadRequest) {
        self.sender.send_replace(request);
    }

    fn take_receiver(&self) -> Option<tokio::sync::watch::Receiver<SessionRecordsReloadRequest>> {
        self.receiver
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }
}

#[component]
pub(crate) fn SessionsPickerComponent<'a>(
    props: &mut SessionsPickerComponentProps<'a>,
    mut hooks: Hooks,
) -> impl Into<AnyElement<'static>> {
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
            props.height.max(MIN_RENDER_HEIGHT)
        }
    });
    let width = observed_width.get();
    let height = observed_height.get();
    let mut model = hooks.use_state(|| SessionsPickerModel::new(props.request.clone(), width));
    if model.read().width != width {
        model.write().set_width(width);
    }
    let mut conversation_cache =
        hooks.use_state(BTreeMap::<String, ConversationPreviewLoadState>::new);
    let reload_generation = hooks.use_state(|| 0_u64);
    let reload_port = hooks.use_memo(
        || SessionRecordsReloadPort::new(model.read().data_query()),
        (),
    );
    hooks.use_future({
        let receiver = reload_port.take_receiver();
        let record_loader = props.record_loader.clone();
        let mut model = model;
        async move {
            let (Some(receiver), Some(loader)) = (receiver, record_loader) else {
                return;
            };
            run_session_record_reload_worker(receiver, loader, move |request, records| {
                if reload_generation.get() != request.generation {
                    return;
                }
                let mut model_value = model.write();
                if model_value.data_query() == request.query {
                    match records {
                        Ok(records) => model_value.replace_records(records),
                        Err(()) => model_value.invalidate_runtime_statuses(),
                    }
                }
            })
            .await;
        }
    });
    hooks.use_future({
        let reload_port = reload_port.clone();
        async move {
            reload_port.send(SessionRecordsReloadRequest {
                generation: reload_generation.get(),
                query: model.read().data_query(),
            });
            let mut refresh_interval = tokio::time::interval(Duration::from_secs(3));
            refresh_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            refresh_interval.tick().await;
            loop {
                refresh_interval.tick().await;
                reload_port.send(SessionRecordsReloadRequest {
                    generation: reload_generation.get(),
                    query: model.read().data_query(),
                });
            }
        }
    });
    let load_conversation = hooks.use_async_handler({
        let mut conversation_cache = conversation_cache;
        move |request: ConversationPreviewLoadRequest| async move {
            let session_id = request.session_id;
            let source = request.source;
            let preview = match tokio::task::spawn_blocking(move || {
                SessionConversationPreview::from_rollout_source(Some(&source))
            })
            .await
            {
                Ok(preview) => preview,
                Err(_) => SessionConversationPreview::unavailable("history unavailable"),
            };
            conversation_cache
                .write()
                .insert(session_id, ConversationPreviewLoadState::Loaded(preview));
        }
    });
    let mut selected_outcome = hooks.use_state(|| Option::<SessionsPickerOutcome>::None);
    let mut should_cancel = hooks.use_state(|| false);
    hooks.use_terminal_events({
        let mut observed_width = observed_width;
        let mut observed_height = observed_height;
        let mut reload_generation = reload_generation;
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
            let previous_query = model_value.data_query();
            match code {
                KeyCode::Down => model_value.handle_key(SessionsPickerKey::MoveDown),
                KeyCode::Up => model_value.handle_key(SessionsPickerKey::MoveUp),
                KeyCode::PageDown => model_value.handle_key(SessionsPickerKey::PageDown),
                KeyCode::PageUp => model_value.handle_key(SessionsPickerKey::PageUp),
                KeyCode::Home => model_value.handle_key(SessionsPickerKey::MoveFirst),
                KeyCode::End => model_value.handle_key(SessionsPickerKey::MoveLast),
                KeyCode::Char('n') if modifiers.contains(KeyModifiers::CONTROL) => {
                    selected_outcome.set(Some(SessionsPickerOutcome::StartNewSession));
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
                    reload_port.send(SessionRecordsReloadRequest {
                        generation: reload_generation.get(),
                        query: model_value.data_query(),
                    });
                }
                KeyCode::F(1) | KeyCode::Char('\u{1f}') => {
                    model_value.handle_key(SessionsPickerKey::ToggleHelp);
                }
                // Crossterm 0.29 decodes legacy byte 0x1f (Ctrl+/ or Ctrl+_) as Ctrl+7.
                KeyCode::Char('/' | '_' | '7') if modifiers.contains(KeyModifiers::CONTROL) => {
                    model_value.handle_key(SessionsPickerKey::ToggleHelp);
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
                KeyCode::Enter if modifiers.contains(KeyModifiers::ALT) => {
                    if let Some(session_id) = model_value.focused_session_id() {
                        selected_outcome.set(Some(SessionsPickerOutcome::ForkSession(
                            session_id.to_owned(),
                        )));
                    }
                }
                KeyCode::Enter => selected_outcome.set(model_value.activation_outcome_for_focus()),
                KeyCode::Esc => {
                    if model_value.show_help {
                        model_value.handle_key(SessionsPickerKey::ToggleHelp);
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
                let generation = reload_generation.get().saturating_add(1);
                reload_generation.set(generation);
                reload_port.send(SessionRecordsReloadRequest {
                    generation,
                    query: next_query,
                });
            }
        }
    });

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

    let selected_conversation = {
        let model_value = model.read();
        model_value.focused_record().map(|record| {
            let selected_preview = {
                let cache = conversation_cache.read();
                selected_conversation_preview_for_record(record, &cache)
            };
            if let Some(load_request) = selected_preview.load_request.clone() {
                conversation_cache.write().insert(
                    load_request.session_id.clone(),
                    ConversationPreviewLoadState::Loading,
                );
                load_conversation(load_request);
            }
            selected_preview.preview
        })
    };

    let minimum_render_height = if live_terminal_height {
        1
    } else {
        MIN_RENDER_HEIGHT
    };
    render_picker_view(
        &model.read(),
        model,
        selected_outcome,
        selected_conversation.as_ref(),
        height,
        minimum_render_height,
    )
}

async fn run_session_record_reload_worker(
    mut receiver: tokio::sync::watch::Receiver<SessionRecordsReloadRequest>,
    loader: SessionsPickerRecordLoader,
    mut accept_records: impl FnMut(
        SessionRecordsReloadRequest,
        Result<crate::picker_runtime_status::PickerRecordsSnapshot, ()>,
    ),
) {
    while receiver.changed().await.is_ok() {
        let request = receiver.borrow_and_update().clone();
        let query = request.query.clone();
        let loader = loader.clone();
        let loaded_records = tokio::task::spawn_blocking(move || loader(query)).await;
        let records = match loaded_records {
            Ok(Ok(records)) => Ok(records),
            Ok(Err(_)) | Err(_) => Err(()),
        };
        accept_records(request, records);
    }
}

fn selected_conversation_preview_for_record(
    record: &SessionPickerRecord,
    cache: &BTreeMap<String, ConversationPreviewLoadState>,
) -> SelectedConversationPreview {
    let Some(source) = record.conversation_source.as_ref() else {
        return SelectedConversationPreview {
            preview: record.conversation.clone(),
            load_request: None,
        };
    };

    match cache.get(&record.session_id) {
        Some(ConversationPreviewLoadState::Loaded(preview)) => SelectedConversationPreview {
            preview: preview.clone(),
            load_request: None,
        },
        Some(ConversationPreviewLoadState::Loading) => SelectedConversationPreview {
            preview: record.conversation.clone(),
            load_request: None,
        },
        None => SelectedConversationPreview {
            preview: record.conversation.clone(),
            load_request: Some(ConversationPreviewLoadRequest {
                session_id: record.session_id.clone(),
                source: source.clone(),
            }),
        },
    }
}

pub(crate) fn run_sessions_picker(
    request: SessionsPickerRequest,
    record_loader: Option<SessionsPickerRecordLoader>,
) -> io::Result<Option<SessionsPickerOutcome>> {
    let mut selected_outcome = None;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(
        element! {
            SessionsPickerComponent(
                request: request,
                record_loader: record_loader,
                width: 0usize,
                selected_outcome_out: &mut selected_outcome,
            )
        }
        .render_loop()
        .fullscreen()
        .ignore_ctrl_c(),
    )?;
    Ok(selected_outcome)
}

#[cfg(feature = "quota-reset-test-harness")]
pub(crate) fn run_sessions_picker_test_harness() -> io::Result<()> {
    use std::io::Write;

    use crate::presentation::session_picker::test_support::picker_request;

    let mut request = picker_request();
    request.root = SessionsPickerRoot::Any;
    request.source = SessionsSource::All;
    if let Some(pointer_focus_record) = request
        .records
        .iter_mut()
        .find(|record| record.session_id == "thread-b")
    {
        pointer_focus_record.title = "Pointer focus beta".to_owned();
        pointer_focus_record.preview = Some("BETA_PREVIEW_ACTIVE".to_owned());
        pointer_focus_record.conversation.snippets = vec!["BETA_CONVERSATION_ACTIVE".to_owned()];
    }

    let outcome = run_sessions_picker(request, None)?;
    let marker = match outcome {
        Some(SessionsPickerOutcome::ResumeSession(session_id)) => {
            format!("SESSION_PICKER_OUTCOME resume:{session_id}")
        }
        Some(SessionsPickerOutcome::ForkSession(session_id)) => {
            format!("SESSION_PICKER_OUTCOME fork:{session_id}")
        }
        Some(SessionsPickerOutcome::StartNewSession) => {
            "SESSION_PICKER_OUTCOME start-new".to_owned()
        }
        Some(SessionsPickerOutcome::TerminalTooNarrow) => {
            "SESSION_PICKER_OUTCOME terminal-too-narrow".to_owned()
        }
        None => "SESSION_PICKER_OUTCOME canceled".to_owned(),
    };
    writeln!(io::stdout(), "{marker}")
}

#[cfg(test)]
#[path = "picker_component_tests.rs"]
mod tests;

use std::collections::BTreeMap;
use std::io;
use std::sync::Arc;
use std::sync::Mutex;

use iocraft::prelude::*;
use unicode_width::UnicodeWidthStr;

use crate::presentation::session_picker::action::SessionsPickerKey;
use crate::presentation::session_picker::action::SessionsPickerOutcome;
use crate::presentation::session_picker::interactive_row::InteractiveSessionChoiceRow;
use crate::presentation::session_picker::model::SessionsPickerModel;
use crate::presentation::session_picker::render::MIN_PICKER_WIDTH;
use crate::presentation::session_picker::request::SessionsPickerDataQuery;
use crate::presentation::session_picker::request::SessionsPickerRecordLoader;
use crate::presentation::session_picker::request::SessionsPickerRequest;
use crate::presentation::session_picker::request::SessionsPickerRoot;
use crate::sessions::SessionConversationPreview;
use crate::sessions::SessionConversationSource;
use crate::sessions::SessionPickerRecord;
use crate::sessions::SessionsSort;
use crate::sessions::SessionsSource;

const MIN_RENDER_HEIGHT: usize = 24;
const SIDECAR_PICKER_WIDTH: usize = 160;
const NARROW_PICKER_WIDTH: usize = 72;
const COMPACT_PICKER_WIDTH: usize = 56;
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
                    model_value.replace_records(records);
                }
            })
            .await;
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
                    model_value.handle_key(SessionsPickerKey::CycleSource);
                }
                KeyCode::Char('o') if modifiers.contains(KeyModifiers::CONTROL) => {
                    model_value.handle_key(SessionsPickerKey::CycleSort);
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
                KeyCode::Enter => selected_outcome.set(model_value.activation_outcome_for_focus()),
                KeyCode::Esc => {
                    if model_value.search.is_empty() {
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
    mut accept_records: impl FnMut(SessionRecordsReloadRequest, Vec<SessionPickerRecord>),
) {
    while receiver.changed().await.is_ok() {
        let request = receiver.borrow_and_update().clone();
        let query = request.query.clone();
        let loader = loader.clone();
        let loaded_records = tokio::task::spawn_blocking(move || loader(query)).await;
        if let Ok(Ok(records)) = loaded_records {
            accept_records(request, records);
        }
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

fn render_picker_view(
    model: &SessionsPickerModel,
    model_state: State<SessionsPickerModel>,
    selected_outcome: State<Option<SessionsPickerOutcome>>,
    selected_conversation: Option<&SessionConversationPreview>,
    height: usize,
    minimum_render_height: usize,
) -> Element<'static, View> {
    let content_width = model.width.saturating_sub(4).max(MIN_PICKER_WIDTH);
    let filter_controls = render_filter_controls(model, content_width);
    let control_height = filter_controls.len();
    let body_budget = picker_body_budget(height, control_height, minimum_render_height);
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

    let visible_len = model.visible_len();
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
        let details_height = focused_record
            .filter(|_| model.width >= NARROW_PICKER_WIDTH)
            .map(|record| {
                detail_height(
                    selected_conversation.or(Some(&record.conversation)),
                    content_width,
                )
            });
        let minimum_list_height = if visible_len == 0 {
            0
        } else {
            session_list_height(visible_len, model.focused_window_start(1), 1)
        };
        let show_stacked_details = details_height
            .map(|height| minimum_list_height + height <= body_budget)
            .unwrap_or(false);
        let list_budget = body_budget.saturating_sub(if show_stacked_details {
            details_height.unwrap_or(0)
        } else {
            0
        });
        let visible_row_budget = session_visible_row_budget(model, list_budget);
        children.push(render_session_list(
            model,
            model_state,
            selected_outcome,
            content_width,
            visible_row_budget,
            session_list_height(
                visible_len,
                model.focused_window_start(visible_row_budget),
                visible_row_budget,
            ),
        ));
        if show_stacked_details && let Some(record) = focused_record {
            children.push(render_details(
                record,
                content_width,
                selected_conversation,
                details_height.unwrap_or(0),
            ));
        }
    }

    children.push(render_footer(content_width));
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

fn picker_body_budget(height: usize, control_height: usize, minimum_render_height: usize) -> usize {
    let root_border_height = 2;
    let title_height = 1;
    let footer_height = 2;
    height
        .max(minimum_render_height)
        .saturating_sub(root_border_height + title_height + control_height + footer_height)
}

fn session_visible_row_budget(model: &SessionsPickerModel, available_height: usize) -> usize {
    let visible_len = model.visible_len();
    if visible_len == 0 {
        return 0;
    }
    for candidate in (1..=visible_len).rev() {
        if session_list_height(
            visible_len,
            model.focused_window_start(candidate),
            candidate,
        ) <= available_height
        {
            return candidate;
        }
    }
    1
}

fn session_list_height(visible_len: usize, window_start: usize, visible_rows: usize) -> usize {
    let visible_rows = visible_rows.max(1);
    let window_end = (window_start + visible_rows).min(visible_len);
    let visible_count = window_end.saturating_sub(window_start);
    let remaining = visible_len.saturating_sub(window_start + visible_rows);

    let header_height = 2;
    let record_rows_height = (window_start..window_end)
        .map(session_choice_row_height)
        .sum::<usize>();
    let record_gap_height = visible_count.saturating_sub(1);
    let more_above_height = if window_start > 0 { 2 } else { 0 };
    let more_below_height = if remaining > 0 { 2 } else { 0 };
    let list_border_and_padding_height = 2;

    list_border_and_padding_height
        + header_height
        + more_above_height
        + record_rows_height
        + record_gap_height
        + more_below_height
}

const fn session_choice_row_height(visible_index: usize) -> usize {
    if visible_index == 0 { 4 } else { 2 }
}

fn detail_height(conversation: Option<&SessionConversationPreview>, width: usize) -> usize {
    let text_width = width.saturating_sub(4).max(1);
    let conversation_row_count = conversation.map_or(1, |conversation| {
        if conversation.snippets.is_empty() {
            return conversation
                .unavailable_reason
                .as_deref()
                .map_or(1, |reason| {
                    UnicodeWidthStr::width(reason).max(1).div_ceil(text_width)
                });
        }
        conversation
            .snippets
            .iter()
            .map(|snippet| {
                UnicodeWidthStr::width(snippet.as_str())
                    .saturating_add(2)
                    .max(1)
                    .div_ceil(text_width)
            })
            .sum::<usize>()
    });
    let detail_border_height = 2;
    let conversation_heading_height = 1;

    detail_border_height + conversation_heading_height + conversation_row_count
}

fn render_filter_controls(model: &SessionsPickerModel, width: usize) -> Vec<AnyElement<'static>> {
    let filter = if model.search.is_empty() {
        "Search text, id:, b:branch, repo:name".to_owned()
    } else {
        format!("Search: [{}]", model.search)
    };
    let scope = format!("[{}]", root_label(model.root));
    let threads = format!("Threads: [{}]", source_label(model.source));
    let sort = format!("Sort: [{}]", sort_label(model.sort));

    if one_line_filter_controls_fit_for_parts(width, [&filter, &scope, &threads, &sort]) {
        return vec![control_line(vec![filter, scope, threads, sort])];
    }

    if width < COMPACT_PICKER_WIDTH {
        return [filter, scope, threads, sort]
            .into_iter()
            .map(|line| control_line(vec![line]))
            .collect();
    }

    if width < NARROW_PICKER_WIDTH {
        return vec![
            control_line(vec![filter]),
            control_line(vec![scope, threads]),
            control_line(vec![sort]),
        ];
    }

    vec![
        control_line(vec![filter]),
        control_line(vec![scope, threads, sort]),
    ]
}

fn one_line_filter_controls_fit_for_parts<const N: usize>(width: usize, parts: [&str; N]) -> bool {
    UnicodeWidthStr::width(parts.join("    ").as_str()) <= width
}

fn control_line(parts: Vec<String>) -> AnyElement<'static> {
    element! {
        Text(
            content: parts.join("    "),
            color: Color::Grey,
            weight: Weight::Normal,
        )
    }
    .into_any()
}

fn render_start_new_details(
    model: &SessionsPickerModel,
    width: usize,
    height: usize,
) -> AnyElement<'static> {
    let detail_width = width.saturating_sub(4);
    element! {
        View(
            width: width as u32,
            height: height.max(START_NEW_DETAILS_HEIGHT) as u32,
            flex_direction: FlexDirection::Column,
            border_style: BorderStyle::Single,
            border_color: Color::DarkGrey,
            padding_left: 1,
            padding_right: 1,
            overflow: Overflow::Hidden,
        ) {
            Text(content: "Start new session", color: Color::Cyan, weight: Weight::Bold)
            Text(content: fit_line(&start_new_args_label(model), detail_width), color: Color::Yellow, weight: Weight::Bold, wrap: TextWrap::NoWrap)
            #(if model.visible_record_len() == 0 {
                Some(detail_text(no_matching_sessions_label(model), detail_width, Color::Grey))
            } else {
                None
            })
        }
    }
    .into_any()
}

fn render_session_list(
    model: &SessionsPickerModel,
    mut model_state: State<SessionsPickerModel>,
    mut selected_outcome: State<Option<SessionsPickerOutcome>>,
    width: usize,
    visible_rows: usize,
    panel_height: usize,
) -> AnyElement<'static> {
    let row_width = width.saturating_sub(6).max(24);
    let mut rows = vec![render_session_header(row_width)];
    let visible_len = model.visible_len();
    let focused_index = model.focused_visible_index();
    let visible_rows = visible_rows.max(1);
    let window_start = model.focused_window_start(visible_rows);
    if window_start > 0 {
        rows.push(
            element! {
                Text(
                    content: format!("+{window_start} more above"),
                    color: Color::DarkGrey,
                    weight: Weight::Light,
                )
            }
            .into_any(),
        );
        rows.push(list_gap());
    }
    let window_end = (window_start + visible_rows).min(visible_len);
    for visible_index in window_start..window_end {
        if visible_index > window_start {
            rows.push(list_gap());
        }
        if visible_index == 0 {
            let row = render_start_new_row(model, visible_index == focused_index, row_width);
            rows.push(
                element! {
                    InteractiveSessionChoiceRow(
                        focus_handler: move |_| {
                            let should_update_focus = {
                                let model_value = model_state.read();
                                model_value.focused_session_id().is_some()
                                    || model_value.focused_window_start(visible_rows) != window_start
                            };
                            if should_update_focus {
                                model_state.write().focus_start_new_in_window(window_start);
                            }
                        },
                        activation_handler: move |_| {
                            selected_outcome.set(Some(SessionsPickerOutcome::StartNewSession));
                        },
                        activates_on_click: true,
                    ) {
                        #(row)
                    }
                }
                .into_any(),
            );
        } else if let Some(record) = model.visible_choice_record_at(visible_index) {
            let session_id = record.session_id.clone();
            let row = render_record_row(record, visible_index == focused_index, row_width);
            rows.push(
                element! {
                    InteractiveSessionChoiceRow(
                        focus_handler: move |_| {
                            let should_update_focus = {
                                let model_value = model_state.read();
                                model_value.focused_session_id() != Some(session_id.as_str())
                                    || model_value.focused_window_start(visible_rows) != window_start
                            };
                            if should_update_focus {
                                let _ = model_state.write().focus_visible_session_in_window(
                                    &session_id,
                                    Some(window_start),
                                );
                            }
                        },
                        activates_on_click: false,
                    ) {
                        #(row)
                    }
                }
                .into_any(),
            );
        }
    }
    let remaining = visible_len.saturating_sub(window_start + visible_rows);
    if remaining > 0 {
        rows.push(list_gap());
        rows.push(
            element! {
                Text(
                    content: format!("+{remaining} more below"),
                    color: Color::DarkGrey,
                    weight: Weight::Light,
                )
            }
            .into_any(),
        );
    }

    element! {
        View(
            width: width as u32,
            height: panel_height as u32,
            flex_direction: FlexDirection::Column,
            border_style: BorderStyle::Single,
            border_color: Color::DarkGrey,
            overflow: Overflow::Hidden,
            padding_left: 1,
            padding_right: 1,
            padding_top: 0,
            padding_bottom: 0,
        ) {
            #(rows)
        }
    }
    .into_any()
}

fn render_start_new_row(
    model: &SessionsPickerModel,
    selected: bool,
    width: usize,
) -> AnyElement<'static> {
    let foreground = if selected { Color::White } else { Color::Grey };
    let title_prefix = if selected { "❯ " } else { "  " };
    let inner_width = width.saturating_sub(4);
    let title_width = inner_width.saturating_sub(18).max(14);
    let first_line = fit_line(
        &format!(
            "{title_prefix}{:<title_width$} {:>6} {:>6}",
            "Start new session", "-", "-"
        ),
        inner_width,
    );
    let metadata_line = if model.visible_record_len() == 0 {
        format!(
            "    {}  {}",
            no_matching_sessions_label(model),
            start_new_args_label(model)
        )
    } else {
        format!("    {}", start_new_args_label(model))
    };
    let second_line = fit_line(&metadata_line, inner_width);

    element! {
        View(
            width: width as u32,
            flex_direction: FlexDirection::Column,
            border_style: BorderStyle::Single,
            border_color: Color::DarkGrey,
            padding_left: 1,
            padding_right: 1,
            padding_top: 0,
            padding_bottom: 0,
        ) {
            Text(content: first_line, color: if selected { Color::Yellow } else { foreground }, weight: Weight::Bold, wrap: TextWrap::NoWrap)
            Text(content: second_line, color: Color::Grey, weight: Weight::Light, wrap: TextWrap::NoWrap)
        }
    }
    .into_any()
}

fn no_matching_sessions_label(model: &SessionsPickerModel) -> &'static str {
    if model.search.is_empty() {
        "No existing sessions match these filters"
    } else {
        "No matching sessions"
    }
}

fn start_new_args_label(model: &SessionsPickerModel) -> String {
    if model.request.new_session_args_display.is_empty() {
        "no extra args".to_owned()
    } else {
        format!("args: {}", model.request.new_session_args_display)
    }
}

fn render_session_header(width: usize) -> AnyElement<'static> {
    let title_width = width.saturating_sub(18).max(14);
    element! {
        View(
            width: width as u32,
            border_style: BorderStyle::Single,
            border_edges: Edges::Bottom,
            border_color: Color::DarkGrey,
        ) {
            Text(
                content: fit_line(&format!("  {:<title_width$} {:>6} {:>6}", "Session", "Upd", "New"), width),
                color: Color::Cyan,
                weight: Weight::Bold,
            )
        }
    }
    .into_any()
}

fn list_gap() -> AnyElement<'static> {
    element! {
        View(height: 1) {
            Text(content: "")
        }
    }
    .into_any()
}

fn render_record_row(
    record: &SessionPickerRecord,
    selected: bool,
    width: usize,
) -> AnyElement<'static> {
    let foreground = if selected { Color::White } else { Color::Grey };
    let metadata = if selected {
        Color::Grey
    } else {
        Color::DarkGrey
    };
    let title_prefix = if selected { "❯ " } else { "  " };
    let title_width = width.saturating_sub(18).max(14);
    let title = truncate_end(&record.title, title_width);
    let first_line = fit_line(
        &format!(
            "{title_prefix}{:<title_width$} {:>6} {:>6}",
            title,
            compact_age(&record.recency),
            compact_age(&record.created)
        ),
        width.saturating_sub(2),
    );
    let cwd = record.cwd.as_deref().unwrap_or("-");
    let metadata_line = if width < 43 {
        format!("    ⎇ {:<10}  {cwd}", record.branch)
    } else {
        format!("    ⎇ {:<12}  📂 {cwd}", record.branch)
    };
    let second_line = fit_line(&metadata_line, width.saturating_sub(2));

    element! {
        View(
            width: width as u32,
            flex_direction: FlexDirection::Column,
            padding_left: 1,
            padding_right: 1,
            padding_top: 0,
            padding_bottom: 0,
        ) {
            Text(content: first_line, color: if selected { Color::Yellow } else { foreground }, weight: Weight::Bold, wrap: TextWrap::NoWrap)
            Text(content: second_line, color: metadata, weight: Weight::Light, wrap: TextWrap::NoWrap)
        }
    }
    .into_any()
}

fn render_details(
    record: &SessionPickerRecord,
    width: usize,
    selected_conversation: Option<&SessionConversationPreview>,
    height: usize,
) -> AnyElement<'static> {
    let conversation = selected_conversation.unwrap_or(&record.conversation);
    let panel_height = height.max(1);
    let conversation_rows = if conversation.snippets.is_empty() {
        vec![conversation_text(
            conversation
                .unavailable_reason
                .as_deref()
                .unwrap_or("history unavailable"),
            Color::DarkGrey,
        )]
    } else {
        conversation
            .snippets
            .iter()
            .map(|snippet| conversation_text(&format!("• {snippet}"), Color::Grey))
            .collect::<Vec<_>>()
    };

    element! {
        View(
            width: width as u32,
            height: panel_height as u32,
            flex_direction: FlexDirection::Column,
            border_style: BorderStyle::Single,
            border_color: Color::DarkGrey,
            overflow: Overflow::Hidden,
            padding_left: 1,
            padding_right: 1,
        ) {
            Text(content: "Conversation", color: Color::Cyan, weight: Weight::Bold)
            #(conversation_rows)
        }
    }
    .into_any()
}

fn conversation_text(value: &str, color: Color) -> AnyElement<'static> {
    element! {
        Text(
            content: value.to_owned(),
            color,
            weight: Weight::Normal,
            wrap: TextWrap::Wrap,
        )
    }
    .into_any()
}

fn detail_text(value: &str, width: usize, color: Color) -> AnyElement<'static> {
    element! {
        Text(
            content: fit_line(value, width),
            color,
            weight: Weight::Normal,
            wrap: TextWrap::NoWrap,
        )
    }
    .into_any()
}

fn render_footer(width: usize) -> AnyElement<'static> {
    let content = if width < NARROW_PICKER_WIDTH {
        "type search    enter resume    ctrl-n new"
    } else if width < 90 {
        "type search    enter resume    ctrl-n new    esc exit"
    } else {
        "type search  ↑/↓ select  enter  ctrl-n new  ctrl-s scope  ctrl-t threads  ctrl-o sort  esc"
    };
    element! {
        View(
            width: 100pct,
            flex_grow: 1.0,
            flex_direction: FlexDirection::Column,
            justify_content: JustifyContent::FlexEnd,
        ) {
            View(
                width: 100pct,
                border_style: BorderStyle::Single,
                border_edges: Edges::Top,
                border_color: Color::DarkGrey,
                padding_top: 0,
            ) {
                Text(
                    content: fit_line(content, width),
                    color: Color::Grey,
                    weight: Weight::Light,
                    wrap: TextWrap::NoWrap,
                )
            }
        }
    }
    .into_any()
}

fn root_label(root: SessionsPickerRoot) -> &'static str {
    match root {
        SessionsPickerRoot::Cwd => "📂 cwd",
        SessionsPickerRoot::Repo => "repo",
        SessionsPickerRoot::Any => "all",
    }
}

fn source_label(source: SessionsSource) -> &'static str {
    match source {
        SessionsSource::Interactive => "interactive",
        SessionsSource::All => "all",
        SessionsSource::Subagents => "subagents",
    }
}

fn sort_label(sort: SessionsSort) -> &'static str {
    match sort {
        SessionsSort::Updated => "updated",
        SessionsSort::Created => "created",
    }
}

fn fit_line(line: &str, width: usize) -> String {
    truncate_middle(&line.replace('\n', " "), width)
}

fn compact_age(value: &str) -> String {
    value
        .strip_suffix(" ago")
        .or_else(|| value.strip_prefix("in "))
        .unwrap_or(value)
        .to_owned()
}

fn truncate_end(value: &str, max_chars: usize) -> String {
    let character_count = value.chars().count();
    if character_count <= max_chars {
        return value.to_owned();
    }
    if max_chars <= 1 {
        return ".".to_owned();
    }
    let keep = max_chars - 1;
    format!("{}.", value.chars().take(keep).collect::<String>())
}

fn truncate_middle(value: &str, max_chars: usize) -> String {
    let character_count = value.chars().count();
    if character_count <= max_chars {
        return value.to_owned();
    }
    if max_chars <= 1 {
        return ".".to_owned();
    }
    let keep = max_chars - 1;
    let prefix_count = keep / 2;
    let suffix_count = keep - prefix_count;
    let prefix = value.chars().take(prefix_count).collect::<String>();
    let suffix = value
        .chars()
        .rev()
        .take(suffix_count)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    format!("{prefix}.{suffix}")
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
mod tests {
    use crossterm::event::MouseButton;
    use futures_util::StreamExt;
    use iocraft::prelude::*;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::presentation::session_picker::request::SessionsPickerRoot as SessionsRoot;
    use crate::presentation::session_picker::test_support::picker_record;
    use crate::presentation::session_picker::test_support::picker_request;
    use crate::sessions::SessionConversationPreview;
    use crate::sessions::SessionPickerRecord;
    use crate::sessions::SessionsProvider;
    use crate::sessions::SessionsSort;
    use crate::sessions::SessionsSource;

    #[tokio::test]
    async fn session_record_reload_worker_runs_single_flight_and_keeps_only_latest_pending_query() {
        let initial_request = SessionRecordsReloadRequest {
            generation: 0,
            query: reload_query("initial"),
        };
        let (sender, receiver) = tokio::sync::watch::channel(initial_request);
        let (release_first_sender, release_first_receiver) = mpsc::channel::<()>();
        let release_first_receiver = Arc::new(Mutex::new(release_first_receiver));
        let (started_sender, mut started_receiver) = tokio::sync::mpsc::unbounded_channel();
        let active_loads = Arc::new(AtomicUsize::new(0));
        let maximum_active_loads = Arc::new(AtomicUsize::new(0));
        let loader: SessionsPickerRecordLoader = Arc::new({
            let active_loads = Arc::clone(&active_loads);
            let maximum_active_loads = Arc::clone(&maximum_active_loads);
            let release_first_receiver = Arc::clone(&release_first_receiver);
            move |query| {
                let active = active_loads.fetch_add(1, Ordering::SeqCst) + 1;
                maximum_active_loads.fetch_max(active, Ordering::SeqCst);
                let search = query.search;
                let _ = started_sender.send(search.clone());
                if search == "a" {
                    release_first_receiver
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .recv()
                        .unwrap_or_else(|error| {
                            panic!("first load release should arrive: {error}")
                        });
                }
                active_loads.fetch_sub(1, Ordering::SeqCst);
                Ok(vec![picker_record(
                    &format!("thread-{search}"),
                    &format!("result {search}"),
                    "/repo/project-a",
                    "codex-router",
                    "cli",
                )])
            }
        });
        let current_generation = Arc::new(AtomicU64::new(0));
        let (accepted_sender, mut accepted_receiver) =
            tokio::sync::mpsc::unbounded_channel::<String>();
        let worker = tokio::spawn({
            let current_generation = Arc::clone(&current_generation);
            async move {
                run_session_record_reload_worker(receiver, loader, move |request, _records| {
                    if current_generation.load(Ordering::SeqCst) == request.generation {
                        let _ = accepted_sender.send(request.query.search);
                    }
                })
                .await;
            }
        });

        current_generation.store(1, Ordering::SeqCst);
        sender.send_replace(SessionRecordsReloadRequest {
            generation: 1,
            query: reload_query("a"),
        });
        assert_eq!(started_receiver.recv().await.as_deref(), Some("a"));

        current_generation.store(2, Ordering::SeqCst);
        sender.send_replace(SessionRecordsReloadRequest {
            generation: 2,
            query: reload_query("b"),
        });
        current_generation.store(3, Ordering::SeqCst);
        sender.send_replace(SessionRecordsReloadRequest {
            generation: 3,
            query: reload_query("c"),
        });
        release_first_sender
            .send(())
            .unwrap_or_else(|error| panic!("first load should release: {error}"));

        assert_eq!(started_receiver.recv().await.as_deref(), Some("c"));
        assert_eq!(accepted_receiver.recv().await.as_deref(), Some("c"));
        assert_eq!(maximum_active_loads.load(Ordering::SeqCst), 1);
        assert!(
            started_receiver.try_recv().is_err(),
            "obsolete B must not run"
        );

        worker.abort();
    }

    fn reload_query(search: &str) -> SessionsPickerDataQuery {
        SessionsPickerDataQuery {
            root: SessionsRoot::Any,
            provider: SessionsProvider::Any,
            source: SessionsSource::All,
            sort: SessionsSort::Updated,
            search: search.to_owned(),
        }
    }

    #[tokio::test]
    async fn sessions_picker_iocraft_mock_terminal_handles_keys() {
        let mut selected_outcome = Option::<SessionsPickerOutcome>::None;
        let actual = element! {
            SessionsPickerComponent(
                request: picker_request(),
                width: 100usize,
                selected_outcome_out: &mut selected_outcome,
            )
        }
        .mock_terminal_render_loop(MockTerminalConfig::with_events(futures_util::stream::iter(
            vec![
                ctrl_key('s'),
                ctrl_key('s'),
                TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Down)),
                TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Enter)),
            ],
        )))
        .map(|canvas| canvas.to_string())
        .collect::<Vec<_>>()
        .await;

        assert!(
            actual
                .last()
                .is_some_and(|snapshot| snapshot.contains("Provider migration")),
            "picker should render the selected row before exiting: {actual:?}"
        );
        assert_eq!(
            selected_outcome,
            Some(SessionsPickerOutcome::ResumeSession("thread-b".to_owned()))
        );
    }

    #[tokio::test]
    async fn sessions_picker_existing_row_pointer_focus_updates_conversation_without_activation() {
        let mut selected_outcome = Option::<SessionsPickerOutcome>::None;
        let frames = element! {
            SessionsPickerComponent(
                request: capture_picker_request(),
                width: 160usize,
                height: 24usize,
                selected_outcome_out: &mut selected_outcome,
            )
        }
        .mock_terminal_render_loop(MockTerminalConfig::with_events(futures_util::stream::iter(
            vec![
                TerminalEvent::FullscreenMouse(FullscreenMouseEvent::new(
                    MouseEventKind::Moved,
                    10,
                    14,
                )),
                TerminalEvent::FullscreenMouse(FullscreenMouseEvent::new(
                    MouseEventKind::Down(MouseButton::Left),
                    10,
                    14,
                )),
                TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Esc)),
            ],
        )))
        .map(|canvas| canvas.to_string())
        .collect::<Vec<_>>()
        .await;

        assert_eq!(selected_outcome, None);
        assert!(
            frames.iter().any(|frame| {
                frame.contains("❯ Provider migration")
                    && frame.contains(
                        "Provider migration with very very long provider metadata first real",
                    )
            }),
            "pointer focus should update the existing-session conversation without activation: {frames:?}"
        );
    }

    #[tokio::test]
    async fn sessions_picker_start_new_hover_focuses_preview_without_activation() {
        let mut selected_outcome = Option::<SessionsPickerOutcome>::None;
        let frames = element! {
            SessionsPickerComponent(
                request: capture_picker_request(),
                width: 160usize,
                height: 24usize,
                selected_outcome_out: &mut selected_outcome,
            )
        }
        .mock_terminal_render_loop(MockTerminalConfig::with_events(futures_util::stream::iter(
            vec![
                TerminalEvent::FullscreenMouse(FullscreenMouseEvent::new(
                    MouseEventKind::Moved,
                    10,
                    7,
                )),
                TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Esc)),
            ],
        )))
        .map(|canvas| canvas.to_string())
        .collect::<Vec<_>>()
        .await;

        assert_eq!(selected_outcome, None);
        assert!(
            frames.iter().any(|frame| {
                frame.contains("❯ Start new session") && frame.contains("no extra args")
            }),
            "Start New hover should focus its preview without activation: {frames:?}"
        );
    }

    #[tokio::test]
    async fn sessions_picker_enter_resumes_pointer_focused_existing_session() {
        let mut selected_outcome = Option::<SessionsPickerOutcome>::None;
        let _frames = element! {
            SessionsPickerComponent(
                request: capture_picker_request(),
                width: 160usize,
                height: 24usize,
                selected_outcome_out: &mut selected_outcome,
            )
        }
        .mock_terminal_render_loop(MockTerminalConfig::with_events(futures_util::stream::iter(
            vec![
                TerminalEvent::FullscreenMouse(FullscreenMouseEvent::new(
                    MouseEventKind::Down(MouseButton::Left),
                    10,
                    14,
                )),
                TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Enter)),
            ],
        )))
        .collect::<Vec<_>>()
        .await;

        assert_eq!(
            selected_outcome,
            Some(SessionsPickerOutcome::ResumeSession("thread-b".to_owned()))
        );
    }

    #[tokio::test]
    async fn sessions_picker_start_new_click_activates_immediately() {
        let mut selected_outcome = Option::<SessionsPickerOutcome>::None;
        let _frames = element! {
            SessionsPickerComponent(
                request: capture_picker_request(),
                width: 160usize,
                height: 24usize,
                selected_outcome_out: &mut selected_outcome,
            )
        }
        .mock_terminal_render_loop(MockTerminalConfig::with_events(futures_util::stream::iter(
            vec![TerminalEvent::FullscreenMouse(FullscreenMouseEvent::new(
                MouseEventKind::Down(MouseButton::Left),
                10,
                7,
            ))],
        )))
        .collect::<Vec<_>>()
        .await;

        assert_eq!(
            selected_outcome,
            Some(SessionsPickerOutcome::StartNewSession)
        );
    }

    #[tokio::test]
    async fn sessions_picker_non_left_row_events_do_not_focus_or_activate() {
        let mut selected_outcome = Option::<SessionsPickerOutcome>::None;
        let _frames = element! {
            SessionsPickerComponent(
                request: capture_picker_request(),
                width: 160usize,
                height: 24usize,
                selected_outcome_out: &mut selected_outcome,
            )
        }
        .mock_terminal_render_loop(MockTerminalConfig::with_events(futures_util::stream::iter(
            vec![
                TerminalEvent::FullscreenMouse(FullscreenMouseEvent::new(
                    MouseEventKind::Down(MouseButton::Right),
                    10,
                    14,
                )),
                TerminalEvent::FullscreenMouse(FullscreenMouseEvent::new(
                    MouseEventKind::Up(MouseButton::Left),
                    10,
                    14,
                )),
                TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Enter)),
            ],
        )))
        .collect::<Vec<_>>()
        .await;

        assert_eq!(
            selected_outcome,
            Some(SessionsPickerOutcome::ResumeSession("thread-a".to_owned())),
            "ignored pointer events must leave the initial existing-session focus unchanged"
        );
    }

    #[tokio::test]
    async fn sessions_picker_hover_keeps_scrolled_row_under_pointer_until_click_and_enter() {
        let mut selected_outcome = Option::<SessionsPickerOutcome>::None;
        let events = futures_util::stream::iter(vec![
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::End)),
            TerminalEvent::FullscreenMouse(FullscreenMouseEvent::new(MouseEventKind::Moved, 10, 8)),
            TerminalEvent::FullscreenMouse(FullscreenMouseEvent::new(
                MouseEventKind::Down(MouseButton::Left),
                10,
                8,
            )),
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Enter)),
        ])
        .then(|event| async move {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            event
        });
        let frames = element! {
            SessionsPickerComponent(
                request: capture_picker_request(),
                width: 160usize,
                height: 24usize,
                selected_outcome_out: &mut selected_outcome,
            )
        }
        .mock_terminal_render_loop(MockTerminalConfig::with_events(events))
        .map(|canvas| canvas.to_string())
        .collect::<Vec<_>>()
        .await;

        assert!(
            frames.iter().any(|frame| {
                frame.contains("❯ Follow-up implementation lane 4")
                    && frame.contains("+8 more above")
            }),
            "pointer focus should not rewindow a scrolled row before mouse-down: {frames:?}"
        );
        assert_eq!(
            selected_outcome,
            Some(SessionsPickerOutcome::ResumeSession(
                "thread-extra-4".to_owned()
            )),
            "Enter must resume the stable session that remained under the pointer"
        );
    }

    #[tokio::test]
    async fn sessions_picker_iocraft_mock_terminal_ctrl_shortcuts_drive_filters() {
        let actual = element! {
            SessionsPickerComponent(
                request: picker_request(),
                width: 100usize,
            )
        }
        .mock_terminal_render_loop(MockTerminalConfig::with_events(futures_util::stream::iter(
            vec![
                ctrl_key('s'),
                ctrl_key('t'),
                ctrl_key('o'),
                TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Esc)),
            ],
        )))
        .map(|canvas| canvas.to_string())
        .collect::<Vec<_>>()
        .await;

        assert!(
            actual
                .iter()
                .any(|snapshot| snapshot.contains("[repo]    Threads: [all]    Sort: [created]")),
            "ctrl shortcuts should cycle scope, threads, and sort: {actual:?}"
        );
    }

    #[tokio::test]
    async fn sessions_picker_filter_shortcuts_reload_records_from_query_source() {
        let observed_queries = Arc::new(Mutex::new(Vec::<SessionsPickerDataQuery>::new()));
        let loader_called = Arc::new(tokio::sync::Notify::new());
        let loader_queries = Arc::clone(&observed_queries);
        let loader_called_from_blocking_task = Arc::clone(&loader_called);
        let record_loader: SessionsPickerRecordLoader = Arc::new(move |query| {
            loader_queries
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(query);
            loader_called_from_blocking_task.notify_one();
            Ok(vec![picker_record(
                "thread-reloaded",
                "Reloaded SQL result",
                "/repo/project-a",
                "codex-router",
                "subagent",
            )])
        });
        let mut request = picker_request();
        request.records = vec![picker_record(
            "thread-initial",
            "Initial SQL result",
            "/repo/project-a",
            "codex-router",
            "cli",
        )];

        let mut selected_outcome = Option::<SessionsPickerOutcome>::None;
        let events = futures_util::stream::once(async { ctrl_key('t') }).chain(
            futures_util::stream::once(async move {
                let _ = tokio::time::timeout(
                    std::time::Duration::from_secs(2),
                    loader_called.notified(),
                )
                .await;
                tokio::task::yield_now().await;
                tokio::task::yield_now().await;
                TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Esc))
            }),
        );
        let actual = element! {
            SessionsPickerComponent(
                request,
                record_loader: Some(record_loader),
                width: 100usize,
                selected_outcome_out: &mut selected_outcome,
            )
        }
        .mock_terminal_render_loop(MockTerminalConfig::with_events(events))
        .map(|canvas| canvas.to_string())
        .collect::<Vec<_>>()
        .await;

        let queries = observed_queries
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        assert!(
            queries
                .iter()
                .any(|query| query.source == SessionsSource::All),
            "ctrl-t should reload records for the next thread-source query: {queries:?}"
        );
        assert!(
            actual
                .iter()
                .any(|snapshot| snapshot.contains("Reloaded SQL result")),
            "the current reload result should become visible in the wired component: {actual:?}"
        );
    }

    #[tokio::test]
    async fn sessions_picker_iocraft_mock_terminal_ctrl_n_starts_new_thread() {
        let mut selected_outcome = None;
        let _actual = element! {
            SessionsPickerComponent(
                request: picker_request(),
                width: 100usize,
                selected_outcome_out: &mut selected_outcome,
            )
        }
        .mock_terminal_render_loop(MockTerminalConfig::with_events(futures_util::stream::iter(
            vec![ctrl_key('n')],
        )))
        .collect::<Vec<_>>()
        .await;

        assert_eq!(
            selected_outcome,
            Some(SessionsPickerOutcome::StartNewSession)
        );
    }

    #[tokio::test]
    async fn sessions_picker_iocraft_mock_terminal_esc_clears_search_before_exit() {
        let mut selected_outcome: Option<SessionsPickerOutcome> = None;
        let actual = element! {
            SessionsPickerComponent(
                request: picker_request(),
                width: 100usize,
                selected_outcome_out: &mut selected_outcome,
            )
        }
        .mock_terminal_render_loop(MockTerminalConfig::with_events(futures_util::stream::iter(
            vec![
                TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Char('r'))),
                TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Char('u'))),
                TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Char('s'))),
                TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Char('t'))),
                TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Esc)),
                TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Esc)),
            ],
        )))
        .map(|canvas| canvas.to_string())
        .collect::<Vec<_>>()
        .await;

        assert!(
            actual
                .iter()
                .any(|snapshot| snapshot.contains("Search text, id:, b:branch, repo:name")),
            "first escape should clear search instead of exiting immediately: {actual:?}"
        );
        assert_eq!(selected_outcome, None);
    }

    #[tokio::test]
    async fn sessions_picker_iocraft_mock_terminal_ctrl_c_and_ctrl_d_exit() {
        for key in ['c', 'd'] {
            let mut selected_outcome: Option<SessionsPickerOutcome> = None;
            let actual = element! {
                SessionsPickerComponent(
                    request: picker_request(),
                    width: 100usize,
                    selected_outcome_out: &mut selected_outcome,
                )
            }
            .mock_terminal_render_loop(MockTerminalConfig::with_events(futures_util::stream::iter(
                vec![ctrl_key(key)],
            )))
            .collect::<Vec<_>>()
            .await;

            assert!(!actual.is_empty(), "ctrl-{key} should render before exit");
            assert_eq!(selected_outcome, None, "ctrl-{key} should cancel picker");
        }
    }

    #[tokio::test]
    async fn sessions_picker_iocraft_mock_terminal_search_keeps_plain_letters() {
        let actual = element! {
            SessionsPickerComponent(
                request: picker_request(),
                width: 100usize,
            )
        }
        .mock_terminal_render_loop(MockTerminalConfig::with_events(futures_util::stream::iter(
            vec![
                TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Char('r'))),
                TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Char('u'))),
                TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Char('s'))),
                TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Char('t'))),
                TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Esc)),
                TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Esc)),
            ],
        )))
        .map(|canvas| canvas.to_string())
        .collect::<Vec<_>>()
        .await;

        assert!(
            actual
                .iter()
                .any(|snapshot| snapshot.contains("[📂 cwd]    Threads: [interactive]")),
            "plain search input should leave filters unchanged: {actual:?}"
        );
    }

    #[tokio::test]
    async fn sessions_picker_selected_row_uses_contract_marker_and_metadata() {
        let actual = element! {
            SessionsPickerComponent(
                request: picker_request(),
                width: 100usize,
            )
        }
        .mock_terminal_render_loop(MockTerminalConfig::with_events(futures_util::stream::iter(
            vec![
                TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Down)),
                TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Esc)),
            ],
        )))
        .collect::<Vec<_>>()
        .await;
        let Some(canvas) = actual.last() else {
            panic!("picker should render a canvas");
        };
        let snapshot = canvas.to_string();
        assert!(
            snapshot.contains("❯ Feature design session"),
            "selected row should use the contracted focus marker: {canvas}"
        );
        assert!(
            snapshot.contains("⎇ main") && snapshot.contains("/repo/project-a"),
            "selected row should keep branch and cwd on the metadata row: {canvas}"
        );
        assert!(
            snapshot.contains('╭') && snapshot.contains('╰'),
            "picker should render an iocraft bordered panel: {canvas}"
        );
    }

    #[tokio::test]
    async fn sessions_picker_start_new_row_uses_outline_instead_of_filled_background() {
        let mut request = picker_request();
        request.new_session_args_display =
            "--router-root /Users/shravansunder/.codex-router".to_owned();
        let text = render_picker_capture(
            request,
            100,
            vec![TerminalEvent::Key(KeyEvent::new(
                KeyEventKind::Press,
                KeyCode::Esc,
            ))],
        )
        .await;
        let lines = text.lines().collect::<Vec<_>>();
        let start_index = lines
            .iter()
            .position(|line| line.contains("Start new session"))
            .unwrap_or_else(|| panic!("start-new row should render:\n{text}"));

        assert!(
            lines
                .get(start_index.saturating_sub(1))
                .is_some_and(|line| line.contains('┌') && line.contains('┐')),
            "start-new row should have a thin outline top border:\n{text}"
        );
        assert!(
            lines
                .get(start_index + 2)
                .is_some_and(|line| line.contains('└') && line.contains('┘')),
            "start-new row should have a thin outline bottom border:\n{text}"
        );
        assert!(
            !lines[start_index].contains('█'),
            "start-new row should not read as a filled selected row:\n{text}"
        );
    }

    #[test]
    fn selected_conversation_preview_requests_background_load_without_reading_jsonl() {
        let mut record = picker_request().records.remove(0);
        record.conversation = SessionConversationPreview::unavailable("history loading");
        let source = SessionConversationSource::for_test(
            "/tmp/codex-router-history.jsonl",
            "/tmp/codex-router".into(),
        );
        record.conversation_source = Some(source.clone());
        let cache = BTreeMap::new();

        let preview = selected_conversation_preview_for_record(&record, &cache);

        assert_eq!(
            preview,
            SelectedConversationPreview {
                preview: SessionConversationPreview::unavailable("history loading"),
                load_request: Some(ConversationPreviewLoadRequest {
                    session_id: "thread-a".to_owned(),
                    source,
                }),
            }
        );
        assert!(cache.is_empty(), "render decision should not mutate cache");
    }

    #[tokio::test]
    async fn sessions_picker_iocraft_mock_terminal_too_narrow_exits_without_selection() {
        let mut selected_outcome = None;
        let actual = element! {
            SessionsPickerComponent(
                request: picker_request(),
                width: 20usize,
                selected_outcome_out: &mut selected_outcome,
            )
        }
        .mock_terminal_render_loop(MockTerminalConfig::with_events(futures_util::stream::iter(
            vec![TerminalEvent::Key(KeyEvent::new(
                KeyEventKind::Press,
                KeyCode::Enter,
            ))],
        )))
        .map(|canvas| canvas.to_string())
        .collect::<Vec<_>>()
        .await;

        assert_eq!(
            selected_outcome,
            Some(SessionsPickerOutcome::TerminalTooNarrow)
        );
        assert!(
            actual
                .last()
                .is_some_and(|snapshot| snapshot.contains("terminal too narrow")),
            "too-narrow picker should render only the concise error: {actual:?}"
        );
    }

    #[tokio::test]
    async fn sessions_picker_width_contract_preserves_layout() {
        for width in [48, 80, 120] {
            let text = render_picker_capture(
                capture_picker_request(),
                width,
                vec![
                    TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Down)),
                    TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Enter)),
                ],
            )
            .await;
            assert!(
                text.lines().all(|line| line.chars().count() <= width),
                "session picker capture width {width} overflowed:\n{text}"
            );
            assert!(
                text.lines()
                    .any(|line| line.contains("❯ Provider migration")),
                "capture should select the long-title row:\n{text}"
            );
            assert!(
                text.contains("+"),
                "capture should show the more-below affordance:\n{text}"
            );
            assert!(
                text.contains("ctrl-n new"),
                "capture should expose the new-thread shortcut:\n{text}"
            );
            let lines = text.lines().collect::<Vec<_>>();
            let footer_index = lines
                .iter()
                .position(|line| line.contains("type search"))
                .unwrap_or_else(|| panic!("capture should render footer:\n{text}"));
            let bottom_border_index = lines
                .iter()
                .rposition(|line| line.contains('╰'))
                .unwrap_or_else(|| panic!("capture should render bottom border:\n{text}"));
            assert_eq!(
                bottom_border_index,
                footer_index + 1,
                "picker outer border should sit directly below footer at width {width}:\n{text}"
            );
        }

        let mut empty_request = picker_request();
        empty_request.records.clear();
        let empty_text = render_picker_capture(
            empty_request,
            80,
            vec![TerminalEvent::Key(KeyEvent::new(
                KeyEventKind::Press,
                KeyCode::Esc,
            ))],
        )
        .await;
        assert!(
            empty_text.contains("Start new session"),
            "empty state should offer a new session:\n{empty_text}"
        );
    }

    #[tokio::test]
    async fn sessions_picker_stacked_details_wraps_conversation_without_preview_or_metadata() {
        let mut request = capture_picker_request();
        let record = request
            .records
            .get_mut(0)
            .unwrap_or_else(|| panic!("capture request should have a record"));
        record.conversation.snippets = vec![format!(
            "{} final-conversation-marker",
            "conversation words that should use the available detail width ".repeat(3)
        )];

        let width = 120;
        let text = render_picker_capture(
            request,
            width,
            vec![TerminalEvent::Key(KeyEvent::new(
                KeyEventKind::Press,
                KeyCode::Esc,
            ))],
        )
        .await;

        assert!(
            text.lines().all(|line| line.chars().count() <= width),
            "wrapped conversation should not overflow stacked details frame:\n{text}"
        );
        assert!(text.contains("final-conversation-marker"), "{text}");
        assert!(!text.contains("Preview"), "{text}");
        assert!(!text.contains("Metadata"), "{text}");
    }

    #[tokio::test]
    async fn sessions_picker_renders_minimum_height_from_short_resize() {
        let text = render_picker_capture_at(
            capture_picker_request(),
            160,
            12,
            vec![TerminalEvent::Key(KeyEvent::new(
                KeyEventKind::Press,
                KeyCode::Esc,
            ))],
        )
        .await;

        assert_eq!(
            meaningful_line_count(&text),
            24,
            "short terminals should still render the 24-row minimum:\n{text}"
        );
    }

    #[tokio::test]
    async fn sessions_picker_uses_taller_height_for_more_visible_rows() {
        let short_text = render_picker_capture_at(
            capture_picker_request(),
            160,
            24,
            vec![TerminalEvent::Key(KeyEvent::new(
                KeyEventKind::Press,
                KeyCode::Esc,
            ))],
        )
        .await;
        let tall_text = render_picker_capture_at(
            capture_picker_request(),
            160,
            32,
            vec![TerminalEvent::Key(KeyEvent::new(
                KeyEventKind::Press,
                KeyCode::Esc,
            ))],
        )
        .await;

        let short_rows = visible_followup_row_count(&short_text);
        let tall_rows = visible_followup_row_count(&tall_text);
        assert!(
            tall_rows > short_rows,
            "taller sessions view should spend height on visible rows before blank space; short={short_rows}, tall={tall_rows}\nshort:\n{short_text}\ntall:\n{tall_text}"
        );
    }

    #[tokio::test]
    async fn sessions_picker_live_resize_uses_terminal_height_and_taller_heights() {
        let short_text = render_picker_capture_at(
            capture_picker_request(),
            0,
            0,
            vec![
                TerminalEvent::Resize(160, 12),
                TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Esc)),
            ],
        )
        .await;
        let tall_text = render_picker_capture_at(
            capture_picker_request(),
            0,
            0,
            vec![
                TerminalEvent::Resize(160, 32),
                TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Esc)),
            ],
        )
        .await;

        assert_eq!(
            meaningful_line_count(&short_text),
            12,
            "live resize below 24 rows should respect terminal height:\n{short_text}"
        );
        let short_rows = visible_followup_row_count(&short_text);
        let tall_rows = visible_followup_row_count(&tall_text);
        assert!(
            tall_rows > short_rows,
            "live resize to a taller terminal should render more rows; short={short_rows}, tall={tall_rows}\nshort:\n{short_text}\ntall:\n{tall_text}"
        );
    }

    #[tokio::test]
    async fn sessions_picker_sidecar_clamps_tall_details_to_body_budget() {
        let mut request = capture_picker_request();
        let record = request
            .records
            .get_mut(0)
            .unwrap_or_else(|| panic!("capture request should have a record"));
        record.conversation.snippets = vec![
            "first long sidecar snippet".to_owned(),
            "second long sidecar snippet".to_owned(),
            "third long sidecar snippet".to_owned(),
            "fourth long sidecar snippet".to_owned(),
        ];
        let text = render_picker_capture_at(
            request,
            160,
            24,
            vec![TerminalEvent::Key(KeyEvent::new(
                KeyEventKind::Press,
                KeyCode::Esc,
            ))],
        )
        .await;

        assert_eq!(
            meaningful_line_count(&text),
            24,
            "sidecar details should stay within the 24-row frame:\n{text}"
        );
        assert!(
            text.contains("type search"),
            "sidecar details should not clip the footer at 160x24:\n{text}"
        );
    }

    #[tokio::test]
    async fn sessions_picker_sidecar_panels_reach_footer_without_unframed_gap() {
        let text = render_picker_capture_at(
            capture_picker_request(),
            160,
            32,
            vec![TerminalEvent::Key(KeyEvent::new(
                KeyEventKind::Press,
                KeyCode::Esc,
            ))],
        )
        .await;
        let lines = text.lines().collect::<Vec<_>>();
        let footer_index = lines
            .iter()
            .position(|line| line.contains("type search"))
            .unwrap_or_else(|| panic!("sidecar footer should render:\n{text}"));
        let panel_bottom_index = lines[..footer_index]
            .iter()
            .rposition(|line| line.matches('└').count() >= 2)
            .unwrap_or_else(|| panic!("both sidecar panel bottoms should render:\n{text}"));

        assert_eq!(
            panel_bottom_index + 2,
            footer_index,
            "sidecar panels should meet the footer divider without an unframed gap:\n{text}"
        );
    }

    #[tokio::test]
    async fn sessions_picker_renders_one_line_controls_when_width_allows() {
        let text = render_picker_capture_at(
            capture_picker_request(),
            160,
            24,
            vec![TerminalEvent::Key(KeyEvent::new(
                KeyEventKind::Press,
                KeyCode::Esc,
            ))],
        )
        .await;

        assert!(
            text.lines().any(|line| {
                line.contains("Search text, id:, b:branch, repo:name")
                    && line.contains("[all]")
                    && line.contains("Threads:")
                    && line.contains("Sort:")
            }),
            "wide sessions controls should fit on one line:\n{text}"
        );
    }

    #[tokio::test]
    async fn sessions_picker_budgets_wrapped_controls_from_actual_search_text() {
        let mut events = "012345678901234567890123456789012345678901234567890123456789012345678901234567890123456789"
            .chars()
            .map(|character| {
                TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Char(character)))
            })
            .collect::<Vec<_>>();
        events.push(TerminalEvent::Key(KeyEvent::new(
            KeyEventKind::Press,
            KeyCode::Esc,
        )));
        events.push(TerminalEvent::Key(KeyEvent::new(
            KeyEventKind::Press,
            KeyCode::Esc,
        )));
        let text = render_picker_capture_at(capture_picker_request(), 100, 24, events).await;

        assert_eq!(
            meaningful_line_count(&text),
            24,
            "long search controls should not grow the picker past the 24-row frame:\n{text}"
        );
        assert!(
            text.contains("type search"),
            "wrapped controls should not clip the footer at 100x24:\n{text}"
        );
        assert!(
            text.lines().all(|line| line.chars().count() <= 100),
            "wrapped controls should fit the terminal width:\n{text}"
        );
    }

    #[tokio::test]
    async fn sessions_picker_stacked_layout_removes_top_padding_and_dead_list_tail() {
        let text = render_picker_capture_at(
            capture_picker_request(),
            120,
            24,
            vec![TerminalEvent::Key(KeyEvent::new(
                KeyEventKind::Press,
                KeyCode::Esc,
            ))],
        )
        .await;
        let lines = text.lines().collect::<Vec<_>>();
        let title_index = lines
            .iter()
            .position(|line| line.contains("Resume a previous session"))
            .unwrap_or_else(|| panic!("sessions title should render:\n{text}"));
        let top_border_index = lines
            .iter()
            .position(|line| line.contains('╭'))
            .unwrap_or_else(|| panic!("sessions top border should render:\n{text}"));
        assert_eq!(
            title_index,
            top_border_index + 1,
            "title should sit directly below the outer border:\n{text}"
        );

        let header_index = lines
            .iter()
            .position(|line| line.contains("Session") && line.contains("Upd"))
            .unwrap_or_else(|| panic!("sessions list header should render:\n{text}"));
        let list_top_border_index = lines[..header_index]
            .iter()
            .rposition(|line| line.contains('┌'))
            .unwrap_or_else(|| panic!("sessions list top border should render:\n{text}"));
        assert_eq!(
            header_index,
            list_top_border_index + 1,
            "list header should sit directly below the list border:\n{text}"
        );

        let more_below_index = lines
            .iter()
            .position(|line| line.contains("more below"))
            .unwrap_or_else(|| panic!("sessions list should render a more-below row:\n{text}"));
        let list_bottom_border_index = more_below_index + 1;
        assert!(
            lines
                .get(list_bottom_border_index)
                .is_some_and(|line| line.contains('└')),
            "sessions list bottom border should sit directly below the more-below row:\n{text}"
        );
        let row_before_bottom = lines
            .get(list_bottom_border_index.saturating_sub(1))
            .unwrap_or_else(|| panic!("sessions list should have content above bottom:\n{text}"));
        assert!(
            row_before_bottom.contains("Follow-up implementation lane")
                || row_before_bottom.contains("more below")
                || row_before_bottom.contains("more above"),
            "sessions list should not leave an empty tail above its bottom border:\n{text}"
        );
    }

    #[tokio::test]
    async fn sessions_picker_uses_sidecar_only_at_160_columns() {
        let stacked_text = render_picker_capture(
            capture_picker_request(),
            159,
            vec![TerminalEvent::Key(KeyEvent::new(
                KeyEventKind::Press,
                KeyCode::Esc,
            ))],
        )
        .await;
        assert!(
            !has_sidecar_details(&stacked_text),
            "session picker should stack details below 160 columns:\n{stacked_text}"
        );

        let sidecar_text = render_picker_capture(
            capture_picker_request(),
            160,
            vec![TerminalEvent::Key(KeyEvent::new(
                KeyEventKind::Press,
                KeyCode::Esc,
            ))],
        )
        .await;
        assert!(
            has_sidecar_details(&sidecar_text),
            "session picker should place details on the right at 160 columns:\n{sidecar_text}"
        );
    }

    #[tokio::test]
    async fn sessions_picker_reflows_when_terminal_width_changes() {
        let frames = element! {
            SessionsPickerComponent(
                request: capture_picker_request(),
                width: 0usize,
            )
        }
        .mock_terminal_render_loop(MockTerminalConfig::with_events(futures_util::stream::iter(
            vec![
                TerminalEvent::Resize(159, 40),
                TerminalEvent::Resize(160, 40),
                TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Esc)),
            ],
        )))
        .map(|canvas| canvas.to_string())
        .collect::<Vec<_>>()
        .await;

        assert!(
            frames.iter().any(|frame| !has_sidecar_details(frame)),
            "session picker should render a stacked frame after shrinking below 160 columns: {frames:?}"
        );
        assert!(
            frames.iter().any(|frame| has_sidecar_details(frame)),
            "session picker should render a sidecar frame after growing to 160 columns: {frames:?}"
        );
    }

    #[tokio::test]
    #[ignore = "writes visual session picker capture artifacts for design review"]
    async fn sessions_picker_capture_artifacts_for_design_review() {
        let capture_dir = capture_dir();

        for width in [48, 80, 120] {
            let text = render_picker_capture(
                capture_picker_request(),
                width,
                vec![
                    TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Down)),
                    TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Enter)),
                ],
            )
            .await;
            write_capture_pair(&capture_dir, &format!("sessions-{width}"), &text);
        }

        for (width, height) in [(160, 24), (160, 32), (100, 24)] {
            let text = render_picker_capture_at(
                capture_picker_request(),
                width,
                height,
                vec![TerminalEvent::Key(KeyEvent::new(
                    KeyEventKind::Press,
                    KeyCode::Esc,
                ))],
            )
            .await;
            write_capture_pair(&capture_dir, &format!("sessions-{width}x{height}"), &text);
        }

        let mut empty_request = picker_request();
        empty_request.records.clear();
        let empty_text = render_picker_capture(
            empty_request,
            80,
            vec![TerminalEvent::Key(KeyEvent::new(
                KeyEventKind::Press,
                KeyCode::Esc,
            ))],
        )
        .await;
        write_capture_pair(&capture_dir, "sessions-empty-80", &empty_text);
    }

    async fn render_picker_capture(
        request: SessionsPickerRequest,
        width: usize,
        events: Vec<TerminalEvent>,
    ) -> String {
        render_picker_capture_at(request, width, MIN_RENDER_HEIGHT, events).await
    }

    async fn render_picker_capture_at(
        request: SessionsPickerRequest,
        width: usize,
        height: usize,
        events: Vec<TerminalEvent>,
    ) -> String {
        let frames = element! {
            SessionsPickerComponent(
                request,
                width,
                height,
            )
        }
        .mock_terminal_render_loop(MockTerminalConfig::with_events(futures_util::stream::iter(
            events,
        )))
        .map(|canvas| canvas.to_string())
        .collect::<Vec<_>>()
        .await;

        frames
            .last()
            .cloned()
            .unwrap_or_else(|| panic!("picker should render at least one frame"))
    }

    fn meaningful_line_count(text: &str) -> usize {
        text.lines().count()
    }

    fn visible_followup_row_count(text: &str) -> usize {
        text.lines()
            .filter(|line| line.contains("Follow-up implementation lane"))
            .count()
    }

    fn has_sidecar_details(text: &str) -> bool {
        text.lines()
            .any(|line| line.matches('┌').count() >= 2 && line.matches('┐').count() >= 2)
    }

    fn ctrl_key(character: char) -> TerminalEvent {
        let mut event = KeyEvent::new(KeyEventKind::Press, KeyCode::Char(character));
        event.modifiers = KeyModifiers::CONTROL;
        TerminalEvent::Key(event)
    }

    fn capture_picker_request() -> SessionsPickerRequest {
        let mut request = picker_request();
        request.root = SessionsRoot::Any;
        request.source = SessionsSource::All;
        for index in 0..8 {
            request.records.push(capture_record(
                &format!("thread-extra-{index}"),
                &format!("Follow-up implementation lane {index}"),
                "/repo/project-a",
                "codex-router",
                "cli",
            ));
        }
        request
    }

    fn capture_record(
        session_id: &str,
        title: &str,
        cwd: &str,
        provider: &str,
        source: &str,
    ) -> SessionPickerRecord {
        SessionPickerRecord {
            session_id: session_id.to_owned(),
            title: title.to_owned(),
            full_title: title.to_owned(),
            recency: "now".to_owned(),
            created: "1d ago".to_owned(),
            recency_at_ms: Some(2_000),
            created_at_ms: Some(1_000),
            branch: "main".to_owned(),
            persisted_branch: "main".to_owned(),
            context: cwd.rsplit('/').next().unwrap_or(cwd).to_owned(),
            cwd: Some(cwd.to_owned()),
            normalized_cwd: Some(cwd.to_owned()),
            git_origin_url: Some("https://github.com/shravan-agent/codex-router.git".to_owned()),
            provider: Some(provider.to_owned()),
            model: Some("gpt-5-codex".to_owned()),
            preview: Some(format!("{title} preview text")),
            first_user_message: format!("{title} recent question"),
            conversation: SessionConversationPreview {
                snippets: vec![
                    format!("{title} recent question"),
                    format!("{title} recent answer"),
                ],
                unavailable_reason: None,
            },
            conversation_source: None,
            source: Some(source.to_owned()),
            thread_source: Some(source.to_owned()),
        }
    }

    fn capture_dir() -> PathBuf {
        let dir = std::env::var_os("CODEX_ROUTER_CAPTURE_DIR").map_or_else(
            || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tmp/ux-proof/production"),
            PathBuf::from,
        );
        must_ok(std::fs::create_dir_all(&dir));
        dir
    }

    fn write_capture_pair(dir: &Path, name: &str, text: &str) {
        must_ok(std::fs::write(dir.join(format!("{name}.txt")), text));
        must_ok(std::fs::write(
            dir.join(format!("{name}.svg")),
            terminal_svg(name, text),
        ));
    }

    fn terminal_svg(title: &str, text: &str) -> String {
        let lines = text.lines().collect::<Vec<_>>();
        let width = lines
            .iter()
            .map(|line| line.chars().count())
            .max()
            .unwrap_or(1);
        let height = lines.len().max(1);
        let pixel_width = width * 9 + 32;
        let pixel_height = height * 18 + 34;
        let mut svg = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{pixel_width}\" height=\"{pixel_height}\" viewBox=\"0 0 {pixel_width} {pixel_height}\"><rect width=\"100%\" height=\"100%\" fill=\"#111318\"/>"
        );
        svg.push_str(&format!(
            "<text x=\"16\" y=\"24\" xml:space=\"preserve\" font-family=\"SFMono-Regular, Menlo, Consolas, monospace\" font-size=\"14\" fill=\"#e6edf3\"><tspan>{}</tspan>",
            escape_xml(title)
        ));
        for (index, line) in lines.iter().enumerate() {
            svg.push_str(&format!(
                "<tspan x=\"16\" dy=\"{}\">{}</tspan>",
                if index == 0 { 20 } else { 18 },
                escape_xml(line)
            ));
        }
        svg.push_str("</text></svg>");
        svg
    }

    fn escape_xml(value: &str) -> String {
        value
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    }

    fn must_ok<T, E: std::fmt::Display>(result: Result<T, E>) -> T {
        match result {
            Ok(value) => value,
            Err(error) => panic!("expected Ok, got error: {error}"),
        }
    }
}

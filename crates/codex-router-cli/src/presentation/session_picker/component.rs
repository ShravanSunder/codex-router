use std::collections::BTreeMap;
use std::io;

use iocraft::prelude::*;

use crate::presentation::session_picker::action::SessionsPickerKey;
use crate::presentation::session_picker::action::SessionsPickerOutcome;
use crate::presentation::session_picker::model::SessionsPickerModel;
use crate::presentation::session_picker::render::MIN_PICKER_WIDTH;
use crate::presentation::session_picker::request::SessionsPickerRequest;
use crate::sessions::SessionConversationPreview;
use crate::sessions::SessionPickerRecord;
use crate::sessions::SessionsRoot;
use crate::sessions::SessionsSource;

const SIDECAR_PICKER_WIDTH: usize = 96;
const NARROW_PICKER_WIDTH: usize = 72;
const COMPACT_PICKER_WIDTH: usize = 56;
const MAX_VISIBLE_RECORDS: usize = 8;

#[derive(Default, Props)]
pub(crate) struct SessionsPickerComponentProps<'a> {
    request: SessionsPickerRequest,
    width: usize,
    selected_outcome_out: Option<&'a mut Option<SessionsPickerOutcome>>,
}

#[component]
pub(crate) fn SessionsPickerComponent<'a>(
    props: &mut SessionsPickerComponentProps<'a>,
    mut hooks: Hooks,
) -> impl Into<AnyElement<'static>> {
    let mut system = hooks.use_context_mut::<SystemContext>();
    let (terminal_width, _) = hooks.use_terminal_size();
    let width = if props.width == 0 {
        usize::from(terminal_width)
    } else {
        props.width
    };
    let mut model = hooks.use_state(|| SessionsPickerModel::new(props.request.clone(), width));
    let mut conversation_cache =
        hooks.use_state(BTreeMap::<String, SessionConversationPreview>::new);
    let mut selected_outcome = hooks.use_state(|| Option::<SessionsPickerOutcome>::None);
    let mut should_cancel = hooks.use_state(|| false);

    hooks.use_terminal_events({
        move |event| {
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
            match code {
                KeyCode::Down => model_value.handle_key(SessionsPickerKey::MoveDown),
                KeyCode::Up => model_value.handle_key(SessionsPickerKey::MoveUp),
                KeyCode::Tab => model_value.handle_key(SessionsPickerKey::CycleRoot),
                KeyCode::Char('n')
                    if modifiers.intersects(KeyModifiers::SUPER | KeyModifiers::CONTROL) =>
                {
                    selected_outcome.set(Some(SessionsPickerOutcome::StartNewSession));
                }
                KeyCode::Char('s') if modifiers.contains(KeyModifiers::CONTROL) => {
                    model_value.handle_key(SessionsPickerKey::CycleSource);
                }
                KeyCode::Backspace => model_value.handle_key(SessionsPickerKey::SearchBackspace),
                KeyCode::Char(character) => {
                    model_value.handle_key(SessionsPickerKey::SearchChar(character));
                }
                KeyCode::Enter => selected_outcome.set(model_value.selected_outcome()),
                KeyCode::Esc => should_cancel.set(true),
                _ => {}
            }
        }
    });

    if let Some(selected_outcome) = selected_outcome.read().clone() {
        if let Some(out) = props.selected_outcome_out.as_mut() {
            **out = Some(selected_outcome);
        }
        system.exit();
    } else if *should_cancel.read() {
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
        let selected = model_value
            .visible_records()
            .get(model_value.selected_index)
            .copied();
        selected.and_then(|record| {
            let source = record.conversation_source.as_deref()?;
            let mut cache = conversation_cache.write();
            Some(
                cache
                    .entry(record.session_id.clone())
                    .or_insert_with(|| SessionConversationPreview::from_rollout_path(Some(source)))
                    .clone(),
            )
        })
    };

    render_picker_view(&model.read(), selected_conversation.as_ref())
}

fn render_picker_view(
    model: &SessionsPickerModel,
    selected_conversation: Option<&SessionConversationPreview>,
) -> Element<'static, View> {
    let content_width = model.width.saturating_sub(4).max(MIN_PICKER_WIDTH);
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
    children.extend(render_filter_controls(model, content_width));

    let visible_records = model.visible_records();
    if visible_records.is_empty() {
        children.push(render_empty_state(model));
    } else {
        let selected_record = visible_records.get(model.selected_index).copied();
        if model.width >= SIDECAR_PICKER_WIDTH {
            let list_width = (content_width.saturating_sub(2) / 2).max(42);
            let detail_width = content_width.saturating_sub(list_width + 2).max(28);
            children.push(
                element! {
                    View(width: 100pct) {
                        #(render_session_list(&visible_records, model.selected_index, list_width))
                        View(width: 2) { Text(content: "") }
                        #(selected_record
                            .map(|record| render_details(record, detail_width, selected_conversation))
                            .unwrap_or_else(|| render_empty_state(model)))
                    }
                }
                .into_any(),
            );
        } else {
            children.push(render_session_list(
                &visible_records,
                model.selected_index,
                content_width,
            ));
            if model.width >= NARROW_PICKER_WIDTH
                && let Some(record) = selected_record
            {
                children.push(render_details(record, content_width, selected_conversation));
            }
        }
    }

    children.push(render_footer(content_width));

    element! {
        View(
            width: model.width as u32,
            border_style: BorderStyle::Round,
            border_color: Color::Cyan,
            padding_left: 1,
            padding_right: 1,
            padding_top: 1,
            padding_bottom: 1,
            flex_direction: FlexDirection::Column,
            row_gap: 0,
        ) {
            #(children)
        }
    }
}

fn render_filter_controls(model: &SessionsPickerModel, width: usize) -> Vec<AnyElement<'static>> {
    let filter = if model.search.is_empty() {
        "Type to search".to_owned()
    } else {
        format!("Search: [{}]", model.search)
    };
    let scope = format!("Scope: [{}]", root_label(model.root));
    let threads = format!("Threads: [{}]", source_label(model.source));
    let sort = "Sort: [updated]".to_owned();

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

fn render_empty_state(model: &SessionsPickerModel) -> AnyElement<'static> {
    element! {
        View(
            width: 100pct,
            flex_direction: FlexDirection::Column,
            background_color: Color::DarkGrey,
            padding_left: 1,
            padding_right: 1,
        ) {
            Text(
                content: "Start new session",
                color: Color::White,
                weight: Weight::Bold,
            )
            Text(
                content: if model.search.is_empty() {
                    "No existing sessions match these filters"
                } else {
                    "No matching sessions"
                },
                color: Color::Grey,
                weight: Weight::Normal,
            )
        }
    }
    .into_any()
}

fn render_session_list(
    visible_records: &[&SessionPickerRecord],
    selected_index: usize,
    width: usize,
) -> AnyElement<'static> {
    let row_width = width.saturating_sub(6).max(24);
    let mut rows = vec![render_session_header(row_width)];
    for (index, record) in visible_records.iter().take(MAX_VISIBLE_RECORDS).enumerate() {
        if index > 0 {
            rows.push(list_gap());
        }
        rows.push(render_record_row(
            record,
            index == selected_index,
            row_width,
        ));
    }
    if visible_records.len() > MAX_VISIBLE_RECORDS {
        rows.push(list_gap());
        rows.push(
            element! {
                Text(
                    content: format!("+{} more below", visible_records.len() - MAX_VISIBLE_RECORDS),
                    color: Color::DarkGrey,
                    weight: Weight::Light,
                )
            }
            .into_any(),
        );
    }

    element! {
        View(
            width: 100pct,
            flex_direction: FlexDirection::Column,
            border_style: BorderStyle::Single,
            border_color: Color::DarkGrey,
            padding_left: 1,
            padding_right: 1,
            padding_top: 1,
            padding_bottom: 1,
        ) {
            #(rows)
        }
    }
    .into_any()
}

fn render_session_header(width: usize) -> AnyElement<'static> {
    let title_width = width.saturating_sub(18).max(14);
    element! {
        View(
            width: 100pct,
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
        View(width: 100pct, height: 1) {
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
    let background_color = selected.then(|| Color::Rgb {
        r: 58,
        g: 70,
        b: 122,
    });
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
    let second_line = fit_line(
        &format!("    ⎇ {:<12}  📂 {cwd}", record.branch),
        width.saturating_sub(2),
    );

    element! {
        View(
            width: 100pct,
            flex_direction: FlexDirection::Column,
            background_color,
            padding_left: 1,
            padding_right: 1,
            padding_top: 0,
            padding_bottom: 0,
        ) {
            Text(content: first_line, color: if selected { Color::Yellow } else { foreground }, weight: Weight::Bold)
            Text(content: second_line, color: metadata, weight: Weight::Light)
        }
    }
    .into_any()
}

fn render_details(
    record: &SessionPickerRecord,
    width: usize,
    selected_conversation: Option<&SessionConversationPreview>,
) -> AnyElement<'static> {
    let detail_width = width.saturating_sub(2);
    let preview = record.preview.as_deref().unwrap_or(&record.title);
    let conversation = selected_conversation.unwrap_or(&record.conversation);
    let conversation_rows = if conversation.snippets.is_empty() {
        vec![detail_text(
            conversation
                .unavailable_reason
                .as_deref()
                .unwrap_or("history unavailable"),
            detail_width,
            Color::DarkGrey,
        )]
    } else {
        conversation
            .snippets
            .iter()
            .map(|snippet| detail_text(&format!("• {snippet}"), detail_width, Color::Grey))
            .collect::<Vec<_>>()
    };
    let metadata_rows = vec![
        detail_line(
            "provider",
            record.provider.as_deref().unwrap_or("-"),
            detail_width,
        ),
        detail_line(
            "model",
            record.model.as_deref().unwrap_or("-"),
            detail_width,
        ),
        detail_line(
            "thread",
            record.thread_source.as_deref().unwrap_or("-"),
            detail_width,
        ),
        detail_line(
            "source",
            record.source.as_deref().unwrap_or("-"),
            detail_width,
        ),
        detail_line("id", &short_id(&record.session_id), detail_width),
    ];

    element! {
        View(
            width: 100pct,
            flex_direction: FlexDirection::Column,
            border_style: BorderStyle::Single,
            border_color: Color::DarkGrey,
            padding_left: 1,
            padding_right: 1,
        ) {
            Text(content: "Preview", color: Color::Cyan, weight: Weight::Bold)
            Text(content: fit_line(preview, detail_width), color: Color::Yellow, weight: Weight::Bold)
            View(width: 100pct, border_style: BorderStyle::Single, border_edges: Edges::Bottom, border_color: Color::DarkGrey) {
                Text(content: "")
            }
            Text(content: "Conversation", color: Color::Cyan, weight: Weight::Bold)
            #(conversation_rows)
            View(width: 100pct, border_style: BorderStyle::Single, border_edges: Edges::Bottom, border_color: Color::DarkGrey) {
                Text(content: "")
            }
            Text(content: "Metadata", color: Color::Cyan, weight: Weight::Bold)
            #(metadata_rows)
        }
    }
    .into_any()
}

fn detail_text(value: &str, width: usize, color: Color) -> AnyElement<'static> {
    element! {
        Text(
            content: fit_line(value, width),
            color,
            weight: Weight::Normal,
        )
    }
    .into_any()
}

fn detail_line(label: &str, value: &str, width: usize) -> AnyElement<'static> {
    element! {
        Text(
            content: fit_line(&format!("{label:<9} {value}"), width),
            color: Color::Grey,
            weight: Weight::Normal,
        )
    }
    .into_any()
}

fn render_footer(width: usize) -> AnyElement<'static> {
    let content = if width < NARROW_PICKER_WIDTH {
        "type search    enter resume    ⌘N/Ctrl-N new thread"
    } else if width < 90 {
        "type search    enter resume    ⌘N/Ctrl-N new thread    esc exit"
    } else {
        "type search    enter resume    ⌘N/Ctrl-N new thread    esc exit    tab scope    ctrl-s threads"
    };
    element! {
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
            )
        }
    }
    .into_any()
}

fn root_label(root: SessionsRoot) -> &'static str {
    match root {
        SessionsRoot::Cwd => "📂 cwd",
        SessionsRoot::Checkout => "worktree",
        SessionsRoot::Repo => "repo",
        SessionsRoot::Any => "all",
    }
}

fn source_label(source: SessionsSource) -> &'static str {
    match source {
        SessionsSource::Interactive => "interactive",
        SessionsSource::All => "all",
        SessionsSource::Subagents => "subagents",
    }
}

fn short_id(session_id: &str) -> String {
    truncate_middle(session_id, 12)
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
) -> io::Result<Option<SessionsPickerOutcome>> {
    let mut selected_outcome = None;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(
        element! {
            SessionsPickerComponent(
                request: request,
                width: 0usize,
                selected_outcome_out: &mut selected_outcome,
            )
        }
        .render_loop(),
    )?;
    Ok(selected_outcome)
}

#[cfg(test)]
mod tests {
    use futures_util::StreamExt;
    use iocraft::prelude::*;
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::presentation::session_picker::test_support::picker_request;
    use crate::sessions::SessionConversationPreview;
    use crate::sessions::SessionPickerRecord;
    use crate::sessions::SessionsRoot;
    use crate::sessions::SessionsSource;

    #[tokio::test]
    async fn sessions_picker_iocraft_mock_terminal_handles_keys() {
        let mut selected_outcome = None;
        let actual = element! {
            SessionsPickerComponent(
                request: picker_request(),
                width: 100usize,
                selected_outcome_out: &mut selected_outcome,
            )
        }
        .mock_terminal_render_loop(MockTerminalConfig::with_events(futures_util::stream::iter(
            vec![
                TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Tab)),
                TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Tab)),
                TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Tab)),
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
            ],
        )))
        .map(|canvas| canvas.to_string())
        .collect::<Vec<_>>()
        .await;

        assert!(
            actual
                .iter()
                .any(|snapshot| snapshot.contains("Search: [rust]")),
            "plain filter letters should search, not switch filters: {actual:?}"
        );
        assert!(
            actual
                .iter()
                .any(|snapshot| snapshot.contains("Scope: [📂 cwd]    Threads: [interactive]")),
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
        let canvas = actual.last().expect("picker should render a canvas");
        let snapshot = canvas.to_string();
        assert!(
            snapshot.contains("❯ Feature design session"),
            "selected row should use the contracted focus marker: {canvas}"
        );
        assert!(
            snapshot.contains("⎇ main") && snapshot.contains("📂 /repo/project-a"),
            "selected row should keep branch and cwd on the metadata row: {canvas}"
        );
        assert!(
            canvas.to_string().contains('╭') && canvas.to_string().contains('╰'),
            "picker should render an iocraft bordered panel: {}",
            canvas
        );
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
                text.contains("⌘N/Ctrl-N new thread"),
                "capture should expose the new-thread shortcut:\n{text}"
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
        let frames = element! {
            SessionsPickerComponent(
                request,
                width,
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
            recency: "now".to_owned(),
            created: "1d ago".to_owned(),
            branch: "main".to_owned(),
            context: cwd.rsplit('/').next().unwrap_or(cwd).to_owned(),
            cwd: Some(cwd.to_owned()),
            provider: Some(provider.to_owned()),
            model: Some("gpt-5-codex".to_owned()),
            preview: Some(format!("{title} preview text")),
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
        for (index, line) in lines.iter().enumerate() {
            if line.contains('❯') || line.contains("Start new session") {
                let y = 36 + index * 18;
                svg.push_str(&format!(
                    "<rect x=\"8\" y=\"{}\" width=\"{}\" height=\"18\" fill=\"#2d333b\"/>",
                    y.saturating_sub(14),
                    pixel_width.saturating_sub(16)
                ));
            }
        }
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

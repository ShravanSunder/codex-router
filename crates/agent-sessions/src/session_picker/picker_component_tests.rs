//! Shared mock-terminal fixtures; actual behavior cases live in focused modules.

use crossterm::event::MouseButton;
use futures_util::StreamExt;
use iocraft::prelude::*;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use super::*;
use crate::picker_runtime_status::PickerRuntimeStatus;
use crate::presentation::session_picker::picker_request::SessionsPickerRoot as SessionsRoot;
use crate::presentation::session_picker::test_support::picker_record;
use crate::presentation::session_picker::test_support::picker_request;
use crate::sessions::SessionConversationPreview;
use crate::sessions::SessionPickerRecord;
use crate::sessions::SessionsProvider;
use crate::sessions::SessionsSort;
use crate::sessions::SessionsSource;

fn reload_query(search: &str) -> SessionsPickerDataQuery {
    SessionsPickerDataQuery {
        root: SessionsRoot::Any,
        provider: SessionsProvider::Any,
        source: SessionsSource::All,
        sort: SessionsSort::Updated,
        search: search.to_owned(),
    }
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

fn alt_enter_key() -> TerminalEvent {
    let mut event = KeyEvent::new(KeyEventKind::Press, KeyCode::Enter);
    event.modifiers = KeyModifiers::ALT;
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
        explicit_name: None,
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
        runtime_status: PickerRuntimeStatus::Unknown,
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

#[path = "picker_interaction_tests.rs"]
mod picker_interaction_tests;

#[path = "picker_layout_tests.rs"]
mod picker_layout_tests;

#[path = "picker_background_tests.rs"]
mod picker_background_tests;

#[path = "picker_capture_tests.rs"]
mod picker_capture_tests;

//! Actual renderer/event proof for machine choices and modal input ownership.
use super::*;
use crate::sessions::RouterRegistryRead;
use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};
use futures_util::StreamExt;
use std::time::Duration;

fn control_key(character: char) -> TerminalEvent {
    let mut event = KeyEvent::new(KeyEventKind::Press, KeyCode::Char(character));
    event.modifiers = KeyModifiers::CONTROL;
    TerminalEvent::Key(event)
}

fn key(code: KeyCode) -> TerminalEvent {
    TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, code))
}

fn configured_request() -> Result<SessionsPickerRequest, crate::sessions::RouterRegistryError> {
    let mut request = picker_request();
    request.router_registry = RouterRegistryRead::Ready(
        crate::sessions::router_connection_registry::RouterConnectionRegistry::parse(
            r#"{
            "version":1,"routers":[{"name":"Sunbook",
            "connection":{"kind":"remote","serviceId":"00000000-0000-4000-8000-000000000001",
                "mcpUrl":"https://machine.example.invalid:443/mcp",
                "nativeCodex":{"address":"wss://machine.example.invalid:443/"}},
            "defaultRemoteCwd":"/remote/only/project"}]
        }"#,
        )?,
    );
    Ok(request)
}

async fn render_events(
    request: SessionsPickerRequest,
    events: Vec<TerminalEvent>,
) -> Result<(Vec<String>, Option<SessionsPickerOutcome>), tokio::time::error::Elapsed> {
    let mut outcome = Option::<SessionsPickerOutcome>::None;
    let (send_event, receive_event) = tokio::sync::mpsc::unbounded_channel();
    let mut remaining = events.into_iter();
    if let Some(event) = remaining.next() {
        let _ = send_event.send(event);
    }
    let input = futures_util::stream::unfold(receive_event, |mut receiver| async {
        receiver.recv().await.map(|event| (event, receiver))
    });
    let frames = tokio::time::timeout(Duration::from_secs(2), async {
        let mut picker = element! {
            SessionsPickerComponent(request, width: 100usize, height: 40usize, selected_outcome_out: &mut outcome)
        };
        let canvases = picker.mock_terminal_render_loop(MockTerminalConfig::with_events(input));
        tokio::pin!(canvases);
        let mut frames = Vec::new();
        while let Some(canvas) = canvases.next().await {
            frames.push(canvas.to_string());
            if let Some(event) = remaining.next() {
                let _ = send_event.send(event);
            }
        }
        frames
    }).await?;
    Ok((frames, outcome))
}

#[tokio::test]
async fn machine_selector_does_not_let_new_fork_search_or_reload_act_through_it() {
    let mut alt_enter = KeyEvent::new(KeyEventKind::Press, KeyCode::Enter);
    alt_enter.modifiers = KeyModifiers::ALT;
    let mut outcome = Option::<SessionsPickerOutcome>::None;
    let (send_event, receive_event) = tokio::sync::mpsc::unbounded_channel();
    send_event.send(control_key('g')).unwrap();
    let input = futures_util::stream::unfold(receive_event, |mut receiver| async {
        receiver.recv().await.map(|event| (event, receiver))
    });
    let frames = tokio::time::timeout(Duration::from_secs(2), async {
        let mut picker = element! {
            SessionsPickerComponent(request: picker_request(), width: 100usize, height: 40usize,
                selected_outcome_out: &mut outcome)
        };
        let canvases = picker.mock_terminal_render_loop(MockTerminalConfig::with_events(input));
        tokio::pin!(canvases);
        let mut opened = false;
        let mut frames = Vec::new();
        while let Some(canvas) = canvases.next().await {
            let text = canvas.to_string();
            if !opened && text.contains("Choose machine") {
                opened = true;
                // Inert input produces no render acknowledgement; send it before the observed back/exit.
                for event in [
                    control_key('n'),
                    TerminalEvent::Key(alt_enter.clone()),
                    key(KeyCode::Char('x')),
                    control_key('r'),
                    key(KeyCode::Esc),
                    control_key('c'),
                ] {
                    send_event.send(event).unwrap();
                }
            }
            frames.push(text);
        }
        frames
    })
    .await
    .unwrap();
    assert_eq!(
        outcome, None,
        "without a registry, any leaked Ctrl+N would create a default action"
    );
    assert!(frames.iter().any(|frame| frame.contains("Choose machine")));
    assert!(frames.iter().all(|frame| !frame.contains("Search: [x]")));
}

#[tokio::test]
async fn selecting_all_reports_unqualified_sources_instead_of_empty_success() {
    let (frames, outcome) = render_events(
        configured_request().unwrap(),
        vec![
            key(KeyCode::F(2)),
            key(KeyCode::Down),
            key(KeyCode::Enter),
            control_key('c'),
        ],
    )
    .await
    .unwrap();
    assert_eq!(outcome, None);
    assert!(
        frames
            .iter()
            .any(|frame| frame.contains("Machine: All machines"))
    );
    assert!(
        frames
            .iter()
            .any(|frame| frame.contains("Partial view") && frame.contains("Sunbook"))
    );
}

#[tokio::test]
async fn unavailable_machine_keeps_the_previous_view_and_cannot_fall_back_to_new() {
    let (frames, outcome) = render_events(
        configured_request().unwrap(),
        vec![
            control_key('g'),
            key(KeyCode::Down),
            key(KeyCode::Down),
            key(KeyCode::Enter),
            key(KeyCode::Esc),
            control_key('c'),
        ],
    )
    .await
    .unwrap();
    assert_eq!(outcome, None);
    assert!(
        frames
            .iter()
            .any(|frame| frame.contains("previous view retained"))
    );
    assert!(
        frames
            .iter()
            .rev()
            .any(|frame| frame.contains("Machine: This machine"))
    );
}

#[tokio::test]
async fn named_new_uses_configured_remote_cwd_and_cannot_create_before_route_qualification() {
    let (frames, outcome) = render_events(
        configured_request().unwrap(),
        vec![
            control_key('n'),
            key(KeyCode::Down),
            key(KeyCode::Enter),
            key(KeyCode::Esc),
            control_key('c'),
        ],
    )
    .await
    .unwrap();
    assert_eq!(outcome, None);
    assert!(
        frames
            .iter()
            .any(|frame| frame.contains("Choose machine for new session"))
    );
    assert!(
        frames
            .iter()
            .any(|frame| frame.contains("/remote/only/project"))
    );
    assert!(
        frames
            .iter()
            .any(|frame| frame.contains("no session created"))
    );
}

#[tokio::test]
async fn tab_and_enter_open_machine_control_while_plain_enter_still_resumes() {
    let (frames, outcome) = render_events(
        picker_request(),
        vec![
            key(KeyCode::Tab),
            key(KeyCode::Enter),
            key(KeyCode::Esc),
            control_key('c'),
        ],
    )
    .await
    .unwrap();
    assert_eq!(outcome, None);
    assert!(frames.iter().any(|frame| frame.contains("Choose machine")));
    let (_, outcome) = render_events(picker_request(), vec![key(KeyCode::Enter)])
        .await
        .unwrap();
    assert!(matches!(
        outcome,
        Some(SessionsPickerOutcome::ResumeSession(_))
    ));
}

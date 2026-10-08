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

pub(super) fn configured_request()
-> Result<SessionsPickerRequest, crate::sessions::RouterRegistryError> {
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

#[tokio::test]
async fn individual_machine_switch_reads_and_publishes_only_its_bound_source() {
    use crate::presentation::session_picker::{PickerSourceContext, SourceInventoryResult};
    let mut request = configured_request().unwrap();
    request.root = SessionsPickerRoot::Any;
    let endpoint: collaboration_client::protocol::EndpointRef = serde_json::from_value(
        serde_json::json!({"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"source-native"}),
    ).unwrap();
    let expected_endpoint = endpoint.clone();
    let loader: SessionsPickerRecordLoader = Arc::new(move |request| {
        let endpoint = endpoint.clone();
        Box::pin(async move {
            if !matches!(
                request.source_context,
                PickerSourceContext::ConfiguredHosted(_)
            ) {
                futures_util::future::pending::<()>().await;
            }
            let mut row = picker_record(
                "source-only-id",
                "Source-only result",
                "/source/only",
                "codex-router",
                "cli",
            )
            .with_hosted_codex(&endpoint);
            row.provenance = crate::sessions::SessionRowProvenance::ObservedHosted;
            row.normalized_cwd = None;
            SourceInventoryResult::Ready {
                request,
                bound_endpoint: Some(endpoint),
                snapshot: observed_records(vec![row]),
            }
        })
    });
    let mut outcome = None;
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    sender.send(key(KeyCode::F(2))).unwrap();
    let input = futures_util::stream::unfold(receiver, |mut receiver| async {
        receiver.recv().await.map(|event| (event, receiver))
    });
    let frames = tokio::time::timeout(Duration::from_secs(2), async {
        let mut picker = element! { SessionsPickerComponent(request, record_loader: Some(loader), width: 130usize, height: 40usize, selected_outcome_out: &mut outcome) };
        let canvases = picker.mock_terminal_render_loop(MockTerminalConfig::with_events(input));
        tokio::pin!(canvases);
        let mut selected = false;
        let mut activated = false;
        let mut frames = Vec::new();
        while let Some(canvas) = canvases.next().await {
            let text = canvas.to_string();
            if !selected && text.contains("Choose machine") {
                selected = true;
                sender.send(key(KeyCode::Down)).unwrap();
                sender.send(key(KeyCode::Down)).unwrap();
                sender.send(key(KeyCode::Enter)).unwrap();
            }
            if !activated && text.contains("Machine: Sunbook") && text.contains("Source-only result") {
                activated = true;
                sender.send(key(KeyCode::Home)).unwrap();
                sender.send(key(KeyCode::Down)).unwrap();
                sender.send(key(KeyCode::Enter)).unwrap();
            }
            frames.push(text);
        }
        frames
    }).await.expect("individual source must publish through the existing loader before activation");
    assert!(frames.iter().any(|frame| frame.contains("Machine: Sunbook") && frame.contains("Source-only result")));
    let Some(SessionsPickerOutcome::ResumeSession(selection)) = outcome else {
        panic!("source row must resume");
    };
    assert_eq!(selection.session_id(), "source-only-id");
    assert_eq!(
        selection.identity,
        crate::sessions::SessionPickerIdentity::HostedCodex(
            collaboration_client::protocol::SessionRef {
                endpoint: expected_endpoint,
                session_id: "source-only-id".to_owned().try_into().unwrap()
            }
        )
    );
}

#[tokio::test]
async fn all_view_reports_each_loading_and_rejected_source_without_empty_success() {
    let loader: SessionsPickerRecordLoader = Arc::new(|request| {
        Box::pin(async move {
            if matches!(
                request.source_context,
                crate::presentation::session_picker::PickerSourceContext::DefaultHosted
            ) {
                futures_util::future::pending::<()>().await;
            }
            crate::presentation::session_picker::SourceInventoryResult::Rejected {
            request, reason: crate::presentation::session_picker::SourceInventoryRejection::EndpointUnqualified,
        }
        })
    });
    let mut outcome = Option::<SessionsPickerOutcome>::None;
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    sender.send(key(KeyCode::F(2))).unwrap();
    let input = futures_util::stream::unfold(receiver, |mut receiver| async {
        receiver.recv().await.map(|event| (event, receiver))
    });
    let frames = tokio::time::timeout(Duration::from_secs(2), async {
        let mut picker = element! { SessionsPickerComponent(request: configured_request().unwrap(), record_loader: Some(loader), width: 130usize, height: 40usize, selected_outcome_out: &mut outcome) };
        let canvases = picker.mock_terminal_render_loop(MockTerminalConfig::with_events(input));
        tokio::pin!(canvases);
        let mut opened = false;
        let mut frames = Vec::new();
        while let Some(canvas) = canvases.next().await {
            let text = canvas.to_string();
            if !opened && text.contains("Choose machine") {
                opened = true;
                sender.send(key(KeyCode::Down)).unwrap();
                sender.send(key(KeyCode::Enter)).unwrap();
            }
            if text.contains("This machine: Loading") && text.contains("Sunbook: Unavailable") {
                sender.send(control_key('c')).unwrap();
            }
            frames.push(text);
        }
        frames
    }).await.expect("each source must expose loading or unavailable status within bounded render wait");
    assert!(
        frames
            .iter()
            .any(|frame| frame.contains("This machine: Loading")
                && frame.contains("Sunbook: Unavailable"))
    );
    assert_eq!(outcome, None);
}

#[tokio::test]
async fn all_view_renders_qualified_alias_labels_from_simulated_bound_source_replies() {
    use crate::presentation::session_picker::{PickerSourceContext, SourceInventoryResult};
    let mut request = configured_request().unwrap();
    request.root = SessionsPickerRoot::Any;
    let RouterRegistryRead::Ready(registry) = &mut request.router_registry else {
        panic!("registry");
    };
    registry.routers[0].name = "Primary machine".to_owned().try_into().unwrap();
    let mut alias = registry.routers[0].clone();
    alias.name = "Second alias".to_owned().try_into().unwrap();
    registry.routers.push(alias);
    let loader: SessionsPickerRecordLoader = Arc::new(|request| {
        Box::pin(async move {
            let PickerSourceContext::ConfiguredHosted(profile) = &request.source_context else {
                return SourceInventoryResult::Ready {
                    bound_endpoint: None,
                    request,
                    snapshot: observed_records(vec![]),
                };
            };
            let endpoint: collaboration_client::protocol::EndpointRef = serde_json::from_value(serde_json::json!({
            "serviceId":String::from(profile.service_id.clone()),"endpointId":"source-native"
        })).unwrap();
            let records = if profile.name.as_str() == "Primary machine" {
                vec![]
            } else {
                let mut row = picker_record(
                    "shared-source-id",
                    "Bound source row",
                    "/source/project",
                    "codex-router",
                    "cli",
                )
                .with_hosted_codex(&endpoint);
                row.provenance = crate::sessions::SessionRowProvenance::ObservedHosted;
                row.normalized_cwd = None;
                vec![row]
            };
            SourceInventoryResult::Ready {
                bound_endpoint: Some(endpoint),
                request,
                snapshot: observed_records(records),
            }
        })
    });
    let mut outcome = Option::<SessionsPickerOutcome>::None;
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    sender.send(key(KeyCode::F(2))).unwrap();
    let input = futures_util::stream::unfold(receiver, |mut receiver| async {
        receiver.recv().await.map(|event| (event, receiver))
    });
    let frames = tokio::time::timeout(Duration::from_secs(2), async {
        let mut picker = element! { SessionsPickerComponent(request, record_loader: Some(loader), width: 160usize, height: 40usize, selected_outcome_out: &mut outcome) };
        let canvases = picker.mock_terminal_render_loop(MockTerminalConfig::with_events(input));
        tokio::pin!(canvases);
        let mut opened = false;
        let mut frames = Vec::new();
        while let Some(canvas) = canvases.next().await {
            let text = canvas.to_string();
            if !opened && text.contains("Choose machine") {
                opened = true;
                sender.send(key(KeyCode::Down)).unwrap();
                sender.send(key(KeyCode::Enter)).unwrap();
            }
            if text.contains("Primary machine (also: Second alias)") && text.contains("Bound source row") {
                sender.send(control_key('c')).unwrap();
            }
            frames.push(text);
        }
        frames
    }).await.expect("bound alias row must be visible within bounded render wait");
    assert!(
        frames
            .iter()
            .any(|frame| frame.contains("Primary machine (also: Second alias)"))
    );
    assert_eq!(outcome, None);
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

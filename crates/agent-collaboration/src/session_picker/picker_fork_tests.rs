//! Fork confirmation through actual renderer/input and frozen source state.
use super::*;
use crate::presentation::session_picker::PickerSourceContext;
use crate::sessions::{RouterRegistryRead, SessionActionSelection, SessionPickerIdentity};
use collaboration_client::protocol::SessionRef;
use serde_json::json;

fn key(code: KeyCode) -> TerminalEvent {
    TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, code))
}

fn named_request() -> SessionsPickerRequest {
    let mut request = picker_request();
    request.router_registry = RouterRegistryRead::Ready(
        crate::sessions::router_connection_registry::RouterConnectionRegistry::parse(
            r#"{"version":1,"routers":[{"name":"Sunbook","connection":{"kind":"remote",
            "serviceId":"00000000-0000-4000-8000-000000000001","mcpUrl":"https://machine.example.invalid/mcp"}}]}"#,
        ).unwrap(),
    );
    request
}

async fn after_popup(
    request: SessionsPickerRequest,
    events: Vec<TerminalEvent>,
) -> (Vec<String>, Option<SessionsPickerOutcome>) {
    after_popup_until(request, events, None).await
}

async fn after_popup_until(
    request: SessionsPickerRequest,
    events: Vec<TerminalEvent>,
    checkpoint: Option<(&'static str, Vec<TerminalEvent>)>,
) -> (Vec<String>, Option<SessionsPickerOutcome>) {
    let mut outcome = None;
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    sender.send(alt_enter_key()).unwrap();
    let input = futures_util::stream::unfold(receiver, |mut receiver| async {
        receiver.recv().await.map(|event| (event, receiver))
    });
    let frames = tokio::time::timeout(Duration::from_secs(2), async {
        let mut picker = element! { SessionsPickerComponent(request, width: 110usize, height: 40usize, selected_outcome_out: &mut outcome) };
        let canvases = picker.mock_terminal_render_loop(MockTerminalConfig::with_events(input));
        tokio::pin!(canvases);
        let mut pending = Some(events);
        let mut checkpoint = checkpoint;
        let mut frames = Vec::new();
        while let Some(canvas) = canvases.next().await {
            let text = canvas.to_string();
            if text.contains("Fork session") && let Some(events) = pending.take() {
                for event in events { sender.send(event).unwrap(); }
            }
            if checkpoint.as_ref().is_some_and(|(label, _)| text.contains(label))
                && let Some((_, events)) = checkpoint.take() {
                for event in events { sender.send(event).unwrap(); }
            }
            frames.push(text);
        }
        frames
    }).await.expect("confirmation events must finish within bounded renderer wait");
    (frames, outcome)
}

#[tokio::test]
async fn fork_confirmation_displays_actual_override_and_returns_frozen_selection() {
    let mut request = picker_request();
    request.native_arguments = vec!["--cd".into(), "/explicit/fork".into()];
    request.records[0].model = Some("gpt-6.1-sol".to_owned());
    request.records[0].reasoning_effort = Some("high".to_owned());
    let expected = SessionActionSelection::from_picker_record(&request.records[0]);
    let (frames, outcome) = after_popup(request, vec![key(KeyCode::Enter)]).await;
    assert!(
        frames
            .iter()
            .any(|frame| frame.contains("Working directory: /explicit/fork")
                && frame.contains("explicit native argument"))
    );
    let Some(SessionsPickerOutcome::ForkSession(selection)) = outcome else {
        panic!("confirmed native source should fork")
    };
    assert_eq!(selection.identity, expected.identity);
    assert_eq!(selection.model_choice, expected.model_choice);
    assert_eq!(
        selection.source_context,
        Some(PickerSourceContext::DefaultHosted)
    );
}

#[tokio::test]
async fn fork_popup_blocks_other_machine_and_does_not_leak_main_shortcuts() {
    let (frames, outcome) = after_popup_until(
        named_request(),
        vec![key(KeyCode::Down), key(KeyCode::Enter)],
        Some((
            "History remains on the source machine",
            vec![
                ctrl_key('n'),
                ctrl_key('r'),
                ctrl_key('g'),
                key(KeyCode::Char('x')),
                key(KeyCode::F(1)),
                key(KeyCode::Esc),
                ctrl_key('c'),
            ],
        )),
    )
    .await;
    assert_eq!(outcome, None);
    assert!(
        frames
            .iter()
            .any(|frame| frame.contains("History remains on the source machine"))
    );
    assert!(
        frames
            .iter()
            .all(|frame| !frame.contains("Search: [x]") && !frame.contains("Choose machine"))
    );
    assert!(frames.iter().any(
        |frame| frame.contains("Source: This machine") && frame.contains("invoking directory")
    ));
}

#[tokio::test]
async fn fork_escape_preserves_main_focus_and_query() {
    let mut request = picker_request();
    request.records.truncate(1);
    let expected = request.records[0].identity.clone();
    let (frames, outcome) =
        after_popup(request, vec![key(KeyCode::Esc), key(KeyCode::Enter)]).await;
    assert!(frames.iter().any(|frame| frame.contains("Fork session")));
    let Some(SessionsPickerOutcome::ResumeSession(selection)) = outcome else {
        panic!("back should restore row activation")
    };
    assert_eq!(selection.identity, expected);
}

#[test]
fn fork_source_survives_refresh_with_equal_native_id_and_changed_metadata() {
    let mut request = picker_request();
    let target: SessionRef = serde_json::from_value(json!({"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000002","endpointId":"codex-local"},"sessionId":"thread-a"})).unwrap();
    request.records[0].identity = SessionPickerIdentity::HostedCodex(target.clone());
    request.records[0].model = Some("gpt-6.1-sol".to_owned());
    let mut model = SessionsPickerModel::new(request, 110);
    model.handle_key(SessionsPickerKey::SearchChar('F'));
    let query = model.data_query();
    model.open_fork_confirmation();
    let captured = model.fork_confirmation.clone().unwrap();
    let mut replacement = picker_request().records.remove(0);
    replacement.model = Some("different-model".to_owned());
    model.replace_records(observed_records(vec![replacement]));
    let confirmation = model.fork_confirmation.as_mut().unwrap();
    assert_eq!(confirmation, &captured);
    let Some(SessionsPickerOutcome::ForkSession(selection)) = confirmation.confirm() else {
        panic!("captured default source remains eligible")
    };
    assert_eq!(
        selection.identity,
        SessionPickerIdentity::HostedCodex(target)
    );
    assert_eq!(selection.model_choice, captured.selection.model_choice);
    assert_eq!(model.data_query(), query);
}

#[tokio::test]
async fn unqualified_source_fork_displays_source_directory_and_never_falls_back() {
    let mut request = named_request();
    let RouterRegistryRead::Ready(registry) = &request.router_registry else {
        panic!("named fixture must contain a validated registry")
    };
    let profile = registry.routers[0].clone();
    request.records[0].source_context = Some(PickerSourceContext::ConfiguredHosted(profile));
    request.records[0].cwd = Some("/source/fork-directory".to_owned());
    let (frames, outcome) = after_popup(request, vec![key(KeyCode::Enter), ctrl_key('d')]).await;
    assert_eq!(outcome, None);
    assert!(frames.iter().any(|frame| frame.contains("Source: Sunbook")
        && frame.contains("/source/fork-directory")
        && frame.contains("source session")));
    assert!(
        frames
            .iter()
            .any(|frame| frame.contains("needs qualification"))
    );
}

#[tokio::test]
async fn provider_fork_opens_an_explanation_without_a_native_action() {
    let summary = serde_json::from_value(json!({
        "origin":"hostedProvider",
        "target":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"claude-local"},"sessionId":"provider-one"},
        "workingDirectory":"/repo/project-a","updatedAt":3,"state":"requiresAction",
        "approver":{"kind":"human","humanId":"owner"},
        "createdBy":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"sessionId":"creator"}
    })).unwrap();
    let mut request = picker_request();
    request.records = vec![SessionPickerRecord::from_provider_summary(
        &summary,
        "Claude fixture",
    )];
    let (frames, outcome) = after_popup(request, vec![key(KeyCode::Enter), ctrl_key('c')]).await;
    assert_eq!(outcome, None);
    assert!(frames.iter().any(|frame| frame.contains("Fork session")
        && frame.contains("Provider fork is unavailable in this launcher")));
}

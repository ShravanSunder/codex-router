//! Owner-selected browse navigation, observed through the actual iocraft component.
use super::*;

fn details_key(code: KeyCode, modifiers: KeyModifiers) -> TerminalEvent {
    let mut event = KeyEvent::new(KeyEventKind::Press, code);
    event.modifiers = modifiers;
    TerminalEvent::Key(event)
}

#[tokio::test]
async fn owner_details_navigation_title_exposes_details_without_standalone_footer() {
    for width in [120, 76, 48] {
        let frame =
            render_picker_capture_at(picker_request(), width, 30, vec![ctrl_key('d')]).await;
        let header = frame
            .lines()
            .find(|line| line.contains("Sessions ·"))
            .unwrap();
        assert!(
            header.contains("Details") && header.contains("Help"),
            "both actions share title row: {frame}"
        );
        assert_eq!(
            frame
                .lines()
                .filter(|line| line.contains("Details"))
                .count(),
            1,
            "no separate Details footer: {frame}"
        );
    }
}

async fn observe_details_transition(
    event: TerminalEvent,
    expected_surface: &'static str,
) -> (Option<SessionsPickerOutcome>, Vec<String>) {
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    sender.send(event).unwrap();
    let events = futures_util::stream::unfold(receiver, |mut receiver| async move {
        receiver.recv().await.map(|event| (event, receiver))
    });
    let mut outcome: Option<SessionsPickerOutcome> = None;
    let mut exit_sent = false;
    let frames = tokio::time::timeout(Duration::from_secs(2),
        element!(SessionsPickerComponent(request: picker_request(), width: 76usize, height: 30usize, selected_outcome_out: &mut outcome))
            .mock_terminal_render_loop(MockTerminalConfig::with_events(events))
            .map(|canvas| {
                let frame = canvas.to_string();
                // Cancellation cannot erase a wrongly published launch: exit is
                // sent only after the intended non-launching surface is observed.
                if !exit_sent && frame.contains(expected_surface) {
                    sender.send(ctrl_key('d')).unwrap();
                    exit_sent = true;
                }
                frame
            }).collect::<Vec<_>>()
    ).await.expect("input must reach the requested surface or return an outcome");
    (outcome, frames)
}

#[tokio::test]
async fn owner_details_navigation_enter_opens_focused_session_without_launching() {
    let (outcome, frames) = observe_details_transition(
        details_key(KeyCode::Enter, KeyModifiers::empty()),
        "Session details",
    )
    .await;
    assert!(
        outcome.is_none(),
        "ordinary Enter inspects; it cannot resume or create: {outcome:?}"
    );
    assert!(
        frames.iter().any(
            |frame| frame.contains("Session details") && frame.contains("Session ID: thread-a")
        ),
        "focused session details must open: {frames:?}"
    );
}

#[tokio::test]
async fn owner_details_navigation_control_shift_enter_opens_fork_confirmation() {
    let (outcome, frames) = observe_details_transition(
        details_key(KeyCode::Enter, KeyModifiers::CONTROL | KeyModifiers::SHIFT),
        "Fork session",
    )
    .await;
    assert!(
        outcome.is_none(),
        "fork must retain its confirmation: {outcome:?}"
    );
    assert!(
        frames.iter().any(|frame| frame.contains("Fork session")),
        "fork confirmation must open: {frames:?}"
    );
}

#[tokio::test]
async fn owner_details_navigation_dialog_exposes_resume_fork_and_copy_session_id() {
    let frame = render_picker_capture_at(
        picker_request(),
        120,
        36,
        vec![
            details_key(KeyCode::F(4), KeyModifiers::empty()),
            ctrl_key('d'),
        ],
    )
    .await;
    assert!(
        frame.contains("Resume") && frame.contains("Fork") && frame.contains("Copy session ID"),
        "session actions must live in Details: {frame}"
    );
}

#[tokio::test]
async fn owner_details_navigation_control_enter_resumes_without_opening_details() {
    let (outcome, frames) = observe_details_transition(
        details_key(KeyCode::Enter, KeyModifiers::CONTROL),
        "unused-surface-marker",
    )
    .await;
    let Some(SessionsPickerOutcome::ResumeSession(selection)) = outcome else {
        panic!("Ctrl+Enter must resume")
    };
    assert_eq!(selection.session_id(), "thread-a");
    assert!(
        frames
            .iter()
            .all(|frame| !frame.contains("Session details"))
    );
}

#[tokio::test]
async fn owner_details_navigation_copy_action_sends_exact_id_once_without_launching() {
    let copied = Arc::new(Mutex::new(Vec::<String>::new()));
    let copier: crate::presentation::session_picker::session_id_clipboard::SessionIdCopier =
        Arc::new({
            let copied = copied.clone();
            move |session_id| {
                copied.lock().unwrap().push(session_id.to_owned());
                Ok(())
            }
        });
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    sender
        .send(details_key(KeyCode::Enter, KeyModifiers::empty()))
        .unwrap();
    let events = futures_util::stream::unfold(receiver, |mut receiver| async move {
        receiver.recv().await.map(|event| (event, receiver))
    });
    let mut outcome: Option<SessionsPickerOutcome> = None;
    let mut phase = 0;
    let frames = tokio::time::timeout(Duration::from_secs(2),
        element!(SessionsPickerComponent(request: picker_request(), width: 48usize, height: 24usize,
            session_id_copier: Some(copier), selected_outcome_out: &mut outcome))
            .mock_terminal_render_loop(MockTerminalConfig::with_events(events))
            .map(|canvas| {
                let frame = canvas.to_string();
                if phase == 0 && frame.contains("Session details") {
                    for code in [KeyCode::Tab, KeyCode::Tab, KeyCode::Enter] { sender.send(details_key(code, KeyModifiers::empty())).unwrap(); }
                    phase = 1;
                } else if phase == 1 && frame.contains("Copy requested") {
                    sender.send(ctrl_key('d')).unwrap();
                    phase = 2;
                }
                frame
            }).collect::<Vec<_>>()
    ).await.expect("copy action must settle without launching");
    assert_eq!(phase, 2, "copy feedback must be observed: {frames:?}");
    assert!(outcome.is_none());
    assert_eq!(copied.lock().unwrap().as_slice(), ["thread-a"]);
}

#[test]
fn owner_details_navigation_actions_keep_the_captured_record_after_background_focus_changes() {
    use crate::presentation::session_picker::details_dialog_actions::apply_details_action;
    use crate::presentation::session_picker::details_dialog_state::DetailsAction;
    let mut request = picker_request();
    request.root = SessionsRoot::Any;
    request.source = SessionsSource::All;
    for action in [
        DetailsAction::Resume,
        DetailsAction::Fork,
        DetailsAction::CopySessionId,
    ] {
        let mut model = SessionsPickerModel::new(request.clone(), 120);
        model.open_details_dialog();
        model.handle_key(SessionsPickerKey::MoveDown);
        assert_eq!(model.focused_record().unwrap().session_id, "thread-b");
        let copied = Arc::new(Mutex::new(Vec::<String>::new()));
        let copier: crate::presentation::session_picker::session_id_clipboard::SessionIdCopier =
            Arc::new({
                let copied = copied.clone();
                move |session_id| {
                    copied.lock().unwrap().push(session_id.to_owned());
                    Ok(())
                }
            });
        let outcome = apply_details_action(&mut model, action, &copier);
        match action {
            DetailsAction::Resume => assert!(
                matches!(outcome, Some(SessionsPickerOutcome::ResumeSession(selection)) if selection.session_id() == "thread-a")
            ),
            DetailsAction::Fork => {
                assert!(outcome.is_none());
                assert_eq!(
                    model
                        .fork_confirmation
                        .as_ref()
                        .unwrap()
                        .selection
                        .session_id(),
                    "thread-a"
                );
            }
            DetailsAction::CopySessionId => {
                assert!(outcome.is_none());
                assert_eq!(copied.lock().unwrap().as_slice(), ["thread-a"]);
            }
            other => panic!("unexpected Details action in test case: {other:?}"),
        }
    }
}

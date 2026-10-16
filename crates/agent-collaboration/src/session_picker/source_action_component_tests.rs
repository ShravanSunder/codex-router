//! Real component input around an explicitly held read-only preparation boundary.
use super::*;
use crate::presentation::session_picker::{
    source_action_state::PendingSourceAction,
    source_action_tests::{prepared, source_model},
    source_action_worker::SourceActionPreparer,
};
use crate::sessions::source_ssh_protocol::{SourceReadError, SourceSessionAction};

fn key(code: KeyCode) -> TerminalEvent {
    TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, code))
}

#[tokio::test]
async fn source_action_component_checking_blocks_inputs_resizes_and_esc_discards_late_result() {
    let (finish, receive) = tokio::sync::oneshot::channel();
    let receive = Arc::new(Mutex::new(Some(receive)));
    let calls = Arc::new(AtomicUsize::new(0));
    let preparer: SourceActionPreparer = {
        let calls = calls.clone();
        Arc::new(move |_, _| {
            calls.fetch_add(1, Ordering::SeqCst);
            let receive = receive
                .lock()
                .unwrap()
                .take()
                .expect("only one preparation");
            Box::pin(async move { receive.await.unwrap() })
        })
    };
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    let events = futures_util::stream::unfold(receiver, |mut receiver| async {
        receiver.recv().await.map(|event| (event, receiver))
    });
    let mut outcome = Option::<SessionsPickerOutcome>::None;
    tokio::time::timeout(Duration::from_secs(2), async {
        let mut picker = element! { SessionsPickerComponent(request: source_model().request, source_preparer: Some(preparer), width: 0usize, height: 0usize, selected_outcome_out: &mut outcome) };
        let frames = picker.mock_terminal_render_loop(MockTerminalConfig::with_events(events));
        tokio::pin!(frames);
        let mut phase = 0;
        let mut finish = Some(finish);
        while let Some(canvas) = frames.next().await {
            let text = canvas.to_string();
            if phase == 0 {
                for event in [TerminalEvent::Resize(120,36), ctrl_key('n'), key(KeyCode::Down), key(KeyCode::Enter)] { sender.send(event).unwrap(); }
                phase = 1;
            } else if phase == 1 && text.contains("Checking source") {
                for event in [key(KeyCode::Enter), ctrl_key('n'), ctrl_key('f'), key(KeyCode::F(1)), key(KeyCode::Down), TerminalEvent::BracketedPaste("do-not-search".to_owned()), TerminalEvent::FullscreenMouse(FullscreenMouseEvent::new(MouseEventKind::Down(MouseButton::Left), 5, 1)), TerminalEvent::Resize(100,30)] { sender.send(event).unwrap(); }
                phase = 2;
            } else if phase == 2 && text.contains("Checking source") && text.lines().count() == 30 {
                assert!(!text.contains("Sessions Help"));
                assert!(!text.contains("do-not-search"));
                sender.send(key(KeyCode::Esc)).unwrap();
                phase = 3;
            } else if phase == 3 && !text.contains("Checking source") && text.contains("Choose machine for new session") {
                let _ = finish.take().unwrap().send(Err(SourceReadError::ReadUnavailable));
                sender.send(ctrl_key('d')).unwrap();
                phase = 4;
            }
        }
        assert_eq!(phase, 4);
    }).await.expect("event-controlled Checking cancellation must finish");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(outcome, None);
}

#[tokio::test]
async fn source_action_component_current_prepared_new_returns_exact_captured_outcome() {
    let preparer: SourceActionPreparer = Arc::new(|profile, action| {
        Box::pin(async move {
            assert!(
                matches!(action, SourceSessionAction::New { ref directory } if directory.as_str() == "/source/only")
            );
            let request = PendingSourceAction {
                attempt: 1,
                profile,
                action,
                source_directory: Some("/source/only".to_owned()),
            };
            Ok(prepared(&request))
        })
    });
    let mut outcome = Option::<SessionsPickerOutcome>::None;
    let events =
        futures_util::stream::iter(vec![ctrl_key('n'), key(KeyCode::Down), key(KeyCode::Enter)])
            .chain(futures_util::stream::pending());
    tokio::time::timeout(Duration::from_secs(2), async {
        let mut picker = element! { SessionsPickerComponent(request: source_model().request, source_preparer: Some(preparer), width: 120usize, height: 36usize, selected_outcome_out: &mut outcome) };
        picker.mock_terminal_render_loop(MockTerminalConfig::with_events(events)).collect::<Vec<_>>().await;
    }).await.expect("prepared action must finish without another confirmation");
    let Some(SessionsPickerOutcome::PreparedSourceSession(prepared)) = outcome else {
        panic!("expected prepared configured source");
    };
    assert_eq!(prepared.profile.name.as_str(), "Source fixture");
    assert_eq!(prepared.launch.working_directory.as_str(), "/source/only");
}

#[tokio::test]
async fn source_action_component_whole_exit_invalidates_held_preparation() {
    let preparer: SourceActionPreparer = Arc::new(|_, _| Box::pin(std::future::pending()));
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    let events = futures_util::stream::unfold(receiver, |mut receiver| async {
        receiver.recv().await.map(|event| (event, receiver))
    });
    let mut outcome = Option::<SessionsPickerOutcome>::None;
    tokio::time::timeout(Duration::from_secs(2), async {
        let mut picker = element! { SessionsPickerComponent(request: source_model().request, source_preparer: Some(preparer), width: 120usize, height: 36usize, selected_outcome_out: &mut outcome) };
        let frames = picker.mock_terminal_render_loop(MockTerminalConfig::with_events(events));
        tokio::pin!(frames);
        let mut started = false;
        let mut checking = false;
        while let Some(canvas) = frames.next().await {
            let text = canvas.to_string();
            if !started {
                for event in [ctrl_key('n'), key(KeyCode::Down), key(KeyCode::Enter)] { sender.send(event).unwrap(); }
                started = true;
            } else if !checking && text.contains("Checking source") {
                sender.send(ctrl_key('d')).unwrap();
                checking = true;
            }
        }
        assert!(checking, "whole exit scenario must reach Checking first");
    }).await.expect("whole exit must finish while preparation is held");
    assert_eq!(outcome, None);
}

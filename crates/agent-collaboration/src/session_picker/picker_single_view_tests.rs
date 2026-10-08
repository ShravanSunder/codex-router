//! Event-controlled source switching through the real renderer and reload worker.
use super::*;
use crate::presentation::session_picker::{
    PickerSourceContext, SourceInventoryRejection, SourceInventoryResult,
};
use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};
use futures_util::StreamExt;
use std::{
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

fn key(code: KeyCode, modifiers: KeyModifiers) -> TerminalEvent {
    let mut event = KeyEvent::new(KeyEventKind::Press, code);
    event.modifiers = modifiers;
    TerminalEvent::Key(event)
}

#[derive(Clone, Copy)]
enum SwitchResult {
    Ready,
    Rejected,
    Canceled,
}

async fn assert_held_switch(result: SwitchResult) {
    let mut request = super::picker_machine_control_tests::configured_request().unwrap();
    request.root = SessionsPickerRoot::Any;
    request.records = vec![picker_record(
        "retained-id",
        "Retained original row",
        "/original",
        "codex-router",
        "cli",
    )];
    let (completion, receiver) = tokio::sync::oneshot::channel();
    let completion = Arc::new(Mutex::new(Some(completion)));
    let receiver = Arc::new(Mutex::new(Some(receiver)));
    let source_reads = Arc::new(AtomicUsize::new(0));
    let loader: SessionsPickerRecordLoader = Arc::new({
        let source_reads = source_reads.clone();
        move |request| {
            let receiver = receiver.clone();
            let source_reads = source_reads.clone();
            Box::pin(async move {
                if !matches!(
                    request.source_context,
                    PickerSourceContext::ConfiguredHosted(_)
                ) {
                    futures_util::future::pending::<()>().await;
                }
                source_reads.fetch_add(1, Ordering::SeqCst);
                let receiver = receiver
                    .lock()
                    .unwrap()
                    .take()
                    .expect("exactly one source read");
                let ready: bool = receiver.await.expect("held source completion");
                if !ready {
                    return SourceInventoryResult::Rejected {
                        request,
                        reason: SourceInventoryRejection::InvalidInventory,
                    };
                }
                let endpoint: collaboration_client::protocol::EndpointRef = serde_json::from_value(
                    serde_json::json!({"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"source-native"}),
                ).unwrap();
                let mut row = picker_record(
                    "source-row",
                    "Fresh selected row",
                    "/source",
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
        }
    });
    let mut outcome: Option<SessionsPickerOutcome> = None;
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    sender.send(key(KeyCode::F(2), KeyModifiers::NONE)).unwrap();
    let input = futures_util::stream::unfold(receiver, |mut receiver| async {
        receiver.recv().await.map(|event| (event, receiver))
    });
    let frames = tokio::time::timeout(Duration::from_secs(2), async {
        let mut picker = element! { SessionsPickerComponent(request, record_loader: Some(loader), width: 0usize, height: 40usize, selected_outcome_out: &mut outcome) };
        let canvases = picker.mock_terminal_render_loop(MockTerminalConfig::with_events(input));
        tokio::pin!(canvases);
        let mut selected = false;
        let mut blocked_input_sent = false;
        let mut settled = false;
        let mut exiting = false;
        let mut frames = Vec::new();
        while let Some(canvas) = canvases.next().await {
            let text = canvas.to_string();
            if !selected && text.contains("Choose machine") {
                selected = true;
                for code in [KeyCode::Down, KeyCode::Down, KeyCode::Enter] {
                    sender.send(key(code, KeyModifiers::NONE)).unwrap();
                }
            }
            if !settled && blocked_input_sent && text.contains("Loading machine: Sunbook")
                && text.lines().next().is_some_and(|line| line.chars().count() == 140) {
                settled = true;
                match result {
                    SwitchResult::Ready => { completion.lock().unwrap().take().unwrap().send(true).unwrap(); }
                    SwitchResult::Rejected => { completion.lock().unwrap().take().unwrap().send(false).unwrap(); }
                    SwitchResult::Canceled => { sender.send(key(KeyCode::Esc, KeyModifiers::NONE)).unwrap(); }
                }
            } else if !blocked_input_sent && text.contains("Loading machine: Sunbook") {
                blocked_input_sent = true;
                for event in [
                    key(KeyCode::Enter, KeyModifiers::NONE),
                    key(KeyCode::Enter, KeyModifiers::ALT),
                    key(KeyCode::Char('n'), KeyModifiers::CONTROL),
                    key(KeyCode::Char('x'), KeyModifiers::NONE),
                    key(KeyCode::Char('r'), KeyModifiers::CONTROL),
                ] { sender.send(event).unwrap(); }
                // Inert keys do not generate changed canvases. A distinct resize frame
                // acknowledges their FIFO consumption while the source reply stays held.
                sender.send(TerminalEvent::Resize(140, 40)).unwrap();
            }
            if settled && !exiting {
                match result {
                    SwitchResult::Ready if text.contains("Machine: Sunbook") && text.contains("Fresh selected row") => {
                        exiting = true;
                        sender.send(key(KeyCode::Char('n'), KeyModifiers::CONTROL)).unwrap();
                    }
                    SwitchResult::Rejected if text.contains("inventory does not match its source; previous view retained") => {
                        exiting = true;
                        sender.send(key(KeyCode::Esc, KeyModifiers::NONE)).unwrap();
                    }
                    SwitchResult::Canceled if text.contains("Machine: This machine") && text.contains("Retained original row") => {
                        exiting = true;
                        sender.send(key(KeyCode::Char('c'), KeyModifiers::CONTROL)).unwrap();
                    }
                    _ => {}
                }
            } else if exiting {
                match result {
                    SwitchResult::Ready if text.contains("Choose machine for new session") => {
                        assert!(text.contains("❯ Sunbook"), "NEW must preselect the committed alias");
                        sender.send(key(KeyCode::Char('c'), KeyModifiers::CONTROL)).unwrap();
                    }
                    SwitchResult::Rejected if text.contains("Machine: This machine") && text.contains("Retained original row") => {
                        sender.send(key(KeyCode::Char('c'), KeyModifiers::CONTROL)).unwrap();
                    }
                    _ => {}
                }
            }
            frames.push(text);
        }
        frames
    }).await.expect("held source must settle or cancel through bounded real render events");
    assert_eq!(outcome, None, "modal input cannot create, resume or fork");
    assert_eq!(
        source_reads.load(Ordering::SeqCst),
        1,
        "repeated Enter cannot start duplicate reads"
    );
    assert!(
        frames
            .iter()
            .any(|frame| frame.contains("Loading machine: Sunbook"))
    );
    assert!(frames.iter().all(|frame| !frame.contains("Search: [x]")));
    match result {
        SwitchResult::Ready => {
            assert!(frames.iter().any(|frame| frame.contains("Machine: Sunbook")
                && !frame.contains("Retained original row")))
        }
        SwitchResult::Rejected | SwitchResult::Canceled => {
            assert!(
                frames
                    .iter()
                    .any(|frame| frame.contains("Machine: This machine")
                        && frame.contains("Retained original row"))
            );
            assert!(
                frames
                    .iter()
                    .all(|frame| !frame.contains("Fresh selected row"))
            );
        }
    }
    if let SwitchResult::Canceled = result {
        assert!(
            completion
                .lock()
                .unwrap()
                .take()
                .unwrap()
                .send(true)
                .is_err(),
            "cancel drops owned pending source future"
        );
    }
}

#[tokio::test]
async fn single_view_loading_blocks_actions_and_preselects_new_after_commit() {
    assert_held_switch(SwitchResult::Ready).await;
}

#[tokio::test]
async fn single_view_failure_retains_the_original_filter_and_rows() {
    assert_held_switch(SwitchResult::Rejected).await;
}

#[tokio::test]
async fn single_view_cancel_drops_the_pending_read_and_retains_original_rows() {
    assert_held_switch(SwitchResult::Canceled).await;
}

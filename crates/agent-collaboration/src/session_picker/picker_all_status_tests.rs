//! Actual renderer/controller/configured dispatcher status proof; default catalog is a stand-in.
use super::*;
use crate::presentation::session_picker::{
    InventoryPageProgress, PickerSourceContext, SourceInventoryResult,
};
use crate::sessions::configured_inventory_fault_tests::{ReadPeer, ReplyScenario};
use crate::sessions::configured_inventory_read_tests::SourceApiFixture;

async fn capture_all_status(
    source: PickerSourceContext,
    actual: SessionsPickerRecordLoader,
    marker: &str,
    held: Option<&ReadPeer>,
) -> String {
    let PickerSourceContext::ConfiguredHosted(mut profile) = source else {
        panic!("configured fixture")
    };
    profile.name = "Observed source".to_owned().try_into().unwrap();
    let mut request = picker_request();
    request.root = SessionsPickerRoot::Any;
    request.source = SessionsSource::All;
    request.records.clear();
    request.router_registry = RouterRegistryRead::Ready(
        crate::sessions::router_connection_registry::RouterConnectionRegistry {
            routers: vec![profile],
        },
    );
    let loader: SessionsPickerRecordLoader = Arc::new(move |request| {
        let actual = actual.clone();
        Box::pin(async move {
            if request.source_context == PickerSourceContext::DefaultHosted {
                SourceInventoryResult::Ready {
                    request,
                    binding: None,
                    progress: InventoryPageProgress::Complete,
                    snapshot: observed_records(Vec::new()),
                }
            } else {
                actual(request).await
            }
        })
    });
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    sender.send(key(KeyCode::F(2))).unwrap();
    let input = futures_util::stream::unfold(receiver, |mut receiver| async {
        receiver.recv().await.map(|event| (event, receiver))
    });
    let mut outcome: Option<SessionsPickerOutcome> = None;
    let held_ready = Arc::new(std::sync::atomic::AtomicBool::new(held.is_none()));
    let observed_held = held_ready.clone();
    let observer_sender = sender.clone();
    let observer = async {
        if let Some(peer) = held {
            peer.wait_for_held_page().await;
            observed_held.store(true, std::sync::atomic::Ordering::SeqCst);
            observer_sender.send(key(KeyCode::F(1))).unwrap();
        }
    };
    let render = tokio::time::timeout(Duration::from_secs(2), async {
        let mut picker = element! {SessionsPickerComponent(request,record_loader:Some(loader),width:180usize,height:60usize,selected_outcome_out:&mut outcome)};
        let frames = picker.mock_terminal_render_loop(MockTerminalConfig::with_events(input));
        tokio::pin!(frames);
        let mut selected = false;
        let mut captured = None;
        while let Some(canvas) = frames.next().await {
            let text = canvas.to_string();
            if !selected && text.contains("Choose machine") {
                selected = true;
                sender.send(key(KeyCode::Down)).unwrap();
                sender.send(key(KeyCode::Enter)).unwrap();
            }
            if captured.is_none()
                && text.contains("Machine: All machines")
                && text.contains(marker)
                && held_ready.load(std::sync::atomic::Ordering::SeqCst)
            {
                captured = Some(text);
                sender.send(control_key('c')).unwrap();
            }
        }
        captured.expect("actual source status must be rendered")
    });
    let (frame, ()) = tokio::join!(render, observer);
    let frame = frame.unwrap();
    assert!(outcome.is_none());
    frame
}

#[tokio::test]
async fn review_all_complete_real_source_is_not_reported_unavailable() {
    let source =
        SourceApiFixture::start("00000000-0000-4000-8000-000000000001", "source-model").await;
    let frame = capture_all_status(
        source.request("Observed source").source_context,
        source.loader(),
        "Observed source: 1 sessions",
        None,
    )
    .await;
    source.finish().await;
    assert!(
        !frame.contains("Partial view"),
        "completed sources are healthy: {frame}"
    );
    assert!(!frame.contains("connections need verification"));
    assert!(frame.contains("Source session"));
}

#[tokio::test]
async fn review_all_qualified_empty_source_is_not_unqualified() {
    let source = ReadPeer::start(ReplyScenario::RowBudget(0)).await;
    let frame = capture_all_status(
        source.request().source_context,
        source.loader(),
        "Observed source: 0 sessions",
        None,
    )
    .await;
    source.finish().await;
    assert!(
        !frame.contains("Partial view"),
        "a qualified empty result is not a rejected read: {frame}"
    );
}

#[tokio::test]
async fn review_all_held_source_reports_loading_not_false_unavailability() {
    let source = ReadPeer::start(ReplyScenario::HoldPage).await;
    let frame = capture_all_status(
        source.request().source_context,
        source.loader(),
        "Observed source: Loading",
        Some(&source),
    )
    .await;
    source.finish().await;
    assert!(
        frame.contains("Loading view"),
        "pending qualified read must remain loading: {frame}"
    );
    assert!(!frame.contains("unavailable: Observed source"));
}

#[tokio::test]
async fn review_all_rejection_uses_actual_failed_source_reason() {
    let source = ReadPeer::start(ReplyScenario::WrongPageEndpoint).await;
    let frame = capture_all_status(
        source.request().source_context,
        source.loader(),
        "Observed source: Unavailable",
        None,
    )
    .await;
    source.finish().await;
    assert!(frame.contains("Partial view") && frame.contains("unavailable: Observed source"));
    assert!(
        !frame.contains("connections need verification"),
        "failure must reflect the actual rejected read: {frame}"
    );
}

#[tokio::test]
async fn review_all_nonterminal_page_reports_unfinished_not_unavailable() {
    let source = ReadPeer::start(ReplyScenario::HoldSecondPage).await;
    let frame = capture_all_status(
        source.request().source_context,
        source.loader(),
        "1 fetched; unfinished",
        Some(&source),
    )
    .await;
    source.finish().await;
    assert!(
        frame.contains("Partial view") && frame.contains("unfinished: Observed source"),
        "unfinished pages remain partial: {frame}"
    );
    assert!(!frame.contains("unavailable: Observed source"));
}

#[tokio::test]
async fn review_machine_help_is_visible_in_collapsed_and_expanded_responsive_frames() {
    for width in [28usize, 40, 51, 52, 104, 108, 160] {
        for expanded in [false, true] {
            let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
            let input = futures_util::stream::unfold(receiver, |mut receiver| async {
                receiver.recv().await.map(|event| (event, receiver))
            });
            let frame = tokio::time::timeout(Duration::from_secs(2), async {
                let mut picker = element! {
                    SessionsPickerComponent(request: picker_request(), width, height: 60usize)
                };
                let frames =
                    picker.mock_terminal_render_loop(MockTerminalConfig::with_events(input));
                tokio::pin!(frames);
                let mut opened = false;
                let mut captured = None;
                while let Some(canvas) = frames.next().await {
                    let text = canvas.to_string();
                    if expanded && !opened {
                        opened = true;
                        sender.send(key(KeyCode::F(1))).unwrap();
                        continue;
                    }
                    if captured.is_none() && (!expanded || text.contains("Sessions Help")) {
                        captured = Some(text);
                        sender.send(control_key('c')).unwrap();
                    }
                }
                captured.expect("help frame")
            })
            .await
            .unwrap();
            assert!(
                if expanded {
                    frame.contains("Choose machine")
                } else {
                    frame.contains("▾")
                },
                "width{width}/expanded{expanded}: {frame}"
            );
        }
    }
}

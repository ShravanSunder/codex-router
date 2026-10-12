//! Progressive rendering through the real controller/dispatcher and controlled HTTP timing peer.
use super::*;
use crate::presentation::session_picker::PickerSourceContext;
use crate::sessions::configured_inventory_fault_tests::{ReadPeer, ReplyScenario};

#[tokio::test]
async fn review_partial_switch_query_change_keeps_selected_source_and_input() {
    let peer = ReadPeer::start(ReplyScenario::HoldSecondPage).await;
    let source = peer.request();
    let PickerSourceContext::ConfiguredHosted(profile) = source.source_context else {
        panic!("configured source")
    };
    let mut request = picker_request();
    request.root = source.query.root;
    request.source = source.query.source;
    request.records = vec![picker_record(
        "previous-id",
        "Previous committed row",
        "/original",
        "",
        "cli",
    )];
    request.router_registry = RouterRegistryRead::Ready(
        crate::sessions::router_connection_registry::RouterConnectionRegistry {
            routers: vec![profile],
        },
    );
    let previous = request.records.clone();
    let actual = peer.loader();
    let loader: SessionsPickerRecordLoader = Arc::new(move |request| {
        let actual = actual.clone();
        let previous = previous.clone();
        Box::pin(async move {
            if request.source_context == PickerSourceContext::DefaultHosted {
                crate::presentation::session_picker::SourceInventoryResult::Ready {
                    request,
                    binding: None,
                    progress: crate::presentation::session_picker::InventoryPageProgress::Complete,
                    snapshot: observed_records(previous),
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
    let mut last_frame = String::new();
    let result=tokio::time::timeout(Duration::from_secs(2),async {
        let mut picker=element! {SessionsPickerComponent(request,record_loader:Some(loader),width:180usize,height:50usize,selected_outcome_out:&mut outcome)};
        let frames=picker.mock_terminal_render_loop(MockTerminalConfig::with_events(input));tokio::pin!(frames);
        let mut selected=false;let mut typed=false;let mut observed=false;
        while let Some(canvas)=frames.next().await {
            let text=canvas.to_string();last_frame=text.clone();
            if !selected&&text.contains("Choose machine") {selected=true;for code in [KeyCode::Down,KeyCode::Down,KeyCode::Enter] {sender.send(key(code)).unwrap();}}
            if !typed&&text.contains("Machine: Source peer")&&text.contains("Literal source title") {typed=true;sender.send(key(KeyCode::Char('x'))).unwrap();}
            if typed&&!observed&&text.contains("Machine: Source peer")&&text.contains("Search: [x]")&&text.contains("source does not support this search expression") {
                observed=true;sender.send(control_key('c')).unwrap();
            }
        }
        observed
    }).await;
    peer.finish().await;
    assert!(
        result.is_ok_and(|observed| observed),
        "an unsupported new query must not roll back the user's source or search: {last_frame}"
    );
    assert!(outcome.is_none());
}

#[tokio::test]
async fn late_source_failure_restores_the_previous_machine_and_literal_row() {
    let peer = ReadPeer::start(ReplyScenario::HoldSecondWrongEndpoint).await;
    let source_request = peer.request();
    let PickerSourceContext::ConfiguredHosted(profile) = source_request.source_context else {
        panic!("configured source")
    };
    let mut request = picker_request();
    request.root = source_request.query.root;
    request.provider = source_request.query.provider;
    request.source = source_request.query.source;
    request.sort = source_request.query.sort;
    request.records = vec![picker_record(
        "previous-id",
        "Previous committed row",
        "/original",
        "",
        "cli",
    )];
    request.router_registry = RouterRegistryRead::Ready(
        crate::sessions::router_connection_registry::RouterConnectionRegistry {
            routers: vec![profile],
        },
    );
    let previous = request.records.clone();
    let actual = peer.loader();
    let loader: SessionsPickerRecordLoader = Arc::new(move |request| {
        let actual = actual.clone();
        let previous = previous.clone();
        Box::pin(async move {
            if request.source_context == PickerSourceContext::DefaultHosted {
                crate::presentation::session_picker::SourceInventoryResult::Ready {
                    request,
                    binding: None,
                    progress: crate::presentation::session_picker::InventoryPageProgress::Complete,
                    snapshot: observed_records(previous),
                }
            } else {
                actual(request).await
            }
        })
    });
    let mut outcome: Option<SessionsPickerOutcome> = None;
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    sender.send(key(KeyCode::F(2))).unwrap();
    let input = futures_util::stream::unfold(receiver, |mut receiver| async {
        receiver.recv().await.map(|event| (event, receiver))
    });
    tokio::time::timeout(Duration::from_secs(2),async {
        let mut picker=element! {SessionsPickerComponent(request,record_loader:Some(loader),width:160usize,height:40usize,selected_outcome_out:&mut outcome)};
        let canvases=picker.mock_terminal_render_loop(MockTerminalConfig::with_events(input));tokio::pin!(canvases);
        let mut selected=false;let mut partial=false;let mut restored=false;
        while let Some(canvas)=canvases.next().await {
            let text=canvas.to_string();
            if !selected&&text.contains("Choose machine") {
                selected=true;for code in [KeyCode::Down,KeyCode::Down,KeyCode::Enter] {sender.send(key(code)).unwrap();}
            }
            if !partial&&text.contains("Machine: Source peer")&&text.contains("Literal source title") {
                partial=true;peer.wait_for_held_page().await;peer.release_held_page().await;
            }
            if partial&&text.contains("Machine: This machine")&&text.contains("Previous committed row") {
                assert!(!text.contains("Literal source title"));restored=true;sender.send(control_key('c')).unwrap();
            }
        }
        assert!(partial&&restored,"partial source then failure must restore the prior view");
    }).await.unwrap();
    assert!(outcome.is_none());
    peer.finish().await;
}

#[tokio::test]
async fn first_source_page_renders_while_the_second_page_is_still_pending() {
    let peer = ReadPeer::start(ReplyScenario::HoldSecondPage).await;
    let source_request = peer.request();
    let PickerSourceContext::ConfiguredHosted(profile) = source_request.source_context else {
        panic!("configured source")
    };
    let mut request = picker_request();
    request.root = source_request.query.root;
    request.provider = source_request.query.provider;
    request.source = source_request.query.source;
    request.sort = source_request.query.sort;
    request.router_registry = RouterRegistryRead::Ready(
        crate::sessions::router_connection_registry::RouterConnectionRegistry {
            routers: vec![profile],
        },
    );
    let loader = peer.loader();
    let mut outcome: Option<SessionsPickerOutcome> = None;
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    sender.send(key(KeyCode::F(2))).unwrap();
    let input = futures_util::stream::unfold(receiver, |mut receiver| async {
        receiver.recv().await.map(|event| (event, receiver))
    });
    let mut last_frame = String::new();
    let observed=tokio::time::timeout(Duration::from_secs(2),async {
        let mut picker=element! {SessionsPickerComponent(request,record_loader:Some(loader),width:160usize,height:40usize,selected_outcome_out:&mut outcome)};
        let canvases=picker.mock_terminal_render_loop(MockTerminalConfig::with_events(input));tokio::pin!(canvases);
        let mut chosen=false;let mut observed=false;
        while let Some(canvas)=canvases.next().await {
            let text=canvas.to_string();last_frame=text.clone();
            if !chosen&&text.contains("Choose machine") {chosen=true;sender.send(key(KeyCode::Down)).unwrap();sender.send(key(KeyCode::Down)).unwrap();sender.send(key(KeyCode::Enter)).unwrap();}
            if !observed&&text.contains("Machine: Source peer")&&text.contains("Literal source title") {
                assert!(text.contains("fetched")&&text.contains("unfinished"),"first page must be visibly incomplete: {text}");
                observed=true;sender.send(control_key('c')).unwrap();
            }
        }
        observed
    }).await.unwrap_or_else(|_|panic!("first page must publish before releasing page2; last frame: {last_frame}"));
    assert!(observed);
    assert!(outcome.is_none());
    peer.finish().await;
}

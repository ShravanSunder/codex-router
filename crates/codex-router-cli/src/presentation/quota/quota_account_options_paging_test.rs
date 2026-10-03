use futures_util::StreamExt;
use iocraft::prelude::*;

use crate::quota_reset::reset_session_supervisor::ConfirmationSelection;
use crate::quota_reset::reset_session_supervisor::InspectionTabRequestId;
use crate::quota_reset::reset_session_supervisor::LiveWeeklyDisplayFacts;
use crate::quota_reset::reset_session_supervisor::ResetCreditDisplayRecord;
use crate::quota_reset::reset_session_supervisor::ResetCreditDisplayStatusDto;
use crate::quota_reset::reset_session_supervisor::ResetIntentSender;
use crate::quota_reset::reset_session_supervisor::ResetSessionIntent;
use crate::quota_reset::reset_session_supervisor::ResetSessionPorts;
use crate::quota_reset::reset_session_supervisor::ResetValueProvenance;
use crate::quota_reset::reset_session_supervisor::ResetWorkflowSnapshot;
use crate::quota_reset::reset_session_supervisor::WorkflowPhase;

use super::super::AccountOptionsCommandPort;
use super::super::AccountOptionsKeyEventContext;
use super::super::AccountOptionsState;
use super::super::handle_account_options_key_event;
use super::QuotaStatusComponent;
use super::acknowledged_event_stream;
use super::control_key;
use super::quota_two_account_view_model;

#[tokio::test]
async fn account_options_page_keys_use_rendered_capacity_and_ack_and_reinspection_reset_page() {
    let (ports, mut intent_receiver, snapshot_sender) =
        ResetSessionPorts::test_channels(options_paging_snapshot(WorkflowPhase::Browse));
    let session_driver = tokio::spawn(async move {
        expect_options_inspection_start(&mut intent_receiver).await;
        snapshot_sender
            .send(options_paging_snapshot(WorkflowPhase::Inspected))
            .expect("the component should still observe reset snapshots");

        let credits_tab_request = receive_options_tab_request(&mut intent_receiver).await;
        snapshot_sender
            .send(options_tab_ack_snapshot(credits_tab_request))
            .expect("the Credits tab acknowledgement should be published");

        let resets_tab_request = receive_options_tab_request(&mut intent_receiver).await;
        snapshot_sender
            .send(options_tab_ack_snapshot(resets_tab_request))
            .expect("the Resets tab acknowledgement should be published");
        expect_options_inspection_start(&mut intent_receiver).await;
        snapshot_sender
            .send(options_paging_snapshot(WorkflowPhase::Inspected))
            .expect("the new reset inspection should be published");

        assert_eq!(
            intent_receiver.recv().await,
            Some(ResetSessionIntent::Cancel),
            "closing an inspected pane should cancel its reset inspection"
        );
        snapshot_sender
            .send(options_paging_snapshot(WorkflowPhase::Browse))
            .expect("the canceled inspection should return to browse");

        expect_options_inspection_start(&mut intent_receiver).await;
        snapshot_sender
            .send(options_paging_snapshot(WorkflowPhase::Inspected))
            .expect("the reopened pane should receive a fresh inspection");
        assert_eq!(
            intent_receiver.recv().await,
            Some(ResetSessionIntent::Cancel),
            "closing the reopened pane should cancel its reset inspection"
        );
        snapshot_sender
            .send(options_paging_snapshot(WorkflowPhase::Browse))
            .expect("the reopened inspection should return to browse");
    });

    let ordered_events = vec![
        (TerminalEvent::Key(control_key('r')), Some("1-1 of 5")),
        (
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::PageDown)),
            Some("2-2 of 5"),
        ),
        (
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::PageDown)),
            Some("3-3 of 5"),
        ),
        (
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::PageDown)),
            Some("4-4 of 5"),
        ),
        (
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::PageDown)),
            Some("5-5 of 5"),
        ),
        (
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::PageUp)),
            Some("4-4 of 5"),
        ),
        (
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Tab)),
            Some("Credit usage"),
        ),
        (
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Tab)),
            Some("1-1 of 5"),
        ),
        (
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::PageDown)),
            Some("2-2 of 5"),
        ),
        (
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Esc)),
            Some("enter inspect  tab credits  esc back"),
        ),
        (
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Esc)),
            Some("ctrl-r account options"),
        ),
        (TerminalEvent::Key(control_key('r')), Some("1-1 of 5")),
        (
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Esc)),
            Some("enter inspect  tab credits  esc back"),
        ),
        (
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Esc)),
            Some("ctrl-r account options"),
        ),
        (
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Char('q'))),
            None,
        ),
    ];
    let (acknowledged_events, acknowledgement_sender) = acknowledged_event_stream(ordered_events);
    let mut terminal = element! {
        QuotaStatusComponent(
            view_model: quota_two_account_view_model(),
            width: 100usize,
            height: 19usize,
            reset_intent_sender: Some(ports.intent_sender),
            reset_snapshot_receiver: Some(ports.snapshot_receiver),
        )
    };
    let component = terminal
        .mock_terminal_render_loop(MockTerminalConfig::with_events(acknowledged_events))
        .map(|canvas| canvas.to_string())
        .inspect(move |frame| {
            for marker in [
                "1-1 of 5",
                "2-2 of 5",
                "3-3 of 5",
                "4-4 of 5",
                "5-5 of 5",
                "Credit usage",
                "enter inspect  tab credits  esc back",
                "ctrl-r account options",
            ] {
                if frame.contains(marker) {
                    let _ = acknowledgement_sender.send(marker);
                }
            }
        })
        .collect::<Vec<_>>();
    let frames = tokio::time::timeout(std::time::Duration::from_secs(3), component)
        .await
        .expect("component should render every requested inventory page");

    session_driver
        .await
        .expect("the reset session driver should observe the full paging journey");
    assert_ordered_inventory_pages(
        &frames,
        &[
            ("1-1 of 5", "voucher-1"),
            ("2-2 of 5", "voucher-2"),
            ("3-3 of 5", "voucher-3"),
            ("4-4 of 5", "voucher-4"),
            ("5-5 of 5", "voucher-5"),
            ("4-4 of 5", "voucher-4"),
            ("1-1 of 5", "voucher-1"),
            ("2-2 of 5", "voucher-2"),
            ("1-1 of 5", "voucher-1"),
        ],
    );
    assert_ordered_frame_markers(
        &frames,
        &[
            "Credit usage",
            "enter inspect  tab credits  esc back",
            "ctrl-r account options",
            "enter inspect  tab credits  esc back",
            "ctrl-r account options",
        ],
    );
}

#[tokio::test]
async fn beginning_a_new_reset_inspection_resets_a_nonzero_inventory_page_start() {
    let (ports, mut intent_receiver, _snapshot_sender) =
        ResetSessionPorts::test_channels(options_paging_snapshot(WorkflowPhase::Browse));
    let (events, acknowledgement_sender) = acknowledged_event_stream(vec![
        (
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Enter)),
            Some("inventory page start: 0"),
        ),
        (TerminalEvent::Key(control_key('c')), None),
    ]);
    let mut terminal = element! {
        NewInspectionPageResetProbe(reset_intent_sender: Some(ports.intent_sender))
    };
    let component = terminal
        .mock_terminal_render_loop(MockTerminalConfig::with_events(events))
        .map(|canvas| canvas.to_string())
        .inspect(move |frame| {
            if frame.contains("inventory page start: 0") {
                let _ = acknowledgement_sender.send("inventory page start: 0");
            }
        })
        .collect::<Vec<_>>();
    let frames = tokio::time::timeout(std::time::Duration::from_secs(3), component)
        .await
        .expect("inspection-start callback should render the reset page index");

    assert!(
        frames
            .iter()
            .any(|frame| frame.contains("inventory page start: 4")),
        "the probe should start from a nonzero inventory page"
    );
    assert!(
        frames
            .iter()
            .any(|frame| frame.contains("inventory page start: 0")),
        "a new inspection should render from the first inventory page"
    );
    assert!(matches!(
        intent_receiver.recv().await,
        Some(ResetSessionIntent::BeginInspection { .. })
    ));
}

#[derive(Default, Props)]
struct NewInspectionPageResetProbeProps {
    reset_intent_sender: Option<ResetIntentSender>,
}

#[component]
fn NewInspectionPageResetProbe(
    props: &mut NewInspectionPageResetProbeProps,
    mut hooks: Hooks,
) -> impl Into<AnyElement<'static>> {
    let mut system = hooks.use_context_mut::<SystemContext>();
    let view_model = quota_two_account_view_model();
    let account_options =
        hooks.use_state(|| Some(AccountOptionsState::new(&view_model.rows[0], 1)));
    let reset_target = hooks
        .use_state(|| None::<super::super::super::quota_reset_presentation_model::ResetPaneTarget>);
    let command_port = hooks.use_memo(AccountOptionsCommandPort::new, ());
    let inventory_page_start = hooks.use_state(|| 4usize);
    let mut should_exit = hooks.use_state(|| false);
    hooks.use_terminal_events({
        let mut account_options = account_options;
        let mut reset_target = reset_target;
        let mut inventory_page_start = inventory_page_start;
        let command_port = command_port.clone();
        let reset_intent_sender = props.reset_intent_sender.clone();
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
            if code == KeyCode::Char('c') && modifiers.contains(KeyModifiers::CONTROL) {
                should_exit.set(true);
                return;
            }
            let _ = handle_account_options_key_event(
                &mut account_options,
                &mut reset_target,
                AccountOptionsKeyEventContext {
                    phase: WorkflowPhase::Browse,
                    code,
                    modifiers,
                    command_port: &command_port,
                    reset_snapshot: None,
                    reset_intent_sender: reset_intent_sender.as_ref(),
                    inventory_page_start: &mut inventory_page_start,
                    now_unix_seconds: 1_000,
                },
            );
        }
    });
    if *should_exit.read() {
        system.exit();
    }
    element! {
        Text(content: format!("inventory page start: {}", inventory_page_start.get()))
    }
}

async fn expect_options_inspection_start(
    intent_receiver: &mut tokio::sync::mpsc::UnboundedReceiver<ResetSessionIntent>,
) {
    assert!(matches!(
        intent_receiver.recv().await,
        Some(ResetSessionIntent::BeginInspection { .. })
    ));
}

async fn receive_options_tab_request(
    intent_receiver: &mut tokio::sync::mpsc::UnboundedReceiver<ResetSessionIntent>,
) -> InspectionTabRequestId {
    match intent_receiver.recv().await {
        Some(ResetSessionIntent::CancelInspectionForTab { request_id, .. }) => request_id,
        other => panic!("expected tab inspection cancellation, received {other:?}"),
    }
}

fn options_paging_snapshot(phase: WorkflowPhase) -> ResetWorkflowSnapshot {
    let inventory = if phase == WorkflowPhase::Inspected {
        (1..=5)
            .map(|index| ResetCreditDisplayRecord {
                id_hint: format!("voucher-{index}"),
                status: ResetCreditDisplayStatusDto::Available,
                title: Some(format!("Voucher {index}")),
                expires_unix_seconds: Some(1_900_000_000 + i64::from(index)),
                earliest_usable: index == 1,
            })
            .collect()
    } else {
        Vec::new()
    };
    ResetWorkflowSnapshot::test_snapshot(
        phase,
        ConfirmationSelection::No,
        Default::default(),
        None,
        (phase == WorkflowPhase::Inspected).then_some(LiveWeeklyDisplayFacts {
            remaining_percent: 4,
            provenance: ResetValueProvenance::CurrentLive,
        }),
        inventory,
        None,
    )
}

fn options_tab_ack_snapshot(request_id: InspectionTabRequestId) -> ResetWorkflowSnapshot {
    ResetWorkflowSnapshot::test_snapshot(
        WorkflowPhase::Browse,
        ConfirmationSelection::No,
        Default::default(),
        None,
        None,
        Vec::new(),
        None,
    )
    .with_last_processed_inspection_tab_request_for_test(request_id)
}

fn assert_ordered_frame_markers(frames: &[String], markers: &[&str]) {
    let mut next_frame_index = 0;
    for marker in markers {
        let Some(frame_index) = frames
            .iter()
            .enumerate()
            .skip(next_frame_index)
            .find_map(|(index, frame)| frame.contains(marker).then_some(index))
        else {
            panic!("expected ordered frame marker {marker:?} was missing: {frames:?}");
        };
        next_frame_index = frame_index + 1;
    }
}

fn assert_ordered_inventory_pages(frames: &[String], expected_pages: &[(&str, &str)]) {
    let mut next_frame_index = 0;
    for (page_range, voucher_id) in expected_pages {
        let voucher_marker = format!("[{voucher_id}]");
        let Some(frame_index) =
            frames
                .iter()
                .enumerate()
                .skip(next_frame_index)
                .find_map(|(index, frame)| {
                    (frame.contains("[ Resets ]")
                        && frame.contains(page_range)
                        && frame.contains(&voucher_marker))
                    .then_some(index)
                })
        else {
            panic!(
                "expected ordered inventory page {page_range} with {voucher_marker} was missing: {frames:?}"
            );
        };
        next_frame_index = frame_index + 1;
    }
}

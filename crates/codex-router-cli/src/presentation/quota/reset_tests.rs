use std::time::Duration;

use futures_util::StreamExt;
use iocraft::prelude::*;

use crate::quota_reset::supervisor::ConfirmationSelection;
use crate::quota_reset::supervisor::KnownConsumeOutcome;
use crate::quota_reset::supervisor::LiveWeeklyDisplayFacts;
use crate::quota_reset::supervisor::OperationActivity;
use crate::quota_reset::supervisor::OperationSuccess;
use crate::quota_reset::supervisor::ResetCreditDisplayRecord;
use crate::quota_reset::supervisor::ResetCreditDisplayStatusDto;
use crate::quota_reset::supervisor::ResetEligibilityDisabledReason;
use crate::quota_reset::supervisor::ResetSessionIntent;
use crate::quota_reset::supervisor::ResetSessionPorts;
use crate::quota_reset::supervisor::ResetValueProvenance;
use crate::quota_reset::supervisor::ResetWorkflowSnapshot;
use crate::quota_reset::supervisor::TestConsumeUnknownReason as ConsumeUnknownReason;
use crate::quota_reset::supervisor::TestRenderSafeFailure as RenderSafeFailure;
use crate::quota_reset::supervisor::WorkflowPhase;
use crate::quota_reset::supervisor::WorkflowResult;
use crate::quota_reset::workflow::WorkflowActivities;

use super::component::QuotaStatusComponent;
use super::interaction::ResetKeyAction;
use super::interaction::reset_key_action;
use super::reset::render_reset_panel;
use super::reset_model::ResetPaneTarget;
use super::tests::assert_quota_golden;
use super::tests::quota_two_account_view_model;

#[tokio::test]
async fn ctrl_r_inspects_the_focused_stable_account_and_generation() {
    let snapshot = test_snapshot(WorkflowPhase::Browse, false);
    let (ports, mut intent_receiver, _snapshot_sender) = ResetSessionPorts::test_channels(snapshot);
    let mut view_model = quota_two_account_view_model();
    view_model.rows[0].account = "duplicate".to_owned();
    view_model.rows[1].account = "duplicate".to_owned();
    let expected_account_id = view_model.rows[1].account_id.clone();
    let events = vec![
        TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Down)),
        TerminalEvent::Key(control_key('r')),
        TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Esc)),
    ];

    let _frames = element! {
        QuotaStatusComponent(
            view_model,
            width: 120usize,
            height: 48usize,
            reset_intent_sender: Some(ports.intent_sender),
            reset_snapshot_receiver: Some(ports.snapshot_receiver),
        )
    }
    .mock_terminal_render_loop(MockTerminalConfig::with_events(
        futures_util::stream::iter(events).then(|event| async move {
            tokio::time::sleep(Duration::from_millis(1)).await;
            event
        }),
    ))
    .collect::<Vec<_>>()
    .await;

    let intent = intent_receiver
        .recv()
        .await
        .expect("Ctrl-R should emit one inspection intent");
    match intent {
        ResetSessionIntent::BeginInspection {
            account_id,
            active_credential_generation,
            ..
        } => {
            assert_eq!(account_id, expected_account_id);
            assert_eq!(active_credential_generation, 1);
        }
        other => panic!("expected focused-account inspection, got {other:?}"),
    }
}

#[tokio::test]
async fn disabled_yes_cannot_receive_focus_and_enter_cancels_from_default_no() {
    let snapshot = test_snapshot(WorkflowPhase::Confirming, false);
    let (ports, mut intent_receiver, _snapshot_sender) = ResetSessionPorts::test_channels(snapshot);
    let events = vec![
        TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Right)),
        TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Enter)),
        TerminalEvent::Key(control_key('c')),
    ];

    let _frames = element! {
        QuotaStatusComponent(
            view_model: quota_two_account_view_model(),
            width: 120usize,
            height: 48usize,
            reset_intent_sender: Some(ports.intent_sender),
            reset_snapshot_receiver: Some(ports.snapshot_receiver),
        )
    }
    .mock_terminal_render_loop(MockTerminalConfig::with_events(
        futures_util::stream::iter(events).then(|event| async move {
            tokio::time::sleep(Duration::from_millis(1)).await;
            event
        }),
    ))
    .collect::<Vec<_>>()
    .await;

    assert_eq!(
        intent_receiver.recv().await,
        Some(ResetSessionIntent::Cancel)
    );
    assert!(intent_receiver.try_recv().is_err());
}

#[tokio::test]
async fn browse_reset_resize_and_cancel_restores_the_existing_shell() {
    for width in [159usize, 160] {
        let browse_snapshot = test_snapshot(WorkflowPhase::Browse, false);
        let (ports, mut intent_receiver, snapshot_sender) =
            ResetSessionPorts::test_channels(browse_snapshot.clone());
        let session_driver = tokio::spawn(async move {
            assert!(matches!(
                intent_receiver.recv().await,
                Some(ResetSessionIntent::BeginInspection { .. })
            ));
            snapshot_sender
                .send(test_snapshot(WorkflowPhase::Inspecting, false))
                .expect("presentation watch should remain connected");
            assert_eq!(
                intent_receiver.recv().await,
                Some(ResetSessionIntent::Cancel)
            );
            snapshot_sender
                .send(browse_snapshot)
                .expect("presentation watch should remain connected");
        });
        let events = vec![
            TerminalEvent::Key(control_key('r')),
            TerminalEvent::Resize(width.saturating_sub(1) as u16, 24),
            TerminalEvent::Resize(width as u16, 24),
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Esc)),
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Char('q'))),
        ];

        let frames = element! {
            QuotaStatusComponent(
                view_model: quota_two_account_view_model(),
                width,
                height: 24usize,
                reset_intent_sender: Some(ports.intent_sender),
                reset_snapshot_receiver: Some(ports.snapshot_receiver),
            )
        }
        .mock_terminal_render_loop(MockTerminalConfig::with_events(
            futures_util::stream::iter(events).then(|event| async move {
                tokio::time::sleep(Duration::from_millis(3)).await;
                event
            }),
        ))
        .map(|canvas| canvas.to_string())
        .collect::<Vec<_>>()
        .await;

        session_driver.await.expect("session driver should finish");
        assert!(
            frames.iter().any(|frame| {
                frame.contains("Reset credit")
                    && frame.contains("❯ alpha")
                    && frame.contains("saved credits")
            }),
            "reset must replace only detail while the account list remains: {frames:?}"
        );
        assert!(
            frames
                .last()
                .is_some_and(|frame| frame.contains("Selected account")),
            "browse detail should return after cancellation: {frames:?}"
        );
    }
}

#[test]
fn reset_detail_renders_all_five_semantic_operation_rows_and_safe_target_tag() {
    let mut activities = WorkflowActivities::default();
    activities.inspection_live_usage = crate::quota_reset::supervisor::OperationActivity::Loading;
    activities.inspection_credit_inventory =
        crate::quota_reset::supervisor::OperationActivity::Failed {
            failure: RenderSafeFailure::Transport,
            previous: None,
        };
    activities.consume_credit =
        crate::quota_reset::supervisor::OperationActivity::RequestDispatchedAwaitingOutcome;
    let snapshot = ResetWorkflowSnapshot::test_snapshot(
        WorkflowPhase::Committing,
        ConfirmationSelection::No,
        false,
        activities,
        None,
        None,
        Vec::new(),
        Some(ResetEligibilityDisabledReason::LiveInspectionIncomplete),
    );
    let target = ResetPaneTarget {
        account_id: quota_two_account_view_model().rows[0].account_id.clone(),
        active_credential_generation: 1,
        account_label: "duplicate".to_owned(),
        account_tag: "safe-tag".to_owned(),
        saved_reset_credits: "3 resets".to_owned(),
        saved_weekly_window: "saved 4% remaining".to_owned(),
    };

    let text = render_reset_panel(&snapshot, &target, 100, 24, 0, 4)
        .render(None)
        .to_string();

    for label in [
        "inspect usage",
        "inspect credits",
        "revalidate usage",
        "revalidate credits",
        "consume credit",
    ] {
        assert!(text.contains(label), "missing {label}:\n{text}");
    }
    assert!(text.contains("duplicate  [safe-tag]"), "{text}");
    assert!(text.contains("request dispatched"), "{text}");
}

#[test]
fn inventory_paging_is_bounded_and_deterministic() {
    assert_eq!(super::reset_model::credit_page_start(0, 9, 4, true), 4);
    assert_eq!(super::reset_model::credit_page_start(4, 9, 4, true), 8);
    assert_eq!(super::reset_model::credit_page_start(8, 9, 4, true), 8);
    assert_eq!(super::reset_model::credit_page_start(8, 9, 4, false), 4);
}

#[test]
fn reset_semantic_frames_render_at_narrow_stacked_boundary_and_wide_widths() {
    let credit_inventory = test_credit_inventory();
    let confirmation = ResetWorkflowSnapshot::test_snapshot(
        WorkflowPhase::Confirming,
        ConfirmationSelection::No,
        true,
        completed_inspection_activities(),
        None,
        Some(LiveWeeklyDisplayFacts {
            remaining_percent: 0,
            provenance: ResetValueProvenance::CurrentLive,
        }),
        credit_inventory,
        None,
    );
    let unknown = ResetWorkflowSnapshot::test_snapshot(
        WorkflowPhase::Result,
        ConfirmationSelection::No,
        false,
        unknown_result_activities(),
        Some(WorkflowResult::OutcomeUnknown(
            ConsumeUnknownReason::Transport,
        )),
        None,
        Vec::new(),
        None,
    );
    let target = test_reset_target();

    for width in [48usize, 100, 159, 160, 200] {
        let confirmation_frame = render_reset_panel(&confirmation, &target, width, 24, 0, 4)
            .render(None)
            .to_string();
        let result_frame = render_reset_panel(&unknown, &target, width, 24, 0, 4)
            .render(None)
            .to_string();
        assert!(
            confirmation_frame.contains("[No]"),
            "width {width}: {confirmation_frame}"
        );
        assert!(
            confirmation_frame.contains("Yes"),
            "width {width}: {confirmation_frame}"
        );
        assert!(
            result_frame.contains("Outcome unknown"),
            "width {width}: {result_frame}"
        );
        assert!(
            result_frame.contains("Saved quota may remain stale"),
            "width {width}: {result_frame}"
        );
        assert_quota_golden(
            &format!("reset-confirming-width-{width}"),
            &confirmation_frame,
        );
        assert_quota_golden(&format!("reset-result-width-{width}"), &result_frame);
    }
}

#[test]
fn reset_operation_transition_frames_are_semantic_and_state_valid() {
    let target = test_reset_target();
    let scenarios = reset_operation_scenarios();
    for width in [100usize, 160] {
        for (name, snapshot, required_text) in &scenarios {
            let frame = render_reset_panel(snapshot, &target, width, 24, 0, 4)
                .render(None)
                .to_string();
            for required in required_text.iter().copied() {
                assert!(
                    frame.contains(required),
                    "{name} width {width} missing {required:?}:\n{frame}"
                );
            }
            assert_quota_golden(&format!("reset-{name}-width-{width}"), &frame);
        }
    }
}

#[test]
fn reset_mode_key_contract_is_fail_closed_and_committing_disables_exit() {
    let no_modifiers = KeyModifiers::NONE;
    assert_eq!(
        reset_key_action(
            WorkflowPhase::Inspecting,
            ConfirmationSelection::No,
            false,
            KeyCode::Enter,
            no_modifiers,
        ),
        ResetKeyAction::None
    );
    assert_eq!(
        reset_key_action(
            WorkflowPhase::Inspected,
            ConfirmationSelection::No,
            false,
            KeyCode::Enter,
            no_modifiers,
        ),
        ResetKeyAction::OpenConfirmation
    );
    assert_eq!(
        reset_key_action(
            WorkflowPhase::Confirming,
            ConfirmationSelection::No,
            false,
            KeyCode::Right,
            no_modifiers,
        ),
        ResetKeyAction::None
    );
    assert_eq!(
        reset_key_action(
            WorkflowPhase::Confirming,
            ConfirmationSelection::Yes,
            true,
            KeyCode::Enter,
            no_modifiers,
        ),
        ResetKeyAction::Confirm
    );
    assert_eq!(
        reset_key_action(
            WorkflowPhase::Result,
            ConfirmationSelection::No,
            false,
            KeyCode::Esc,
            no_modifiers,
        ),
        ResetKeyAction::DismissResult
    );
    assert_eq!(
        reset_key_action(
            WorkflowPhase::Committing,
            ConfirmationSelection::No,
            false,
            KeyCode::Esc,
            no_modifiers,
        ),
        ResetKeyAction::None
    );
    assert_eq!(
        reset_key_action(
            WorkflowPhase::Committing,
            ConfirmationSelection::No,
            false,
            KeyCode::Char('c'),
            KeyModifiers::CONTROL,
        ),
        ResetKeyAction::None
    );
}

fn test_snapshot(phase: WorkflowPhase, yes_enabled: bool) -> ResetWorkflowSnapshot {
    ResetWorkflowSnapshot::test_snapshot(
        phase,
        ConfirmationSelection::No,
        yes_enabled,
        WorkflowActivities::default(),
        None,
        None,
        Vec::new(),
        Some(ResetEligibilityDisabledReason::LiveInspectionIncomplete),
    )
}

include!("tests/reset_scenarios.rs");

fn test_reset_target() -> ResetPaneTarget {
    ResetPaneTarget {
        account_id: quota_two_account_view_model().rows[0].account_id.clone(),
        active_credential_generation: 1,
        account_label: "alpha".to_owned(),
        account_tag: "alpha-tag".to_owned(),
        saved_reset_credits: "3 resets".to_owned(),
        saved_weekly_window: "saved 4% remaining".to_owned(),
    }
}

fn control_key(character: char) -> KeyEvent {
    let mut event = KeyEvent::new(KeyEventKind::Press, KeyCode::Char(character));
    event.modifiers = KeyModifiers::CONTROL;
    event
}

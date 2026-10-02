use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use iocraft::prelude::*;

use crate::quota_reset::reset_session_supervisor::WorkflowPhase;

use super::super::quota_browse_presentation_test::quota_two_account_view_model;
use super::super::quota_status_component::QuotaStatusComponent;
use super::rendering::AccountOptionsPanelProps;
use super::rendering::render_account_options_panel;
use super::*;

#[test]
fn account_options_render_explicit_tabs_and_unchecked_observation() {
    let view_model = quota_two_account_view_model();
    let mut options = AccountOptionsState::new(&view_model.rows[0], 1);
    options.tab = AccountOptionsTab::Credits;
    assert_eq!(
        account_options_footer(&options),
        "tab resets  enter edit  r refresh  esc back"
    );

    let frame = render_account_options_panel(AccountOptionsPanelProps {
        options: &options,
        reset_snapshot: None,
        reset_target: None,
        width: 100,
        height: 24,
        inventory_page_start: 0,
        spinner_tick: 0,
    })
    .render(None)
    .to_string();

    assert!(frame.contains("Resets  [ Credits ]"), "{frame}");
    assert!(frame.contains("Credit balance"), "{frame}");
    assert!(frame.contains("Credit usage"), "{frame}");
    assert!(frame.contains("not checked"), "{frame}");
    assert!(!frame.contains("checked unknown ago"), "{frame}");

    options.tab = AccountOptionsTab::Resets;
    assert_eq!(
        account_options_footer(&options),
        "enter inspect  tab credits  esc back"
    );
    let reset_frame = render_account_options_panel(AccountOptionsPanelProps {
        options: &options,
        reset_snapshot: None,
        reset_target: None,
        width: 100,
        height: 24,
        inventory_page_start: 0,
        spinner_tick: 0,
    })
    .render(None)
    .to_string();
    assert!(reset_frame.contains("[ Resets ]  Credits"), "{reset_frame}");
}

#[test]
fn credit_policy_options_render_in_keyboard_selection_order() {
    let view_model = quota_two_account_view_model();
    let mut options = AccountOptionsState::new(&view_model.rows[0], 1);
    options.tab = AccountOptionsTab::Credits;
    let editing = account_options_key_action(
        &options,
        WorkflowPhase::Browse,
        KeyCode::Enter,
        KeyModifiers::empty(),
    );
    update_credit_policy_editor(&mut options, editing, None);

    let left_action = account_options_key_action(
        &options,
        WorkflowPhase::Browse,
        KeyCode::Left,
        KeyModifiers::empty(),
    );
    assert_eq!(left_action, AccountOptionsKeyAction::SelectDisallow);
    update_credit_policy_editor(&mut options, left_action, None);
    let disallow_frame = render_account_options_panel(AccountOptionsPanelProps {
        options: &options,
        reset_snapshot: None,
        reset_target: None,
        width: 100,
        height: 24,
        inventory_page_start: 0,
        spinner_tick: 0,
    })
    .render(None)
    .to_string();
    let disallow_line = disallow_frame
        .lines()
        .find(|line| line.contains("Usage") && line.contains("Allow") && line.contains("Disallow"))
        .expect("editing should render both credit policies on the Usage row");
    let disallow_position = disallow_line
        .find("Disallow")
        .expect("Disallow should be visible");
    let allow_position = disallow_line
        .find("Allow")
        .expect("Allow should be visible");
    let selected_position = disallow_line
        .find('›')
        .expect("selection marker should be visible");
    assert!(
        disallow_position < allow_position && selected_position < disallow_position,
        "Left selects the visually left Disallow option, but the row was:\n{disallow_line}"
    );

    let right_action = account_options_key_action(
        &options,
        WorkflowPhase::Browse,
        KeyCode::Right,
        KeyModifiers::empty(),
    );
    assert_eq!(right_action, AccountOptionsKeyAction::SelectAllow);
    update_credit_policy_editor(&mut options, right_action, None);
    let allow_frame = render_account_options_panel(AccountOptionsPanelProps {
        options: &options,
        reset_snapshot: None,
        reset_target: None,
        width: 100,
        height: 24,
        inventory_page_start: 0,
        spinner_tick: 0,
    })
    .render(None)
    .to_string();
    let allow_line = allow_frame
        .lines()
        .find(|line| line.contains("Usage") && line.contains("Allow") && line.contains("Disallow"))
        .expect("editing should continue to render both credit policies");
    let disallow_position = allow_line
        .find("Disallow")
        .expect("Disallow should remain visible");
    let allow_position = allow_line
        .find("Allow")
        .expect("Allow should remain visible");
    let selected_position = allow_line
        .find('›')
        .expect("selection marker should be visible");
    assert!(
        disallow_position < selected_position && selected_position < allow_position,
        "Right selects the visually right Allow option, but the row was:\n{allow_line}"
    );
}

#[tokio::test]
async fn control_c_in_credit_browsing_exits_the_component() {
    assert_credit_control_c_exits_the_component(false).await;
}

#[tokio::test]
async fn control_c_in_credit_editor_exits_the_component() {
    assert_credit_control_c_exits_the_component(true).await;
}

#[test]
fn protected_reset_phases_keep_forced_keys_with_the_reset_owner() {
    let view_model = quota_two_account_view_model();
    let mut options = AccountOptionsState::new(&view_model.rows[0], 1);
    options.tab = AccountOptionsTab::Credits;

    for phase in [
        WorkflowPhase::Confirming,
        WorkflowPhase::Revalidating,
        WorkflowPhase::Committing,
        WorkflowPhase::Result,
    ] {
        for (code, modifiers) in [
            (KeyCode::Char('c'), KeyModifiers::CONTROL),
            (KeyCode::Char('d'), KeyModifiers::CONTROL),
        ] {
            let credit_action = account_options_key_action(&options, phase, code, modifiers);
            assert_eq!(credit_action, AccountOptionsKeyAction::Ignore, "{phase:?}");
            assert_eq!(
                update_credit_policy_editor(&mut options, credit_action, None),
                None,
                "{phase:?} input must not emit a credit mutation"
            );

            options.tab = AccountOptionsTab::Resets;
            assert_eq!(
                account_options_key_action(&options, phase, code, modifiers),
                AccountOptionsKeyAction::ResetWorkflow,
                "the reset workflow retains control during {phase:?}"
            );
            let reset_action = super::super::quota_reset_keyboard_interaction::reset_key_action(
                phase,
                crate::quota_reset::reset_session_supervisor::ConfirmationSelection::No,
                false,
                code,
                modifiers,
            );
            if phase == WorkflowPhase::Committing {
                assert_eq!(
                    reset_action,
                    super::super::quota_reset_keyboard_interaction::ResetKeyAction::None,
                    "Committing cannot be canceled or force-exited by a key"
                );
            } else {
                assert_eq!(
                    reset_action,
                    super::super::quota_reset_keyboard_interaction::ResetKeyAction::ExitPrecommit,
                    "{phase:?} keeps its existing reset shutdown path"
                );
            }
            options.tab = AccountOptionsTab::Credits;
        }
    }
}

#[test]
fn credit_options_truncate_dynamic_status_and_help_inside_narrow_pane() {
    let view_model = quota_two_account_view_model();
    let mut options = AccountOptionsState::new(&view_model.rows[0], 1);
    options.tab = AccountOptionsTab::Credits;
    options.target.weekly_quota_floor_percent = 95;
    options.target.credit_usage = crate::quota::CreditUsageStatus {
        policy: CreditUsagePolicy::Disallow,
        provider_observation: codex_router_core::credit_usage::CreditProviderObservation::new(
            CreditAvailability::Unknown,
            codex_router_core::credit_usage::CreditSpendControl::Unknown,
            Some(codex_router_core::credit_usage::CreditProviderLimitReason::Unknown),
        ),
        freshness: CreditUsageFreshness::Stale,
        age_label: "very old synthetic sample".to_owned(),
    };
    options.editor = Some(CreditPolicyEditorState {
        saved_policy: CreditUsagePolicy::Disallow,
        draft_policy: CreditUsagePolicy::Allow,
        phase: CreditPolicyEditorPhase::SaveFailed(
            CreditUsagePolicySaveError::StateOperationFailed,
        ),
        operation_generation: 2,
    });
    options.message = Some(AccountOptionsMessage::RefreshFailed);

    let frame = render_account_options_panel(AccountOptionsPanelProps {
        options: &options,
        reset_snapshot: None,
        reset_target: None,
        width: 48,
        height: 24,
        inventory_page_start: 0,
        spinner_tick: 0,
    })
    .render(None)
    .to_string();

    assert!(frame.contains("Weekly floor"), "{frame}");
    assert!(frame.contains("blocks credit routing"), "{frame}");
    assert!(frame.contains("Provider spend-control"), "{frame}");
    assert!(frame.contains("credit preference save failed"), "{frame}");
    assert!(frame.contains("Credit refresh failed"), "{frame}");
    assert!(
        frame
            .lines()
            .filter(|line| {
                line.contains("Credits may")
                    || line.contains("Allow is limited")
                    || line.contains("Credit refresh failed")
            })
            .all(|line| line.contains("… │")),
        "long help and status rows should show truncation before the inner border:\n{frame}"
    );
    assert!(
        frame
            .lines()
            .find(|line| line.contains("Provider spend-control"))
            .is_some_and(|line| line.ends_with('│')),
        "provider-control copy should end inside the inner border:\n{frame}"
    );
    assert!(
        frame.lines().all(|line| line.chars().count() <= 48),
        "account-options rows must stay within their 48-column pane:\n{frame}"
    );
}

#[test]
fn reset_review_and_consume_phases_do_not_offer_tab_switches() {
    let view_model = quota_two_account_view_model();
    let options = AccountOptionsState::new(&view_model.rows[0], 1);
    for phase in [
        WorkflowPhase::Confirming,
        WorkflowPhase::Revalidating,
        WorkflowPhase::Committing,
        WorkflowPhase::Result,
    ] {
        let action = account_options_key_action(
            &options,
            phase,
            iocraft::prelude::KeyCode::Tab,
            KeyModifiers::empty(),
        );
        assert_ne!(
            action,
            AccountOptionsKeyAction::SwitchTab(AccountOptionsTab::Credits),
            "tab switching must remain unavailable during {phase:?}"
        );
    }
}

#[test]
fn pending_tab_transition_keeps_escape_back_and_truthful_shortcuts() {
    let view_model = quota_two_account_view_model();
    let mut options = AccountOptionsState::new(&view_model.rows[0], 1);
    options.tab = AccountOptionsTab::Credits;
    options.pending_tab = Some(AccountOptionsTab::Resets);
    options.pending_tab_request = Some(InspectionTabRequestId::new(1, 2));

    assert_eq!(
        account_options_key_action(
            &options,
            WorkflowPhase::Browse,
            KeyCode::Esc,
            KeyModifiers::empty(),
        ),
        AccountOptionsKeyAction::Close,
        "Esc should close the pane while a tab acknowledgement waits behind refresh"
    );
    assert_eq!(
        account_options_key_action(
            &options,
            WorkflowPhase::Browse,
            KeyCode::Char('c'),
            KeyModifiers::CONTROL,
        ),
        AccountOptionsKeyAction::ExitPending,
        "forced exit should reach the component while a tab acknowledgement is pending"
    );
    assert_eq!(
        account_options_key_action(
            &options,
            WorkflowPhase::Inspecting,
            KeyCode::Char('c'),
            KeyModifiers::CONTROL,
        ),
        AccountOptionsKeyAction::ResetWorkflow,
        "inspection exit continues through the existing reset shutdown path"
    );
    options.tab = AccountOptionsTab::Resets;
    options.pending_tab = Some(AccountOptionsTab::Credits);
    assert_eq!(
        account_options_key_action(
            &options,
            WorkflowPhase::Confirming,
            KeyCode::Enter,
            KeyModifiers::empty(),
        ),
        AccountOptionsKeyAction::ResetWorkflow,
        "a protected reset confirmation remains owned by the reset workflow"
    );
    assert_eq!(
        account_options_footer(&options),
        "tab change pending  esc back  q/ctrl-c exit"
    );
}

#[test]
fn reset_inspection_footer_lists_account_options_and_review_keys() {
    let inspecting_footer =
        super::rendering::account_options_inspection_footer(WorkflowPhase::Inspecting, 100)
            .expect("Inspecting uses account-options navigation");
    assert!(inspecting_footer.contains("tab/←/→ credits"));
    assert!(inspecting_footer.contains("esc/ctrl-r back"));

    let inspected_footer =
        super::rendering::account_options_inspection_footer(WorkflowPhase::Inspected, 100)
            .expect("Inspected keeps reset review controls alongside account-options navigation");
    for marker in [
        "tab/←/→ credits",
        "enter review",
        "pgup/pgdn pages",
        "esc/ctrl-r back",
    ] {
        assert!(
            inspected_footer.contains(marker),
            "missing {marker} in {inspected_footer}"
        );
    }

    let narrow_footer =
        super::rendering::account_options_inspection_footer(WorkflowPhase::Inspected, 48)
            .expect("narrow Inspected footer keeps its primary actions");
    for marker in ["tab/←/→ credits", "enter review", "esc/ctrl-r back"] {
        assert!(
            narrow_footer.contains(marker),
            "missing {marker} in {narrow_footer}"
        );
    }

    for protected_phase in [
        WorkflowPhase::Confirming,
        WorkflowPhase::Revalidating,
        WorkflowPhase::Committing,
        WorkflowPhase::Result,
    ] {
        assert_eq!(
            super::rendering::account_options_inspection_footer(protected_phase, 100),
            None,
            "protected phase {protected_phase:?} retains its established reset footer"
        );
    }
}

#[tokio::test]
async fn ctrl_r_opens_disabled_credentialless_account_and_switches_to_credits() {
    let mut view_model = quota_two_account_view_model();
    view_model.rows[0].enabled = false;
    view_model.rows[0].active_credential_generation = None;
    let events = vec![
        (
            TerminalEvent::Key(control_key('r')),
            Some("Resets are unavailable"),
        ),
        (
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Tab)),
            Some("Credit usage"),
        ),
        (TerminalEvent::Key(control_key('c')), None),
    ];

    let frames = render_account_options_component_frames(view_model, 100, 36, events, None).await;

    assert!(
        frames
            .iter()
            .any(|frame| frame.contains("Resets are unavailable while this account is disabled."))
    );
    assert!(
        frames
            .iter()
            .any(|frame| frame.contains("tab credits  esc back"))
    );
    assert!(frames.iter().any(|frame| frame.contains("[ Credits ]")));
    assert!(frames.iter().any(|frame| frame.contains("Credit usage")));
}

#[tokio::test]
async fn short_quota_options_keeps_credit_policy_and_footer_visible() {
    for width in [100usize, 48] {
        let events = vec![
            (TerminalEvent::Key(control_key('r')), Some("Reset credits")),
            (
                TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Tab)),
                Some("Credit usage"),
            ),
            (
                TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Enter)),
                Some("enter save"),
            ),
            (TerminalEvent::Key(control_key('c')), None),
        ];

        let frames = render_account_options_component_frames(
            quota_two_account_view_model(),
            width,
            24,
            events,
            None,
        )
        .await;

        assert!(
            frames.iter().any(|frame| {
                frame.contains("Credit usage")
                    && frame.contains("Allow")
                    && frame.contains("Disallow")
                    && frame.contains("enter save")
                    && frame.contains("esc cancel")
            }),
            "a {width}x24 credits pane must keep the active setting and editor footer visible:\n{}",
            frames.join("\n--- frame ---\n")
        );
    }
}

#[tokio::test]
async fn account_options_panel_keeps_spare_height_outside_its_border() {
    let events = vec![
        (TerminalEvent::Key(control_key('r')), Some("Reset credits")),
        (
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Tab)),
            Some("Credit usage"),
        ),
        (TerminalEvent::Key(control_key('c')), None),
    ];
    let frames = render_account_options_component_frames(
        quota_two_account_view_model(),
        100,
        36,
        events,
        None,
    )
    .await;
    let frame = frames
        .iter()
        .find(|frame| frame.contains("[ Credits ]") && frame.contains("Credit usage"))
        .expect("Credits pane should render");
    let lines = frame.lines().collect::<Vec<_>>();
    let footer_index = lines
        .iter()
        .position(|line| line.contains("tab resets  enter edit  r refresh  esc back"))
        .expect("Credits pane footer should remain visible");
    let pane_bottom_index = lines
        .iter()
        .take(footer_index)
        .enumerate()
        .rev()
        .find(|(_index, line)| line.contains('└'))
        .map(|(index, _line)| index)
        .expect("Credits pane border should close before the footer");

    assert!(
        footer_index.saturating_sub(pane_bottom_index + 1) >= 3,
        "flexible empty space belongs outside the content-sized Credits pane:\n{frame}"
    );
}

#[tokio::test]
async fn short_credit_editor_keeps_save_error_with_floor_and_provider_notes_visible() {
    for width in [100usize, 48] {
        let mut view_model = quota_two_account_view_model();
        view_model.rows[0].weekly_quota_floor_percent = 95;
        view_model.rows[0].credit_usage = crate::quota::CreditUsageStatus {
            policy: CreditUsagePolicy::Disallow,
            provider_observation: codex_router_core::credit_usage::CreditProviderObservation::new(
                CreditAvailability::Unknown,
                codex_router_core::credit_usage::CreditSpendControl::Unknown,
                Some(codex_router_core::credit_usage::CreditProviderLimitReason::Unknown),
            ),
            freshness: CreditUsageFreshness::Stale,
            age_label: "very old synthetic sample".to_owned(),
        };
        let policy_saver: CreditUsagePolicySaver = Arc::new(|_account_id, _generation, _policy| {
            Box::pin(async { Err(CreditUsagePolicySaveError::StateOperationFailed) })
        });
        let events = vec![
            (TerminalEvent::Key(control_key('r')), Some("Reset credits")),
            (
                TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Tab)),
                Some("Credit usage"),
            ),
            (
                TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Enter)),
                Some("enter save"),
            ),
            (
                TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Enter)),
                Some("credit preference save failed"),
            ),
            (TerminalEvent::Key(control_key('c')), None),
        ];
        let frames = render_account_options_component_frames(
            view_model,
            width,
            24,
            events,
            Some(policy_saver),
        )
        .await;
        let failed_frame = frames
            .iter()
            .find(|frame| frame.contains("credit preference save failed"))
            .expect("save error should remain visible at short viewport height");

        for marker in [
            "Weekly floor",
            "blocks credit routing",
            "Provider spend-control",
            "esc cancel",
        ] {
            assert!(
                failed_frame.contains(marker),
                "missing {marker}:\n{failed_frame}"
            );
        }
        assert!(
            failed_frame
                .lines()
                .all(|line| line.chars().count() <= width),
            "a {width}x24 credit options frame must fit the enclosing terminal:\n{failed_frame}"
        );
    }
}

async fn render_account_options_component_frames(
    view_model: QuotaStatusViewModel,
    width: usize,
    height: usize,
    ordered_events: Vec<(TerminalEvent, Option<&'static str>)>,
    credit_policy_saver: Option<CreditUsagePolicySaver>,
) -> Vec<String> {
    let (acknowledged_events, acknowledgement_sender) = acknowledged_event_stream(ordered_events);
    element! {
        QuotaStatusComponent(view_model, width, height, credit_policy_saver)
    }
    .mock_terminal_render_loop(MockTerminalConfig::with_events(acknowledged_events))
    .map(|canvas| canvas.to_string())
    .inspect(move |frame| {
        for marker in [
            "Reset credits",
            "Resets are unavailable",
            "Credit usage",
            "enter save",
            "credit preference save failed",
        ] {
            if frame.contains(marker) {
                let _ = acknowledgement_sender.send(marker);
            }
        }
    })
    .collect::<Vec<_>>()
    .await
}

fn acknowledged_event_stream(
    ordered_events: Vec<(TerminalEvent, Option<&'static str>)>,
) -> (
    impl futures_util::Stream<Item = TerminalEvent>,
    tokio::sync::mpsc::UnboundedSender<&'static str>,
) {
    let (acknowledgement_sender, acknowledgement_receiver) = tokio::sync::mpsc::unbounded_channel();
    let events = futures_util::stream::unfold(
        (ordered_events.into_iter(), acknowledgement_receiver, None),
        |(mut pending_events, mut acknowledgement_receiver, expected_frame)| async move {
            if let Some(expected_frame) = expected_frame {
                loop {
                    match acknowledgement_receiver.recv().await {
                        Some(acknowledgement) if acknowledgement == expected_frame => break,
                        Some(_) => {}
                        None => panic!(
                            "rendered-frame acknowledgement channel closed before marker {expected_frame:?}"
                        ),
                    }
                }
            }
            let (event, next_expected_frame) = pending_events.next()?;
            Some((
                event,
                (
                    pending_events,
                    acknowledgement_receiver,
                    next_expected_frame,
                ),
            ))
        },
    );
    (events, acknowledgement_sender)
}

#[tokio::test]
#[should_panic(expected = "rendered-frame acknowledgement channel closed before marker")]
async fn acknowledged_event_stream_fails_when_expected_frame_is_missing() {
    let (events, acknowledgement_sender) = acknowledged_event_stream(vec![(
        TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Enter)),
        Some("Credit usage"),
    )]);
    drop(acknowledgement_sender);
    let mut events = Box::pin(events);

    let _ = events.next().await;
    let _ = events.next().await;
}

async fn assert_credit_control_c_exits_the_component(editing: bool) {
    let mut ordered_events = vec![
        (TerminalEvent::Key(control_key('r')), Some("Reset credits")),
        (
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Tab)),
            Some("Credit usage"),
        ),
    ];
    if editing {
        ordered_events.push((
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Enter)),
            Some("enter save"),
        ));
    }
    ordered_events.push((
        TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Char('\u{3}'))),
        Some(if editing {
            "enter save"
        } else {
            "Credit usage"
        }),
    ));

    let (acknowledged_events, acknowledgement_sender) = acknowledged_event_stream(ordered_events);
    let events = acknowledged_events.chain(futures_util::stream::pending());
    let view_model = quota_two_account_view_model();
    let width = 100usize;
    let height = 36usize;
    let credit_policy_saver = None;
    let mut terminal = element! {
        QuotaStatusComponent(view_model, width, height, credit_policy_saver)
    };
    let component = terminal
        .mock_terminal_render_loop(MockTerminalConfig::with_events(events))
        .map(|canvas| canvas.to_string())
        .inspect(move |frame| {
            for marker in ["Reset credits", "Credit usage", "enter save"] {
                if frame.contains(marker) {
                    let _ = acknowledgement_sender.send(marker);
                }
            }
        })
        .collect::<Vec<_>>();

    let frames = tokio::time::timeout(Duration::from_secs(2), component)
        .await
        .expect("Ctrl-C in Credits should return control to the terminal host");
    assert!(
        frames.iter().any(|frame| frame.contains("[ Credits ]")),
        "the component should enter Credits before handling Ctrl-C"
    );
}

fn control_key(character: char) -> KeyEvent {
    let mut event = KeyEvent::new(KeyEventKind::Press, KeyCode::Char(character));
    event.modifiers = KeyModifiers::CONTROL;
    event
}

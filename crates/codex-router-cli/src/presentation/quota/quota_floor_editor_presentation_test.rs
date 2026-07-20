use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use futures_util::StreamExt;
use iocraft::prelude::*;

use super::quota_browse_presentation_test::quota_two_account_view_model;
use super::quota_floor_editor::WeeklyFloorEditorPhase;
use super::quota_floor_editor::WeeklyFloorEditorState;
use super::quota_floor_editor::WeeklyFloorStep;
use super::quota_floor_editor::WeeklyQuotaFloorSaveError;
use super::quota_floor_editor::WeeklyQuotaFloorSaver;
use super::quota_floor_editor::render_weekly_floor_editor;
use super::quota_floor_editor::step_weekly_floor_percent;
use super::quota_floor_editor::weekly_floor_editor_key_command;
use super::quota_status_component::QuotaStatusComponent;
use super::quota_status_component::forced_quota_exit_key;
use super::quota_status_view_model::QuotaStatusViewModelLoader;

#[test]
fn weekly_floor_stepper_clamps_to_zero_and_fifteen() {
    assert_eq!(step_weekly_floor_percent(0, WeeklyFloorStep::Decrease), 0);
    assert_eq!(step_weekly_floor_percent(14, WeeklyFloorStep::Increase), 15);
    assert_eq!(step_weekly_floor_percent(15, WeeklyFloorStep::Increase), 15);
}

#[test]
fn floor_editor_preserves_component_owned_ctrl_c_and_ctrl_d_exit_keys() {
    assert!(forced_quota_exit_key(
        &KeyCode::Char('c'),
        KeyModifiers::CONTROL
    ));
    assert!(forced_quota_exit_key(
        &KeyCode::Char('d'),
        KeyModifiers::CONTROL
    ));
    assert!(!forced_quota_exit_key(
        &KeyCode::Char('e'),
        KeyModifiers::CONTROL
    ));
    assert!(!forced_quota_exit_key(&KeyCode::Esc, KeyModifiers::NONE));
}

#[test]
fn committed_save_cannot_be_cancelled_while_refresh_is_unresolved() {
    let mut state = Some(WeeklyFloorEditorState::new(
        codex_router_core::ids::AccountId::new("committed-floor")
            .expect("test account should be valid"),
        "committed".to_owned(),
        11,
    ));
    state.as_mut().expect("editor should exist").phase = WeeklyFloorEditorPhase::SavedRefreshFailed;

    assert!(
        weekly_floor_editor_key_command(&mut state, KeyCode::Esc, KeyModifiers::NONE).is_none()
    );
    assert!(state.is_some());
    assert!(
        weekly_floor_editor_key_command(&mut state, KeyCode::Char('e'), KeyModifiers::CONTROL)
            .is_none()
    );
    assert!(state.is_some());
}

#[tokio::test]
async fn ctrl_e_saves_the_focused_stable_account_and_reloads_immediately() {
    let mut view_model = quota_two_account_view_model();
    view_model.rows[0].account = "duplicate".to_owned();
    view_model.rows[1].account = "duplicate".to_owned();
    view_model.rows[1].weekly_quota_floor_percent = 10;
    let expected_account_id = view_model.rows[1].account_id.clone();
    let mut reloaded_view_model = view_model.clone();
    reloaded_view_model.rows[1].weekly_quota_floor_percent = 11;
    let recorded_saves = Arc::new(Mutex::new(Vec::new()));
    let saver: WeeklyQuotaFloorSaver = {
        let recorded_saves = Arc::clone(&recorded_saves);
        Arc::new(move |account_id, percent| {
            recorded_saves
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((account_id, percent));
            Box::pin(async { Ok(()) })
        })
    };
    let loader: QuotaStatusViewModelLoader = Arc::new(move || {
        let reloaded_view_model = reloaded_view_model.clone();
        Box::pin(async move { Some(reloaded_view_model) })
    });
    let events = vec![
        TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Down)),
        TerminalEvent::Key(control_key('e')),
        TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Right)),
        TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Enter)),
        TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Esc)),
    ];

    let frames = element! {
        QuotaStatusComponent(
            view_model,
            width: 120usize,
            height: 48usize,
            reload_view_model: Some(loader),
            weekly_floor_saver: Some(saver),
        )
    }
    .mock_terminal_render_loop(MockTerminalConfig::with_events(
        futures_util::stream::iter(events).then(|event| async move {
            tokio::time::sleep(Duration::from_millis(5)).await;
            event
        }),
    ))
    .map(|canvas| canvas.to_string())
    .collect::<Vec<_>>()
    .await;

    assert_eq!(
        *recorded_saves
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        vec![(expected_account_id, 11)]
    );
    assert!(frames.iter().any(|frame| frame.contains("Weekly floor")));
    assert!(
        frames
            .last()
            .is_some_and(|frame| frame.contains("ctrl-e edit floor"))
    );
}

#[tokio::test]
async fn esc_and_ctrl_e_cancel_without_saving() {
    for cancel_event in [
        TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Esc)),
        TerminalEvent::Key(control_key('e')),
    ] {
        let recorded_saves = Arc::new(Mutex::new(Vec::new()));
        let saver: WeeklyQuotaFloorSaver = {
            let recorded_saves = Arc::clone(&recorded_saves);
            Arc::new(move |account_id, percent| {
                recorded_saves
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push((account_id, percent));
                Box::pin(async { Ok(()) })
            })
        };
        let events = vec![
            TerminalEvent::Key(control_key('e')),
            cancel_event,
            TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Esc)),
        ];

        let _frames = element! {
            QuotaStatusComponent(
                view_model: quota_two_account_view_model(),
                width: 120usize,
                height: 48usize,
                weekly_floor_saver: Some(saver),
            )
        }
        .mock_terminal_render_loop(MockTerminalConfig::with_events(futures_util::stream::iter(
            events,
        )))
        .collect::<Vec<_>>()
        .await;

        assert!(
            recorded_saves
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty()
        );
    }
}

#[tokio::test]
async fn committed_save_with_failed_reload_stays_truthful_and_does_not_save_twice() {
    let save_count = Arc::new(Mutex::new(0usize));
    let saver: WeeklyQuotaFloorSaver = {
        let save_count = Arc::clone(&save_count);
        Arc::new(move |_account_id, _percent| {
            *save_count
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) += 1;
            Box::pin(async { Ok(()) })
        })
    };
    let loader: QuotaStatusViewModelLoader = Arc::new(|| Box::pin(async { None }));
    let events = vec![
        TerminalEvent::Key(control_key('e')),
        TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Right)),
        TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Enter)),
        TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Enter)),
        TerminalEvent::Key(control_key('c')),
    ];

    let frames = element! {
        QuotaStatusComponent(
            view_model: quota_two_account_view_model(),
            width: 120usize,
            height: 48usize,
            reload_view_model: Some(loader),
            weekly_floor_saver: Some(saver),
        )
    }
    .mock_terminal_render_loop(MockTerminalConfig::with_events(
        futures_util::stream::iter(events).then(|event| async move {
            tokio::time::sleep(Duration::from_millis(5)).await;
            event
        }),
    ))
    .map(|canvas| canvas.to_string())
    .collect::<Vec<_>>()
    .await;

    assert_eq!(
        *save_count
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        1
    );
    assert!(
        frames
            .iter()
            .any(|frame| frame.contains("saved; refresh failed"))
    );
    assert!(
        frames
            .iter()
            .rev()
            .any(|frame| frame.contains("current") && frame.contains("1%"))
    );
}

#[tokio::test]
async fn mutation_failure_remains_visible_and_enter_retries_without_losing_the_draft() {
    let save_count = Arc::new(Mutex::new(0usize));
    let saver: WeeklyQuotaFloorSaver = {
        let save_count = Arc::clone(&save_count);
        Arc::new(move |_account_id, _percent| {
            *save_count
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) += 1;
            Box::pin(async { Err(WeeklyQuotaFloorSaveError::DatabaseBusy) })
        })
    };
    let events = vec![
        TerminalEvent::Key(control_key('e')),
        TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Right)),
        TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Enter)),
        TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Enter)),
        TerminalEvent::Key(control_key('e')),
        TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Esc)),
    ];

    let frames = element! {
        QuotaStatusComponent(
            view_model: quota_two_account_view_model(),
            width: 120usize,
            height: 48usize,
            weekly_floor_saver: Some(saver),
        )
    }
    .mock_terminal_render_loop(MockTerminalConfig::with_events(
        futures_util::stream::iter(events).then(|event| async move {
            tokio::time::sleep(Duration::from_millis(5)).await;
            event
        }),
    ))
    .map(|canvas| canvas.to_string())
    .collect::<Vec<_>>()
    .await;

    assert_eq!(
        *save_count
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        2
    );
    assert!(
        frames
            .iter()
            .any(|frame| frame.contains("database busy; retry save"))
    );
    assert!(frames.iter().any(|frame| frame.contains("1%")));
}

#[component]
fn WeeklyFloorMouseHarness(mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
    let mut system = hooks.use_context_mut::<SystemContext>();
    let mut should_exit = hooks.use_state(|| false);
    let editor_state = hooks.use_state(|| {
        Some(WeeklyFloorEditorState::new(
            codex_router_core::ids::AccountId::new("mouse-account")
                .expect("test account should be valid"),
            "mouse".to_owned(),
            10,
        ))
    });
    hooks.use_terminal_events(move |event| {
        if matches!(
            event,
            TerminalEvent::Key(KeyEvent {
                code: KeyCode::Char('q'),
                ..
            })
        ) {
            should_exit.set(true);
        }
    });
    if should_exit.get() {
        system.exit();
    }
    let editor = editor_state
        .read()
        .clone()
        .expect("editor should remain available");
    render_weekly_floor_editor(&editor, 48, 10, editor_state)
}

#[tokio::test]
async fn real_iocraft_arrow_button_handles_mouse_click() {
    let events = vec![
        TerminalEvent::FullscreenMouse(FullscreenMouseEvent::new(
            MouseEventKind::Down(crossterm::event::MouseButton::Left),
            12,
            5,
        )),
        TerminalEvent::Key(KeyEvent::new(KeyEventKind::Press, KeyCode::Char('q'))),
    ];

    let frames = element!(WeeklyFloorMouseHarness)
        .mock_terminal_render_loop(MockTerminalConfig::with_events(futures_util::stream::iter(
            events,
        )))
        .map(|canvas| canvas.to_string())
        .collect::<Vec<_>>()
        .await;

    assert!(frames.iter().any(|frame| frame.contains("  9%")));
}

fn control_key(character: char) -> KeyEvent {
    let mut event = KeyEvent::new(KeyEventKind::Press, KeyCode::Char(character));
    event.modifiers = KeyModifiers::CONTROL;
    event
}

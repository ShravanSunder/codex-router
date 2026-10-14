use std::sync::Arc;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use super::quota_dialog_actions::*;
use super::quota_help_rendering::*;
use codex_router_tui::DialogHost;
use iocraft::prelude::*;
use tokio::sync::watch;

use crate::quota_reset::reset_session_supervisor::PinnedTargetInvalidationReason;
use crate::quota_reset::reset_session_supervisor::ResetIntentSender;
use crate::quota_reset::reset_session_supervisor::ResetSessionIntent;
use crate::quota_reset::reset_session_supervisor::ResetWorkflowSnapshot;
use crate::quota_reset::reset_session_supervisor::WorkflowPhase;

use super::quota_account_options::*;
use super::quota_browse_rendering::*;
use super::quota_detail_panel::*;
use super::quota_dialog_rendering::*;
use super::quota_floor_editor::*;
use super::quota_provider_filter_rendering::*;
use super::quota_provider_filters::*;
use super::quota_reset_component_input::*;
use super::quota_reset_detail_rendering::*;
use super::quota_reset_presentation_model::*;
use super::quota_status_view_model::*;
use super::responsive_quota_layout::*;

pub(super) const MIN_QUOTA_WIDTH: usize = 48;
pub(super) const MIN_RENDER_HEIGHT: usize = 24;
pub(super) const SIDECAR_QUOTA_WIDTH: usize = 160;
pub(super) const NARROW_QUOTA_WIDTH: usize = MIN_QUOTA_WIDTH;
const LIVE_QUOTA_WIDTH_POLL_INTERVAL: Duration = Duration::from_millis(80);
pub(super) const LIVE_QUOTA_STATUS_RELOAD_INTERVAL: Duration = Duration::from_secs(60);
pub(super) const LIVE_QUOTA_STATUS_SPINNER_INTERVAL: Duration = Duration::from_millis(120);
pub(super) const QUOTA_STATUS_SPINNER_TICKS: &[&str] =
    &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

#[derive(Default, Props)]
pub(super) struct QuotaStatusComponentProps {
    pub(super) interaction_mode: QuotaInteractionMode,
    pub(super) view_model: QuotaStatusViewModel,
    pub(super) width: usize,
    pub(super) height: usize,
    pub(super) reload_view_model: Option<QuotaStatusViewModelLoader>,
    pub(super) reload_interval: Duration,
    pub(super) spinner_interval: Duration,
    pub(super) reset_intent_sender: Option<ResetIntentSender>,
    pub(super) reset_snapshot_receiver: Option<watch::Receiver<ResetWorkflowSnapshot>>,
    pub(super) weekly_floor_saver: Option<WeeklyQuotaFloorSaver>,
    pub(super) credit_policy_saver: Option<CreditUsagePolicySaver>,
    pub(super) credit_usage_refresher: Option<CreditUsageRefresher>,
}

#[component]
pub(super) fn QuotaStatusComponent(
    props: &mut QuotaStatusComponentProps,
    mut hooks: Hooks,
) -> impl Into<AnyElement<'static>> {
    let mut system = hooks.use_context_mut::<SystemContext>();
    let (terminal_width, terminal_height) = hooks.use_terminal_size();
    let live_terminal_width = props.width == 0;
    let live_terminal_height = props.height == 0 && live_terminal_width;
    let view_model = hooks.use_state(|| props.view_model.clone());
    let filters = hooks.use_state(QuotaFilterState::default);
    let mut help_open = hooks.use_state(|| false);
    let mut details_dialog = hooks.use_state(|| None::<codex_router_core::ids::AccountId>);
    let interaction_mode = props.interaction_mode;
    let weekly_floor_editing_enabled = props.weekly_floor_saver.is_some();
    let initial_focused_account_id = props
        .view_model
        .rows
        .iter()
        .find(|row| row.selected)
        .or_else(|| props.view_model.rows.first())
        .map(|row| row.account_id.clone());
    let observed_width = hooks.use_state(|| {
        if live_terminal_width {
            current_terminal_width()
                .or_else(|| Some(usize::from(terminal_width)))
                .unwrap_or(props.view_model.width)
                .max(MIN_QUOTA_WIDTH)
        } else {
            props.width.max(MIN_QUOTA_WIDTH)
        }
    });
    let observed_height = hooks.use_state(|| {
        if live_terminal_height {
            usize::from(terminal_height).max(MIN_RENDER_HEIGHT)
        } else if props.height == 0 {
            MIN_RENDER_HEIGHT
        } else {
            props.height.max(1)
        }
    });
    let focused_account_id = hooks.use_state(|| initial_focused_account_id);
    let filter_controller = QuotaFilterController {
        state: filters,
        view_model,
        focused_account_id,
    };
    let reset_snapshot = hooks.use_state(|| {
        props
            .reset_snapshot_receiver
            .as_ref()
            .map(|receiver| receiver.borrow().clone())
    });
    let reset_target = hooks.use_state(|| None::<ResetPaneTarget>);
    let account_options = hooks.use_state(|| None::<AccountOptionsState>);
    let account_options_session_generation = hooks.use_state(|| 0_u64);
    let account_options_command_port = hooks.use_memo(AccountOptionsCommandPort::new, ());
    let weekly_floor_editor = hooks.use_state(|| None::<WeeklyFloorEditorState>);
    let weekly_floor_command_port = hooks.use_memo(WeeklyFloorEditorCommandPort::new, ());
    let reload_lock = hooks.use_memo(|| Arc::new(tokio::sync::Mutex::new(())), ());
    let inventory_page_start = hooks.use_state(|| 0usize);
    let spinner_tick = hooks.use_state(|| 0usize);
    let mut should_exit = hooks.use_state(|| false);
    hooks.use_terminal_events({
        let mut observed_width = observed_width;
        let mut observed_height = observed_height;
        let mut focused_account_id = focused_account_id;
        let mut reset_target = reset_target;
        let mut account_options = account_options;
        let mut account_options_session_generation = account_options_session_generation;
        let mut inventory_page_start = inventory_page_start;
        let mut weekly_floor_editor = weekly_floor_editor;
        let mut details_dialog = details_dialog;
        let weekly_floor_command_port = weekly_floor_command_port.clone();
        let account_options_command_port = account_options_command_port.clone();
        let reset_intent_sender = props.reset_intent_sender.clone();
        let current_reset_snapshot = reset_snapshot.read().clone().or_else(|| {
            props
                .reset_snapshot_receiver
                .as_ref()
                .map(|receiver| receiver.borrow().clone())
        });
        move |event| match event {
            TerminalEvent::Resize(width, height) => {
                if live_terminal_width {
                    let mut width_value = observed_width.get();
                    if apply_live_terminal_width_sample(&mut width_value, Some(usize::from(width)))
                    {
                        observed_width.set(width_value);
                    }
                }
                if live_terminal_height {
                    observed_height.set(usize::from(height).max(MIN_RENDER_HEIGHT));
                }
            }
            TerminalEvent::Key(KeyEvent {
                code,
                kind,
                modifiers,
                ..
            }) => {
                if kind == KeyEventKind::Release {
                    return;
                }
                if interaction_mode == QuotaInteractionMode::Interactive {
                    if help_open.get() {
                        if code == KeyCode::Esc || quota_help_key(code, modifiers) {
                            help_open.set(false);
                            return;
                        }
                        // Preserve reset/floor/options forced-exit ownership below.
                        if !forced_quota_exit_key(&code, modifiers) {
                            return;
                        }
                    }
                    if quota_help_key(code, modifiers) {
                        help_open.set(true);
                        return;
                    }
                }
                let current_reset_snapshot = reset_snapshot
                    .read()
                    .clone()
                    .or_else(|| current_reset_snapshot.clone());
                let rows_for_navigation = filters
                    .read()
                    .committed
                    .visible_rows(&view_model.read().rows);
                let row_count = rows_for_navigation.len();
                if filters.read().committed != QuotaAccountFilters::default() {
                    reconcile_quota_filter_focus(&rows_for_navigation, &mut focused_account_id);
                }
                let phase = current_reset_snapshot
                    .as_ref()
                    .map_or(WorkflowPhase::Browse, ResetWorkflowSnapshot::phase);
                if phase == WorkflowPhase::Browse
                    && weekly_floor_editor.read().is_some()
                    && forced_quota_exit_key(&code, modifiers)
                {
                    should_exit.set(true);
                    return;
                }
                if phase == WorkflowPhase::Browse && weekly_floor_editor.read().is_some() {
                    let mut next = weekly_floor_editor.read().clone();
                    let command = weekly_floor_editor_key_command(&mut next, code, modifiers);
                    weekly_floor_editor.set(next);
                    if let Some(command) = command
                        && !weekly_floor_command_port.send(command)
                    {
                        let mut next = weekly_floor_editor.read().clone();
                        weekly_floor_editor_send_failed(&mut next);
                        weekly_floor_editor.set(next);
                    }
                    return;
                }
                if handle_account_options_key_event(
                    &mut account_options,
                    &mut reset_target,
                    AccountOptionsKeyEventContext {
                        phase,
                        code,
                        modifiers,
                        command_port: &account_options_command_port,
                        reset_snapshot: current_reset_snapshot.as_ref(),
                        reset_intent_sender: reset_intent_sender.as_ref(),
                        inventory_page_start: &mut inventory_page_start,
                        now_unix_seconds: current_unix_seconds(),
                    },
                ) {
                    return;
                }
                if phase != WorkflowPhase::Browse {
                    handle_quota_reset_key(QuotaResetKeyContext {
                        phase,
                        code,
                        modifiers,
                        current_reset_snapshot: current_reset_snapshot.as_ref(),
                        reset_intent_sender: reset_intent_sender.as_ref(),
                        reset_target,
                        inventory_page_start,
                        observed_width,
                        observed_height,
                        account_options,
                        weekly_floor_editor,
                        rows_for_navigation: &rows_for_navigation,
                        focused_account_id,
                        should_exit,
                        interaction_mode,
                        filters,
                    });
                    return;
                }
                if forced_quota_exit_key(&code, modifiers) {
                    should_exit.set(true);
                    return;
                }
                if filter_controller.handle_key(code, modifiers) {
                    return;
                }
                if details_dialog.read().is_some() {
                    if code == KeyCode::Esc {
                        details_dialog.set(None);
                    }
                    return;
                }
                if code == KeyCode::F(4) && interaction_mode == QuotaInteractionMode::Interactive {
                    details_dialog.set(focused_account_id.read().clone());
                    return;
                }
                if interaction_mode == QuotaInteractionMode::Interactive
                    && opens_quota_filters(code, modifiers)
                    && !quota_filter_modal_owns_input(
                        account_options.read().as_ref(),
                        weekly_floor_editor.read().as_ref(),
                        current_reset_snapshot.as_ref(),
                        reset_target.read().as_ref(),
                    )
                {
                    filter_controller.dispatch(QuotaFilterAction::Open);
                    return;
                }
                match code {
                    KeyCode::Up => {
                        let current_index = focused_row_index_for_account(
                            &rows_for_navigation,
                            focused_account_id.read().as_ref(),
                        );
                        if let Some(row) = moved_quota_focus_index(
                            current_index,
                            row_count,
                            QuotaFocusMove::Previous,
                        )
                        .and_then(|index| rows_for_navigation.get(index))
                        {
                            focused_account_id.set(Some(row.account_id.clone()));
                        }
                    }
                    KeyCode::Down => {
                        let current_index = focused_row_index_for_account(
                            &rows_for_navigation,
                            focused_account_id.read().as_ref(),
                        );
                        if let Some(row) =
                            moved_quota_focus_index(current_index, row_count, QuotaFocusMove::Next)
                                .and_then(|index| rows_for_navigation.get(index))
                        {
                            focused_account_id.set(Some(row.account_id.clone()));
                        }
                    }
                    KeyCode::Char('r') if modifiers.contains(KeyModifiers::CONTROL) => {
                        let current_index = focused_row_index_for_account(
                            &rows_for_navigation,
                            focused_account_id.read().as_ref(),
                        );
                        if let Some(row) =
                            current_index.and_then(|index| rows_for_navigation.get(index))
                        {
                            open_account_options_for_account(
                                &mut account_options,
                                &mut account_options_session_generation,
                                row,
                                &mut reset_target,
                                reset_intent_sender.as_ref(),
                                current_unix_seconds(),
                            );
                            inventory_page_start.set(0);
                        }
                    }
                    KeyCode::Char('e') if modifiers.contains(KeyModifiers::CONTROL) => {
                        let current_index = focused_row_index_for_account(
                            &rows_for_navigation,
                            focused_account_id.read().as_ref(),
                        );
                        if weekly_floor_editing_enabled
                            && reset_target.read().is_none()
                            && let Some(row) =
                                current_index.and_then(|index| rows_for_navigation.get(index))
                        {
                            weekly_floor_editor.set(Some(WeeklyFloorEditorState::new(
                                row.account_id.clone(),
                                row.account.clone(),
                                row.weekly_quota_floor_percent,
                            )));
                        }
                    }
                    KeyCode::Esc | KeyCode::Char('q') => should_exit.set(true),
                    KeyCode::Char('c' | 'd') if modifiers.contains(KeyModifiers::CONTROL) => {
                        should_exit.set(true);
                    }
                    KeyCode::Char('\u{3}' | '\u{4}') => should_exit.set(true),
                    _ => {}
                }
            }
            _ => {}
        }
    });
    hooks.use_future({
        let weekly_floor_saver = props.weekly_floor_saver.clone();
        let reload_view_model = props.reload_view_model.clone();
        let reload_lock = reload_lock.clone();
        let receiver = weekly_floor_command_port.take_receiver();
        async move {
            let Some(receiver) = receiver else {
                return;
            };
            run_weekly_floor_editor_commands(
                receiver,
                weekly_floor_saver,
                reload_view_model,
                reload_lock,
                view_model,
                weekly_floor_editor,
            )
            .await;
        }
    });
    hooks.use_future({
        let policy_saver = props.credit_policy_saver.clone();
        let refresher = props.credit_usage_refresher.clone();
        let loader = props.reload_view_model.clone();
        let reload_lock = reload_lock.clone();
        let reset_intent_sender = props.reset_intent_sender.clone();
        let reset_snapshot_receiver = props.reset_snapshot_receiver.clone();
        let receiver = account_options_command_port.take_receiver();
        async move {
            let Some(receiver) = receiver else {
                return;
            };
            run_account_options_commands(AccountOptionsCommandContext {
                receiver,
                policy_saver,
                refresher,
                loader,
                reload_lock,
                view_model,
                account_options,
                reset_snapshot,
                reset_snapshot_receiver,
                reset_target,
                inventory_page_start,
                reset_intent_sender,
            })
            .await;
        }
    });
    hooks.use_future({
        let mut view_model = view_model;
        let mut account_options = account_options;
        let mut reset_target = reset_target;
        let reset_intent_sender = props.reset_intent_sender.clone();
        let reload_view_model = props.reload_view_model.clone();
        let reload_interval = if props.reload_interval.is_zero() {
            LIVE_QUOTA_STATUS_RELOAD_INTERVAL
        } else {
            props.reload_interval
        };
        async move {
            let Some(reload_view_model) = reload_view_model else {
                return;
            };
            let mut interval = tokio::time::interval(reload_interval);
            interval.tick().await;
            loop {
                interval.tick().await;
                let _reload_guard = reload_lock.lock().await;
                if let Some(next_view_model) = reload_view_model().await {
                    super::quota_account_options::install_reloaded_view_model(
                        next_view_model,
                        &mut view_model,
                        &mut account_options,
                        &reset_snapshot,
                        &mut reset_target,
                        reset_intent_sender.as_ref(),
                    );
                }
            }
        }
    });
    hooks.use_future({
        let mut reset_snapshot = reset_snapshot;
        let reset_snapshot_receiver = props.reset_snapshot_receiver.clone();
        async move {
            let Some(mut receiver) = reset_snapshot_receiver else {
                return;
            };
            reset_snapshot.set(Some(receiver.borrow_and_update().clone()));
            while receiver.changed().await.is_ok() {
                reset_snapshot.set(Some(receiver.borrow_and_update().clone()));
            }
        }
    });
    hooks.use_future({
        let mut spinner_tick = spinner_tick;
        let spinner_interval = if props.spinner_interval.is_zero() {
            LIVE_QUOTA_STATUS_SPINNER_INTERVAL
        } else {
            props.spinner_interval
        };
        async move {
            let mut interval = tokio::time::interval(spinner_interval);
            interval.tick().await;
            loop {
                interval.tick().await;
                spinner_tick += 1;
            }
        }
    });
    hooks.use_future({
        let mut observed_width = observed_width;
        async move {
            if !live_terminal_width {
                return;
            }
            let mut interval = tokio::time::interval(LIVE_QUOTA_WIDTH_POLL_INTERVAL);
            interval.tick().await;
            loop {
                interval.tick().await;
                let mut width_value = observed_width.get();
                if apply_live_terminal_width_sample(&mut width_value, current_terminal_width()) {
                    observed_width.set(width_value);
                }
            }
        }
    });
    if *should_exit.read() {
        system.exit();
    }
    let width = if live_terminal_width {
        observed_width.get()
    } else {
        props.width
    }
    .max(MIN_QUOTA_WIDTH);
    let height = if live_terminal_height {
        observed_height.get()
    } else if props.height == 0 {
        MIN_RENDER_HEIGHT
    } else {
        props.height.max(1)
    };
    let content_width = width.saturating_sub(4).max(44);
    let view_model = view_model.read();
    let filter_state = *filters.read();
    let visible_rows = filter_state.committed.visible_rows(&view_model.rows);
    let row_count = visible_rows.len();
    if filter_state.committed != QuotaAccountFilters::default() {
        let mut focused_account_id = focused_account_id;
        reconcile_quota_filter_focus(&visible_rows, &mut focused_account_id);
    }
    let focused_row_index_value =
        focused_row_index_for_account(&visible_rows, focused_account_id.read().as_ref());
    let focused_details =
        focused_row_index_value.and_then(|index| visible_rows.get(index).map(|row| &row.details));
    let current_reset_snapshot = reset_snapshot.read().clone().or_else(|| {
        props
            .reset_snapshot_receiver
            .as_ref()
            .map(|receiver| receiver.borrow().clone())
    });
    let current_reset_target = reset_target.read().clone();
    let current_account_options = account_options.read().clone();
    let current_weekly_floor_editor = weekly_floor_editor.read().clone();
    if let (Some(snapshot), Some(target), Some(sender)) = (
        current_reset_snapshot.as_ref(),
        current_reset_target.as_ref(),
        props.reset_intent_sender.as_ref(),
    ) && reset_mode(Some(snapshot))
    {
        let invalidation = view_model
            .rows
            .iter()
            .find(|row| row.account_id == target.account_id)
            .map_or(
                Some(PinnedTargetInvalidationReason::AccountRemoved),
                |row| {
                    if !row.enabled {
                        Some(PinnedTargetInvalidationReason::AccountDisabled)
                    } else {
                        (row.active_credential_generation
                            != Some(target.active_credential_generation))
                        .then_some(PinnedTargetInvalidationReason::CredentialGenerationChanged)
                    }
                },
            );
        if let Some(reason) = invalidation {
            try_send_reset_intent(
                Some(sender),
                ResetSessionIntent::PinnedTargetInvalidated {
                    account_id: target.account_id.clone(),
                    active_credential_generation: target.active_credential_generation,
                    reason,
                },
            );
        }
    }
    let filter_modal_owns_input = quota_filter_modal_owns_input(
        current_account_options.as_ref(),
        current_weekly_floor_editor.as_ref(),
        current_reset_snapshot.as_ref(),
        current_reset_target.as_ref(),
    );
    let body_budget = quota_body_budget(height)
        .saturating_add(usize::from(
            interaction_mode == QuotaInteractionMode::Interactive
                && !(width >= SIDECAR_QUOTA_WIDTH && filter_modal_owns_input),
        ))
        .saturating_sub(quota_filter_controls_height(
            interaction_mode,
            content_width,
            filter_state.draft.is_some(),
            filter_modal_owns_input,
        ));
    let reset_detail_active = current_reset_snapshot
        .as_ref()
        .is_some_and(|snapshot| reset_mode(Some(snapshot)));
    let details_content_height = current_weekly_floor_editor.as_ref().map_or_else(
        || {
            current_account_options.as_ref().map_or_else(
                || {
                    current_reset_snapshot
                        .as_ref()
                        .filter(|_| reset_detail_active)
                        .and_then(|snapshot| {
                            current_reset_target.as_ref().map(|target| {
                                reset_panel_content_height(
                                    snapshot,
                                    target,
                                    inventory_page_start.get(),
                                )
                            })
                        })
                        .unwrap_or_else(|| selected_detail_height(focused_details.is_some()))
                },
                |options| {
                    account_options_content_height(
                        options,
                        current_reset_snapshot.as_ref(),
                        current_reset_target.as_ref(),
                        inventory_page_start.get(),
                    )
                },
            )
        },
        |_| weekly_floor_editor_content_height(),
    );
    let sidecar = width >= SIDECAR_QUOTA_WIDTH;
    let stacked_details = interaction_mode == QuotaInteractionMode::Static
        && !sidecar
        && (current_account_options.is_some()
            || (width >= NARROW_QUOTA_WIDTH
                && (focused_details.is_some() || props.view_model.selected.is_none())));
    let layout_mode = if sidecar {
        QuotaBodyLayoutMode::Sidecar
    } else if stacked_details {
        QuotaBodyLayoutMode::Stacked {
            fill_available_height: !reset_detail_active
                && current_weekly_floor_editor.is_none()
                && current_account_options.is_none(),
            prioritize_details: current_account_options.is_some(),
        }
    } else {
        QuotaBodyLayoutMode::Inline
    };
    let layout = quota_body_layout(
        body_budget,
        layout_mode,
        row_count,
        focused_row_index_value,
        details_content_height,
    );
    let details_height = layout.details_height;
    let visible_account_budget = layout.visible_account_budget;
    let list_height = layout.list_height;
    let stacked_details_height = layout.stacked_details_height;
    let show_stacked_details = layout.show_stacked_details;
    let body_height = layout.body_height;
    let component_height = quota_status_height(height);
    let account_pointer_interaction_enabled = !reset_detail_active
        && current_weekly_floor_editor.is_none()
        && current_account_options.is_none()
        && details_dialog.read().is_none()
        && filter_state.draft.is_none();
    let empty_notice = (interaction_mode == QuotaInteractionMode::Interactive
        && visible_rows.is_empty())
    .then_some("No accounts match these filters");
    let stacked_list = if current_account_options.is_some() && visible_account_budget == 0 {
        element! { View(height: 0) {} }.into_any()
    } else {
        render_account_list(QuotaAccountListProps {
            rows: &visible_rows,
            width: content_width,
            height: list_height,
            focused_row_index: focused_row_index_value,
            visible_rows: visible_account_budget,
            focused_account_id,
            interaction_enabled: account_pointer_interaction_enabled,
            empty_notice,
        })
    };
    let body = if sidecar {
        let list_width = (content_width.saturating_sub(2) * 3 / 5)
            .max(58)
            .min(content_width.saturating_sub(44));
        let details_width = content_width.saturating_sub(list_width + 2).max(34);
        element! {
            View(width: 100pct, height: body_height as u32) {
                #(render_account_list(QuotaAccountListProps {
                    rows: &visible_rows,
                    width: list_width,
                    height: list_height,
                    focused_row_index: focused_row_index_value,
                    visible_rows: visible_account_budget,
                    focused_account_id,
                    interaction_enabled: account_pointer_interaction_enabled,
                    empty_notice,
                }))
                View(width: 2) { Text(content: "") }
                #(render_detail_panel(QuotaDetailPanelProps {
                    interactive: interaction_mode == QuotaInteractionMode::Interactive,
                    focused_details,
                    account_options: current_account_options.as_ref(),
                    reset_snapshot: current_reset_snapshot.as_ref(),
                    reset_target: current_reset_target.as_ref(),
                    weekly_floor_editor: current_weekly_floor_editor.as_ref(),
                    weekly_floor_editor_state: weekly_floor_editor,
                    width: details_width,
                    height: details_height,
                    inventory_page_start: inventory_page_start.get(),
                    spinner_tick: spinner_tick.get(),
                }))
            }
        }
        .into_any()
    } else if show_stacked_details {
        element! {
            View(width: content_width as u32, flex_direction: FlexDirection::Column) {
                #(stacked_list)
                #(render_detail_panel(QuotaDetailPanelProps {
                    interactive: interaction_mode == QuotaInteractionMode::Interactive,
                    focused_details,
                    account_options: current_account_options.as_ref(),
                    reset_snapshot: current_reset_snapshot.as_ref(),
                    reset_target: current_reset_target.as_ref(),
                    weekly_floor_editor: current_weekly_floor_editor.as_ref(),
                    weekly_floor_editor_state: weekly_floor_editor,
                    width: content_width,
                    height: stacked_details_height,
                    inventory_page_start: inventory_page_start.get(),
                    spinner_tick: spinner_tick.get(),
                }))
            }
        }
        .into_any()
    } else {
        render_account_list(QuotaAccountListProps {
            rows: &visible_rows,
            width: content_width,
            height: list_height,
            focused_row_index: focused_row_index_value,
            visible_rows: visible_account_budget,
            focused_account_id,
            interaction_enabled: account_pointer_interaction_enabled,
            empty_notice,
        })
    };
    let footer_line = quota_legacy_footer(QuotaFooterPresentation {
        weekly_floor_editor: current_weekly_floor_editor.as_ref(),
        account_options: current_account_options.as_ref(),
        reset_snapshot: current_reset_snapshot.as_ref(),
        width: content_width,
        weekly_floor_editing_enabled,
        reset_detail_active,
        filters: filter_state,
        filter_modal_owns_input,
    });
    let controls =
        if interaction_mode == QuotaInteractionMode::Interactive && !filter_modal_owns_input {
            let controls = vec![render_quota_filter_toolbar(
                filter_state.committed,
                visible_rows.len(),
                view_model.rows.len(),
                content_width,
                account_pointer_interaction_enabled,
                move |_| {
                    if !help_open.get()
                        && !quota_filter_modal_owns_input(
                            account_options.read().as_ref(),
                            weekly_floor_editor.read().as_ref(),
                            reset_snapshot.read().as_ref(),
                            reset_target.read().as_ref(),
                        )
                    {
                        filter_controller.dispatch(QuotaFilterAction::Open);
                    }
                },
                move |_| {
                    if !help_open.get() && filters.read().draft.is_none() {
                        details_dialog.set(focused_account_id.read().clone());
                    }
                },
            )];
            controls
        } else {
            Vec::new()
        };
    let route_line = if interaction_mode == QuotaInteractionMode::Interactive {
        format!("Pool-wide: {}", view_model.route_line)
    } else {
        view_model.route_line.clone()
    };

    let interactive = interaction_mode == QuotaInteractionMode::Interactive;
    let actions = quota_dialog_actions(QuotaDialogActionContext {
        help_open,
        weekly_floor_editor,
        weekly_floor_command_port,
        account_options,
        account_options_command_port,
        reset_target,
        reset_snapshot,
        reset_intent_sender: props.reset_intent_sender.clone(),
        inventory_page_start,
        observed_width,
        observed_height,
        focused_account_id,
        rows: view_model.rows.clone(),
        filters,
        interaction_mode,
        should_exit,
    });
    let (sidecar_actions, dialog_actions) = if interactive && sidecar && filter_modal_owns_input {
        (actions, Vec::new())
    } else {
        (Vec::new(), actions)
    };
    let background = render_quota_screen(QuotaScreenChromeProps {
        width,
        height: component_height,
        header: render_quota_header(
            &view_model,
            content_width,
            spinner_tick.get(),
            interactive,
            help_open.get()
                || (!sidecar && filter_modal_owns_input)
                || filter_state.draft.is_some()
                || details_dialog.read().is_some(),
            help_open,
        ),
        route: truncate_quota_header_text(&route_line, content_width),
        controls,
        body,
        actions: sidecar_actions,
        static_footer: (!interactive).then(|| fit_line(&footer_line, content_width)),
    });
    let viewport_width = u16::try_from(width).unwrap_or(u16::MAX);
    let viewport_height = u16::try_from(component_height).unwrap_or(u16::MAX);
    let help_mode = if current_weekly_floor_editor.is_some() {
        QuotaHelpMode::Floor
    } else if reset_detail_active {
        QuotaHelpMode::Reset
    } else if current_account_options.is_some() {
        QuotaHelpMode::Options
    } else if filter_state.draft.is_some() {
        QuotaHelpMode::Filters
    } else if details_dialog.read().is_some() {
        QuotaHelpMode::Details
    } else {
        QuotaHelpMode::Browse
    };
    let mut dialog = render_quota_dialog(QuotaDialogViewProps {
        width: viewport_width,
        height: viewport_height,
        content_width,
        filter_state,
        filter_controller,
        details_dialog,
        view_model: &view_model,
        sidecar,
        filter_modal_owns_input,
        help_action: Some(quota_help_action(help_open)),
        actions: dialog_actions,
        panel: QuotaDetailPanelProps {
            interactive: interaction_mode == QuotaInteractionMode::Interactive,
            focused_details,
            account_options: current_account_options.as_ref(),
            reset_snapshot: current_reset_snapshot.as_ref(),
            reset_target: current_reset_target.as_ref(),
            weekly_floor_editor: current_weekly_floor_editor.as_ref(),
            weekly_floor_editor_state: weekly_floor_editor,
            width: content_width.min(90).saturating_sub(4),
            height: details_content_height,
            inventory_page_start: inventory_page_start.get(),
            spinner_tick: spinner_tick.get(),
        },
    });
    if interactive && help_open.get() {
        dialog = Some(render_quota_help(
            viewport_width,
            viewport_height,
            help_mode,
            help_open,
        ));
    }
    element! {
        View(width: width as u32, height: component_height as u32) {
            DialogHost(width: viewport_width, height: viewport_height, background: vec![background], dialog)
        }
    }
}

pub(super) fn try_send_reset_intent(
    sender: Option<&ResetIntentSender>,
    intent: ResetSessionIntent,
) -> bool {
    if let Some(sender) = sender {
        return sender.send_now(intent).is_ok();
    }
    false
}

pub(super) fn forced_quota_exit_key(code: &KeyCode, modifiers: KeyModifiers) -> bool {
    (matches!(code, KeyCode::Char('c' | 'd')) && modifiers.contains(KeyModifiers::CONTROL))
        || matches!(code, KeyCode::Char('\u{3}' | '\u{4}'))
}

pub(super) fn current_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

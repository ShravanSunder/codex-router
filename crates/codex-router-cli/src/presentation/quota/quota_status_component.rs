use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use iocraft::prelude::*;
use tokio::sync::watch;

use crate::quota_reset::reset_session_supervisor::ConfirmationSelection;
use crate::quota_reset::reset_session_supervisor::PinnedTargetInvalidationReason;
use crate::quota_reset::reset_session_supervisor::ResetIntentSender;
use crate::quota_reset::reset_session_supervisor::ResetSessionIntent;
use crate::quota_reset::reset_session_supervisor::ResetWorkflowSnapshot;
use crate::quota_reset::reset_session_supervisor::WorkflowPhase;

use super::quota_browse_rendering::*;
use super::quota_reset_detail_rendering::*;
use super::quota_reset_keyboard_interaction::*;
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
    pub(super) view_model: QuotaStatusViewModel,
    pub(super) width: usize,
    pub(super) height: usize,
    pub(super) reload_view_model: Option<QuotaStatusViewModelLoader>,
    pub(super) reload_interval: Duration,
    pub(super) spinner_interval: Duration,
    pub(super) reset_intent_sender: Option<ResetIntentSender>,
    pub(super) reset_snapshot_receiver: Option<watch::Receiver<ResetWorkflowSnapshot>>,
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
    let rows_for_navigation = view_model.read().rows.clone();
    let row_count = rows_for_navigation.len();
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
    let reset_snapshot = hooks.use_state(|| {
        props
            .reset_snapshot_receiver
            .as_ref()
            .map(|receiver| receiver.borrow().clone())
    });
    let reset_target = hooks.use_state(|| None::<ResetPaneTarget>);
    let inventory_page_start = hooks.use_state(|| 0usize);
    let spinner_tick = hooks.use_state(|| 0usize);
    let mut should_exit = hooks.use_state(|| false);
    hooks.use_terminal_events({
        let mut observed_width = observed_width;
        let mut observed_height = observed_height;
        let mut focused_account_id = focused_account_id;
        let mut reset_target = reset_target;
        let mut inventory_page_start = inventory_page_start;
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
                let phase = current_reset_snapshot
                    .as_ref()
                    .map_or(WorkflowPhase::Browse, ResetWorkflowSnapshot::phase);
                if phase != WorkflowPhase::Browse {
                    let selection = current_reset_snapshot.as_ref().map_or(
                        ConfirmationSelection::No,
                        ResetWorkflowSnapshot::confirmation_selection,
                    );
                    let yes_enabled = current_reset_snapshot
                        .as_ref()
                        .is_some_and(ResetWorkflowSnapshot::yes_enabled);
                    match reset_key_action(phase, selection, yes_enabled, code, modifiers) {
                        ResetKeyAction::Cancel => {
                            if try_send_reset_intent(
                                reset_intent_sender.as_ref(),
                                ResetSessionIntent::Cancel,
                            ) {
                                reset_target.set(None);
                                inventory_page_start.set(0);
                            }
                        }
                        ResetKeyAction::OpenConfirmation => {
                            let _ = try_send_reset_intent(
                                reset_intent_sender.as_ref(),
                                ResetSessionIntent::OpenConfirmation,
                            );
                        }
                        ResetKeyAction::SelectNo => {
                            let _ = try_send_reset_intent(
                                reset_intent_sender.as_ref(),
                                ResetSessionIntent::SelectNo,
                            );
                        }
                        ResetKeyAction::SelectYes => {
                            let _ = try_send_reset_intent(
                                reset_intent_sender.as_ref(),
                                ResetSessionIntent::SelectYes,
                            );
                        }
                        ResetKeyAction::Confirm => {
                            let _ = try_send_reset_intent(
                                reset_intent_sender.as_ref(),
                                ResetSessionIntent::Confirm {
                                    now_unix_seconds: current_unix_seconds(),
                                },
                            );
                        }
                        ResetKeyAction::DismissResult => {
                            if try_send_reset_intent(
                                reset_intent_sender.as_ref(),
                                ResetSessionIntent::DismissResult,
                            ) {
                                reset_target.set(None);
                                inventory_page_start.set(0);
                            }
                        }
                        ResetKeyAction::PreviousInventoryPage
                        | ResetKeyAction::NextInventoryPage => {
                            let next_page = matches!(
                                reset_key_action(phase, selection, yes_enabled, code, modifiers),
                                ResetKeyAction::NextInventoryPage
                            );
                            let width = observed_width.get();
                            let focused_index = focused_row_index_for_account(
                                &rows_for_navigation,
                                focused_account_id.read().as_ref(),
                            );
                            let sidecar = width >= SIDECAR_QUOTA_WIDTH;
                            let layout = quota_body_layout(
                                quota_body_budget(observed_height.get()),
                                sidecar,
                                !sidecar && width >= NARROW_QUOTA_WIDTH && focused_index.is_some(),
                                row_count,
                                focused_index,
                                selected_detail_height(focused_index.is_some()),
                                false,
                            );
                            let page_size =
                                reset_inventory_page_size(layout.detail_viewport_height(sidecar));
                            inventory_page_start.set(credit_page_start(
                                inventory_page_start.get(),
                                current_reset_snapshot
                                    .as_ref()
                                    .map_or(0, |snapshot| snapshot.credit_inventory().len()),
                                page_size,
                                next_page,
                            ));
                        }
                        ResetKeyAction::ExitPrecommit => {
                            if try_send_reset_intent(
                                reset_intent_sender.as_ref(),
                                ResetSessionIntent::Shutdown,
                            ) {
                                should_exit.set(true);
                            }
                        }
                        ResetKeyAction::None => {}
                    }
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
                            && let Some(active_credential_generation) =
                                row.active_credential_generation
                        {
                            let target = ResetPaneTarget {
                                account_id: row.account_id.clone(),
                                active_credential_generation,
                                account_label: row.account.clone(),
                                account_tag: row.account_tag.clone(),
                                saved_reset_credits: row.reset_credits.clone(),
                                saved_weekly_window: row.weekly_window.clone(),
                            };
                            if row.enabled
                                && try_send_reset_intent(
                                    reset_intent_sender.as_ref(),
                                    ResetSessionIntent::BeginInspection {
                                        account_id: row.account_id.clone(),
                                        active_credential_generation,
                                        now_unix_seconds: current_unix_seconds(),
                                    },
                                )
                            {
                                reset_target.set(Some(target));
                                inventory_page_start.set(0);
                            }
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
        let mut view_model = view_model;
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
                if let Some(next_view_model) = reload_view_model().await {
                    view_model.set(next_view_model);
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
    let focused_row_index_value =
        focused_row_index_for_account(&view_model.rows, focused_account_id.read().as_ref());
    let focused_details = focused_row_index_value
        .and_then(|index| view_model.rows.get(index).map(|row| &row.details));
    let current_reset_snapshot = reset_snapshot.read().clone().or_else(|| {
        props
            .reset_snapshot_receiver
            .as_ref()
            .map(|receiver| receiver.borrow().clone())
    });
    let current_reset_target = reset_target.read().clone();
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
    let body_budget = quota_body_budget(height);
    let reset_detail_active = current_reset_snapshot
        .as_ref()
        .is_some_and(|snapshot| reset_mode(Some(snapshot)));
    let details_content_height = current_reset_snapshot
        .as_ref()
        .filter(|_| reset_detail_active)
        .map(reset_panel_content_height)
        .unwrap_or_else(|| selected_detail_height(focused_details.is_some()));
    let sidecar = width >= SIDECAR_QUOTA_WIDTH;
    let stacked_details = !sidecar
        && width >= NARROW_QUOTA_WIDTH
        && (focused_details.is_some() || props.view_model.selected.is_none());
    let layout = quota_body_layout(
        body_budget,
        sidecar,
        stacked_details,
        row_count,
        focused_row_index_value,
        details_content_height,
        !reset_detail_active,
    );
    let details_height = layout.details_height;
    let visible_account_budget = layout.visible_account_budget;
    let list_height = layout.list_height;
    let stacked_details_height = layout.stacked_details_height;
    let show_stacked_details = layout.show_stacked_details;
    let body_height = layout.body_height;
    let component_height = quota_status_height(height);
    let body = if sidecar {
        let list_width = (content_width.saturating_sub(2) * 3 / 5)
            .max(58)
            .min(content_width.saturating_sub(44));
        let details_width = content_width.saturating_sub(list_width + 2).max(34);
        element! {
            View(width: 100pct, height: body_height as u32) {
                #(render_account_list(&view_model.rows, list_width, list_height, focused_row_index_value, visible_account_budget))
                View(width: 2) { Text(content: "") }
                #(render_detail_panel(
                    focused_details,
                    current_reset_snapshot.as_ref(),
                    current_reset_target.as_ref(),
                    details_width,
                    details_height,
                    inventory_page_start.get(),
                    spinner_tick.get(),
                ))
            }
        }
        .into_any()
    } else if show_stacked_details {
        element! {
            View(width: content_width as u32, flex_direction: FlexDirection::Column) {
                #(render_account_list(&view_model.rows, content_width, list_height, focused_row_index_value, visible_account_budget))
                #(render_detail_panel(
                    focused_details,
                    current_reset_snapshot.as_ref(),
                    current_reset_target.as_ref(),
                    content_width,
                    stacked_details_height,
                    inventory_page_start.get(),
                    spinner_tick.get(),
                ))
            }
        }
        .into_any()
    } else {
        render_account_list(
            &view_model.rows,
            content_width,
            list_height,
            focused_row_index_value,
            visible_account_budget,
        )
    };

    element! {
        View(
            width: width as u32,
            height: component_height as u32,
            border_style: BorderStyle::Round,
            border_color: Color::Cyan,
            overflow: Overflow::Hidden,
            padding_left: 1,
            padding_right: 1,
            padding_top: 0,
            padding_bottom: 0,
            flex_direction: FlexDirection::Column,
        ) {
            Text(content: quota_title_line(&view_model, content_width, spinner_tick.get()), color: Color::Cyan, weight: Weight::Bold, wrap: TextWrap::NoWrap)
            Text(content: fit_line(&view_model.route_line, content_width), color: Color::White, weight: Weight::Bold, wrap: TextWrap::NoWrap)
            #(body)
            Text(content: fit_line(reset_footer(current_reset_snapshot.as_ref()), content_width), color: Color::Grey, wrap: TextWrap::NoWrap)
        }
    }
}

fn render_detail_panel(
    focused_details: Option<&QuotaSelectedAccountViewModel>,
    reset_snapshot: Option<&ResetWorkflowSnapshot>,
    reset_target: Option<&ResetPaneTarget>,
    width: usize,
    height: usize,
    inventory_page_start: usize,
    spinner_tick: usize,
) -> AnyElement<'static> {
    if let (Some(snapshot), Some(target)) = (reset_snapshot, reset_target)
        && reset_mode(Some(snapshot))
    {
        return render_reset_panel(
            snapshot,
            target,
            width,
            height,
            inventory_page_start,
            reset_inventory_page_size(height),
            spinner_tick,
        );
    }
    render_selected_panel(focused_details, width, height)
}

fn try_send_reset_intent(sender: Option<&ResetIntentSender>, intent: ResetSessionIntent) -> bool {
    if let Some(sender) = sender {
        return sender.send_now(intent).is_ok();
    }
    false
}

fn current_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

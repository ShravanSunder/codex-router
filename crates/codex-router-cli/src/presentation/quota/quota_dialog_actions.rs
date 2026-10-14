//! Plain actions dispatch through the existing editor/options/reset input owners.
use super::quota_account_options::*;
use super::quota_floor_editor::*;
use super::quota_provider_filters::*;
use super::quota_reset_component_input::*;
use super::quota_reset_presentation_model::*;
use super::quota_status_component::current_unix_seconds;
use super::quota_status_view_model::QuotaStatusAccountViewModel;
use crate::quota_reset::reset_session_supervisor::{
    ResetIntentSender, ResetWorkflowSnapshot, WorkflowPhase,
};
use codex_router_core::ids::AccountId;
use codex_router_tui::DialogAction;
use iocraft::prelude::*;

#[derive(Clone)]
pub(super) struct QuotaDialogActionContext {
    pub help_open: State<bool>,
    pub weekly_floor_editor: State<Option<WeeklyFloorEditorState>>,
    pub weekly_floor_command_port: WeeklyFloorEditorCommandPort,
    pub account_options: State<Option<AccountOptionsState>>,
    pub account_options_command_port: AccountOptionsCommandPort,
    pub reset_target: State<Option<ResetPaneTarget>>,
    pub reset_snapshot: State<Option<ResetWorkflowSnapshot>>,
    pub reset_intent_sender: Option<ResetIntentSender>,
    pub inventory_page_start: State<usize>,
    pub observed_width: State<usize>,
    pub observed_height: State<usize>,
    pub focused_account_id: State<Option<AccountId>>,
    pub rows: Vec<QuotaStatusAccountViewModel>,
    pub filters: State<QuotaFilterState>,
    pub interaction_mode: QuotaInteractionMode,
    pub should_exit: State<bool>,
}

impl QuotaDialogActionContext {
    fn dispatch(mut self, code: KeyCode) {
        if self.help_open.get() {
            return;
        }
        let snapshot = self.reset_snapshot.read().clone();
        let phase = snapshot
            .as_ref()
            .map_or(WorkflowPhase::Browse, ResetWorkflowSnapshot::phase);
        if phase == WorkflowPhase::Browse && self.weekly_floor_editor.read().is_some() {
            let mut next = self.weekly_floor_editor.read().clone();
            let command = weekly_floor_editor_key_command(&mut next, code, KeyModifiers::NONE);
            self.weekly_floor_editor.set(next);
            if let Some(command) = command
                && !self.weekly_floor_command_port.send(command)
            {
                let mut next = self.weekly_floor_editor.read().clone();
                weekly_floor_editor_send_failed(&mut next);
                self.weekly_floor_editor.set(next);
            }
            return;
        }
        if handle_account_options_key_event(
            &mut self.account_options,
            &mut self.reset_target,
            AccountOptionsKeyEventContext {
                phase,
                code,
                modifiers: KeyModifiers::NONE,
                command_port: &self.account_options_command_port,
                reset_snapshot: snapshot.as_ref(),
                reset_intent_sender: self.reset_intent_sender.as_ref(),
                inventory_page_start: &mut self.inventory_page_start,
                now_unix_seconds: current_unix_seconds(),
            },
        ) {
            return;
        }
        if phase != WorkflowPhase::Browse {
            handle_quota_reset_key(QuotaResetKeyContext {
                phase,
                code,
                modifiers: KeyModifiers::NONE,
                current_reset_snapshot: snapshot.as_ref(),
                reset_intent_sender: self.reset_intent_sender.as_ref(),
                reset_target: self.reset_target,
                inventory_page_start: self.inventory_page_start,
                observed_width: self.observed_width,
                observed_height: self.observed_height,
                account_options: self.account_options,
                weekly_floor_editor: self.weekly_floor_editor,
                rows_for_navigation: &self.rows,
                focused_account_id: self.focused_account_id,
                should_exit: self.should_exit,
                interaction_mode: self.interaction_mode,
                filters: self.filters,
            });
        }
    }
}

pub(super) fn quota_dialog_actions(context: QuotaDialogActionContext) -> Vec<AnyElement<'static>> {
    let snapshot = context.reset_snapshot.read().clone();
    let phase = snapshot
        .as_ref()
        .map_or(WorkflowPhase::Browse, ResetWorkflowSnapshot::phase);
    let labels = if let Some(editor) = context.weekly_floor_editor.read().as_ref() {
        match editor.phase {
            WeeklyFloorEditorPhase::Saving => vec![],
            WeeklyFloorEditorPhase::SavedRefreshFailed => vec![("Retry refresh", KeyCode::Enter)],
            _ => vec![("Save", KeyCode::Enter), ("Cancel", KeyCode::Esc)],
        }
    } else if phase == WorkflowPhase::Committing {
        vec![]
    } else if phase != WorkflowPhase::Browse {
        match phase {
            WorkflowPhase::Inspected => vec![("Review", KeyCode::Enter), ("Cancel", KeyCode::Esc)],
            WorkflowPhase::Confirming => {
                let mut choices = vec![("No", KeyCode::Left)];
                if snapshot
                    .as_ref()
                    .is_some_and(ResetWorkflowSnapshot::yes_enabled)
                {
                    choices.push(("Yes", KeyCode::Right));
                }
                choices.push(("Confirm", KeyCode::Enter));
                choices.push(("Cancel", KeyCode::Esc));
                choices
            }
            WorkflowPhase::Result => vec![("Close", KeyCode::Esc)],
            _ => vec![("Cancel", KeyCode::Esc)],
        }
    } else if let Some(options) = context.account_options.read().as_ref() {
        if options.editor.is_some() {
            vec![("Save", KeyCode::Enter), ("Cancel", KeyCode::Esc)]
        } else {
            vec![
                (
                    if options.tab == AccountOptionsTab::Resets {
                        "Inspect"
                    } else {
                        "Edit"
                    },
                    KeyCode::Enter,
                ),
                ("Close", KeyCode::Esc),
            ]
        }
    } else {
        vec![]
    };
    let buttons = labels.into_iter().map(|(label, code)| {
        let context = context.clone();
        element!(DialogAction(label: label.to_owned(), handler: move |_| context.clone().dispatch(code))).into_any()
    }).collect::<Vec<_>>();
    if buttons.is_empty() {
        Vec::new()
    } else {
        vec![element! { View(column_gap: 3) { #(buttons) } }.into_any()]
    }
}

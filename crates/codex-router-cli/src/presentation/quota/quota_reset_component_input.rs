use super::quota_account_options::*;
use super::quota_floor_editor::WeeklyFloorEditorState;
use super::quota_provider_filter_rendering::*;
use super::quota_provider_filters::*;
use super::quota_reset_keyboard_interaction::*;
use super::quota_reset_presentation_model::*;
use super::quota_status_component::{
    NARROW_QUOTA_WIDTH, SIDECAR_QUOTA_WIDTH, current_unix_seconds, try_send_reset_intent,
};
use super::quota_status_view_model::QuotaStatusAccountViewModel;
use super::responsive_quota_layout::*;
use crate::quota_reset::reset_session_supervisor::{
    ConfirmationSelection, ResetIntentSender, ResetSessionIntent, ResetWorkflowSnapshot,
    WorkflowPhase,
};
use iocraft::prelude::*;

pub(super) struct QuotaResetKeyContext<'a> {
    pub(super) phase: WorkflowPhase,
    pub(super) code: KeyCode,
    pub(super) modifiers: KeyModifiers,
    pub(super) current_reset_snapshot: Option<&'a ResetWorkflowSnapshot>,
    pub(super) reset_intent_sender: Option<&'a ResetIntentSender>,
    pub(super) reset_target: State<Option<ResetPaneTarget>>,
    pub(super) inventory_page_start: State<usize>,
    pub(super) observed_width: State<usize>,
    pub(super) observed_height: State<usize>,
    pub(super) account_options: State<Option<AccountOptionsState>>,
    pub(super) weekly_floor_editor: State<Option<WeeklyFloorEditorState>>,
    pub(super) rows_for_navigation: &'a [QuotaStatusAccountViewModel],
    pub(super) focused_account_id: State<Option<codex_router_core::ids::AccountId>>,
    pub(super) should_exit: State<bool>,
    pub(super) interaction_mode: QuotaInteractionMode,
    pub(super) filters: State<QuotaFilterState>,
}

pub(super) fn handle_quota_reset_key(context: QuotaResetKeyContext<'_>) {
    let QuotaResetKeyContext {
        phase,
        code,
        modifiers,
        current_reset_snapshot,
        reset_intent_sender,
        mut reset_target,
        mut inventory_page_start,
        observed_width,
        observed_height,
        account_options,
        weekly_floor_editor,
        rows_for_navigation,
        focused_account_id,
        mut should_exit,
        interaction_mode,
        filters,
    } = context;
    let selection = current_reset_snapshot.map_or(
        ConfirmationSelection::No,
        ResetWorkflowSnapshot::confirmation_selection,
    );
    let yes_enabled = current_reset_snapshot.is_some_and(ResetWorkflowSnapshot::yes_enabled);
    match reset_key_action(phase, selection, yes_enabled, code, modifiers) {
        ResetKeyAction::Cancel => {
            if try_send_reset_intent(reset_intent_sender, ResetSessionIntent::Cancel) {
                reset_target.set(None);
                inventory_page_start.set(0);
            }
        }
        ResetKeyAction::OpenConfirmation => {
            let _ =
                try_send_reset_intent(reset_intent_sender, ResetSessionIntent::OpenConfirmation);
        }
        ResetKeyAction::SelectNo => {
            let _ = try_send_reset_intent(reset_intent_sender, ResetSessionIntent::SelectNo);
        }
        ResetKeyAction::SelectYes => {
            let _ = try_send_reset_intent(reset_intent_sender, ResetSessionIntent::SelectYes);
        }
        ResetKeyAction::Confirm => {
            let _ = try_send_reset_intent(
                reset_intent_sender,
                ResetSessionIntent::Confirm {
                    now_unix_seconds: current_unix_seconds(),
                },
            );
        }
        ResetKeyAction::DismissResult => {
            if try_send_reset_intent(reset_intent_sender, ResetSessionIntent::DismissResult) {
                reset_target.set(None);
                inventory_page_start.set(0);
            }
        }
        ResetKeyAction::PreviousInventoryPage | ResetKeyAction::NextInventoryPage => {
            let next_page = matches!(
                reset_key_action(phase, selection, yes_enabled, code, modifiers),
                ResetKeyAction::NextInventoryPage
            );
            let width = observed_width.get();
            let focused_index = focused_row_index_for_account(
                rows_for_navigation,
                focused_account_id.read().as_ref(),
            );
            let sidecar = width >= SIDECAR_QUOTA_WIDTH;
            let current_options = account_options.read().clone();
            let current_reset_target = reset_target.read().clone();
            let details_content_height = current_options.as_ref().map_or_else(
                || selected_detail_height(focused_index.is_some()),
                |options| {
                    account_options_content_height(
                        options,
                        current_reset_snapshot,
                        current_reset_target.as_ref(),
                        inventory_page_start.get(),
                    )
                },
            );
            let layout_mode = if sidecar {
                QuotaBodyLayoutMode::Sidecar
            } else if current_options.is_some()
                || (width >= NARROW_QUOTA_WIDTH && focused_index.is_some())
            {
                QuotaBodyLayoutMode::Stacked {
                    fill_available_height: false,
                    prioritize_details: current_options.is_some(),
                }
            } else {
                QuotaBodyLayoutMode::Inline
            };
            let layout = quota_body_layout(
                quota_body_budget(observed_height.get()).saturating_sub(
                    quota_filter_controls_height(
                        interaction_mode,
                        width.saturating_sub(4).max(44),
                        filters.read().draft.is_some(),
                        quota_filter_modal_owns_input(
                            current_options.as_ref(),
                            weekly_floor_editor.read().as_ref(),
                            current_reset_snapshot,
                            current_reset_target.as_ref(),
                        ),
                    ),
                ),
                layout_mode,
                rows_for_navigation.len(),
                focused_index,
                details_content_height,
            );
            let detail_height = if !sidecar && interaction_mode == QuotaInteractionMode::Interactive
            {
                super::quota_dialog_rendering::quota_dialog_panel_height(
                    observed_height.get(),
                    details_content_height,
                )
            } else {
                layout.detail_viewport_height(sidecar)
            };
            let options_page_size = current_options
                .as_ref()
                .filter(|options| options.tab == AccountOptionsTab::Resets)
                .and(current_reset_snapshot)
                .filter(|snapshot| snapshot.phase() != WorkflowPhase::Browse)
                .and_then(|_| {
                    current_reset_target.as_ref().map(|_| {
                        account_options_reset_inventory_page_size(detail_height.saturating_sub(7))
                    })
                });
            let page_size =
                options_page_size.unwrap_or_else(|| reset_inventory_page_size(detail_height));
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
            if try_send_reset_intent(reset_intent_sender, ResetSessionIntent::Shutdown) {
                should_exit.set(true);
            }
        }
        ResetKeyAction::None => {}
    }
}

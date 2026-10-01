use iocraft::prelude::KeyCode;
use iocraft::prelude::KeyModifiers;
use iocraft::prelude::State;

use crate::quota_reset::reset_session_supervisor::ResetIntentSender;
use crate::quota_reset::reset_session_supervisor::ResetSessionIntent;
use crate::quota_reset::reset_session_supervisor::ResetWorkflowSnapshot;
use crate::quota_reset::reset_session_supervisor::WorkflowPhase;

use super::super::quota_reset_presentation_model::ResetPaneTarget;
use super::super::quota_status_view_model::QuotaStatusAccountViewModel;
use super::AccountOptionsCommandPort;
use super::AccountOptionsKeyAction;
use super::AccountOptionsMessage;
use super::AccountOptionsState;
use super::CreditPolicyEditorPhase;
use super::CreditUsagePolicySaveError;
use super::account_options_key_action;
use super::update_credit_policy_editor;

pub(in crate::presentation::quota) struct AccountOptionsKeyEventContext<'a> {
    pub(in crate::presentation::quota) phase: WorkflowPhase,
    pub(in crate::presentation::quota) code: KeyCode,
    pub(in crate::presentation::quota) modifiers: KeyModifiers,
    pub(in crate::presentation::quota) command_port: &'a AccountOptionsCommandPort,
    pub(in crate::presentation::quota) reset_snapshot: Option<&'a ResetWorkflowSnapshot>,
    pub(in crate::presentation::quota) reset_intent_sender: Option<&'a ResetIntentSender>,
    pub(in crate::presentation::quota) now_unix_seconds: u64,
}

pub(in crate::presentation::quota) fn handle_account_options_key_event(
    account_options: &mut State<Option<AccountOptionsState>>,
    reset_target: &mut State<Option<ResetPaneTarget>>,
    context: AccountOptionsKeyEventContext<'_>,
) -> bool {
    let AccountOptionsKeyEventContext {
        phase,
        code,
        modifiers,
        command_port,
        reset_snapshot,
        reset_intent_sender,
        now_unix_seconds,
    } = context;
    let Some(options) = account_options.read().clone() else {
        return false;
    };
    let action = account_options_key_action(&options, phase, code, modifiers);
    let expected_inspection_attempt =
        reset_snapshot.and_then(ResetWorkflowSnapshot::inspection_attempt_generation);

    if action == AccountOptionsKeyAction::ExitPending {
        return false;
    }

    if phase != WorkflowPhase::Browse {
        match action {
            AccountOptionsKeyAction::SwitchTab(_) => apply_account_options_key_action(
                account_options,
                command_port,
                action,
                expected_inspection_attempt,
            ),
            AccountOptionsKeyAction::ResetWorkflow => return false,
            AccountOptionsKeyAction::Close => {
                close_account_options(account_options, reset_target, reset_intent_sender)
            }
            _ => {}
        }
        return true;
    }

    match action {
        AccountOptionsKeyAction::BeginResetInspection => {
            if reset_target.read().is_none()
                && let Some(target) = reset_pane_target_for_account_options(&options)
                && let Some(sender) = reset_intent_sender
            {
                reset_target.set(Some(target.clone()));
                if sender
                    .send_now(ResetSessionIntent::BeginInspection {
                        account_id: target.account_id.clone(),
                        active_credential_generation: target.active_credential_generation,
                        now_unix_seconds,
                    })
                    .is_err()
                {
                    reset_target.set(None);
                }
            }
        }
        AccountOptionsKeyAction::Close => {
            close_account_options(account_options, reset_target, reset_intent_sender);
        }
        AccountOptionsKeyAction::ResetWorkflow | AccountOptionsKeyAction::Ignore => {}
        _ => apply_account_options_key_action(
            account_options,
            command_port,
            action,
            expected_inspection_attempt,
        ),
    }
    true
}

fn close_account_options(
    account_options: &mut State<Option<AccountOptionsState>>,
    reset_target: &mut State<Option<ResetPaneTarget>>,
    reset_intent_sender: Option<&ResetIntentSender>,
) {
    if reset_target.read().is_some() {
        send_reset_intent(reset_intent_sender, ResetSessionIntent::Cancel);
        reset_target.set(None);
    }
    account_options.set(None);
}

pub(in crate::presentation::quota) fn open_account_options_for_account(
    account_options: &mut State<Option<AccountOptionsState>>,
    session_generation: &mut State<u64>,
    row: &QuotaStatusAccountViewModel,
    reset_target: &mut State<Option<ResetPaneTarget>>,
    reset_intent_sender: Option<&ResetIntentSender>,
    now_unix_seconds: u64,
) {
    let next_generation = session_generation.read().saturating_add(1);
    session_generation.set(next_generation);
    account_options.set(Some(AccountOptionsState::new(row, next_generation)));

    if let Some(target) = reset_pane_target_for_account(row)
        && let Some(sender) = reset_intent_sender
    {
        reset_target.set(Some(target.clone()));
        if sender
            .send_now(ResetSessionIntent::BeginInspection {
                account_id: target.account_id.clone(),
                active_credential_generation: target.active_credential_generation,
                now_unix_seconds,
            })
            .is_err()
        {
            reset_target.set(None);
        }
    }
}

fn apply_account_options_key_action(
    account_options: &mut State<Option<AccountOptionsState>>,
    command_port: &AccountOptionsCommandPort,
    action: AccountOptionsKeyAction,
    expected_inspection_attempt: Option<u64>,
) {
    let Some(mut options) = account_options.read().clone() else {
        return;
    };
    let command = update_credit_policy_editor(&mut options, action, expected_inspection_attempt);
    account_options.set(Some(options));
    let Some(command) = command else {
        return;
    };
    if command_port.send(command) {
        return;
    }
    let Some(mut options) = account_options.read().clone() else {
        return;
    };
    match action {
        AccountOptionsKeyAction::SavePolicy => {
            if let Some(editor) = options.editor.as_mut() {
                editor.phase = CreditPolicyEditorPhase::SaveFailed(
                    CreditUsagePolicySaveError::StateOperationFailed,
                );
            }
        }
        AccountOptionsKeyAction::RefreshCredits | AccountOptionsKeyAction::RetryReload => {
            options.message = Some(AccountOptionsMessage::RefreshFailed);
        }
        AccountOptionsKeyAction::SwitchTab(_) => {
            options.pending_tab = None;
            options.pending_tab_request = None;
            options.message = Some(AccountOptionsMessage::ResetReviewActive);
        }
        _ => {}
    }
    account_options.set(Some(options));
}

fn reset_pane_target_for_account(row: &QuotaStatusAccountViewModel) -> Option<ResetPaneTarget> {
    if !row.enabled {
        return None;
    }
    let active_credential_generation = row.active_credential_generation?;
    Some(ResetPaneTarget {
        account_id: row.account_id.clone(),
        active_credential_generation,
        account_label: row.account.clone(),
        account_tag: row.account_tag.clone(),
        saved_reset_credits: row.reset_credits.clone(),
        saved_weekly_window: row.weekly_window.clone(),
    })
}

fn reset_pane_target_for_account_options(options: &AccountOptionsState) -> Option<ResetPaneTarget> {
    if !options.target.enabled {
        return None;
    }
    let active_credential_generation = options.target.credential_generation?;
    Some(ResetPaneTarget {
        account_id: options.target.account_id.clone(),
        active_credential_generation,
        account_label: options.target.account_label.clone(),
        account_tag: options.target.account_tag.clone(),
        saved_reset_credits: options.target.saved_reset_credits.clone(),
        saved_weekly_window: options.target.saved_weekly_window.clone(),
    })
}

fn send_reset_intent(sender: Option<&ResetIntentSender>, intent: ResetSessionIntent) -> bool {
    sender.is_some_and(|sender| sender.send_now(intent).is_ok())
}

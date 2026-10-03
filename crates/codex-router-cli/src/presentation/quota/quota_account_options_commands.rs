use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use iocraft::prelude::State;
use tokio::sync::watch;

use crate::quota_reset::reset_session_supervisor::InspectionTabRequestId;
use crate::quota_reset::reset_session_supervisor::ResetIntentSender;
use crate::quota_reset::reset_session_supervisor::ResetSessionIntent;
use crate::quota_reset::reset_session_supervisor::ResetWorkflowSnapshot;
use crate::quota_reset::reset_session_supervisor::WorkflowPhase;

use super::super::quota_floor_editor::QuotaStatusReloadLock;
use super::super::quota_reset_presentation_model::ResetPaneTarget;
use super::super::quota_status_view_model::QuotaStatusViewModel;
use super::super::quota_status_view_model::QuotaStatusViewModelLoader;
use super::AccountOptionsCommand;
use super::AccountOptionsMessage;
use super::AccountOptionsState;
use super::AccountOptionsTab;
use super::CreditPolicyEditorPhase;
use super::CreditUsagePolicySaveError;
use super::CreditUsagePolicySaver;
use super::CreditUsageRefresher;
use super::update_account_options_from_report;

pub(in crate::presentation::quota) struct AccountOptionsCommandContext {
    pub(in crate::presentation::quota) receiver:
        tokio::sync::mpsc::UnboundedReceiver<AccountOptionsCommand>,
    pub(in crate::presentation::quota) policy_saver: Option<CreditUsagePolicySaver>,
    pub(in crate::presentation::quota) refresher: Option<CreditUsageRefresher>,
    pub(in crate::presentation::quota) loader: Option<QuotaStatusViewModelLoader>,
    pub(in crate::presentation::quota) reload_lock: QuotaStatusReloadLock,
    pub(in crate::presentation::quota) view_model: State<QuotaStatusViewModel>,
    pub(in crate::presentation::quota) account_options: State<Option<AccountOptionsState>>,
    pub(in crate::presentation::quota) reset_snapshot: State<Option<ResetWorkflowSnapshot>>,
    pub(in crate::presentation::quota) reset_snapshot_receiver:
        Option<watch::Receiver<ResetWorkflowSnapshot>>,
    pub(in crate::presentation::quota) reset_target: State<Option<ResetPaneTarget>>,
    pub(in crate::presentation::quota) inventory_page_start: State<usize>,
    pub(in crate::presentation::quota) reset_intent_sender: Option<ResetIntentSender>,
}

pub(in crate::presentation::quota) async fn run_account_options_commands(
    context: AccountOptionsCommandContext,
) {
    let AccountOptionsCommandContext {
        mut receiver,
        policy_saver,
        refresher,
        loader,
        reload_lock,
        mut view_model,
        mut account_options,
        mut reset_snapshot,
        mut reset_snapshot_receiver,
        mut reset_target,
        mut inventory_page_start,
        reset_intent_sender,
    } = context;
    while let Some(command) = receiver.recv().await {
        match command {
            AccountOptionsCommand::SwitchTab {
                account_id,
                credential_generation,
                session_generation,
                operation_generation,
                tab,
                request_id,
                expected_inspection_attempt,
            } => {
                if !tab_switch_is_current(
                    account_options.read().as_ref(),
                    &account_id,
                    credential_generation,
                    session_generation,
                    operation_generation,
                    tab,
                    request_id,
                ) {
                    continue;
                }
                let acknowledged_snapshot = match (
                    reset_intent_sender.as_ref(),
                    reset_snapshot_receiver.as_mut(),
                ) {
                    (Some(sender), Some(snapshot_receiver)) => {
                        if sender
                            .send_now(ResetSessionIntent::CancelInspectionForTab {
                                request_id,
                                expected_inspection_attempt,
                            })
                            .is_err()
                        {
                            None
                        } else {
                            wait_for_inspection_tab_ack(snapshot_receiver, request_id).await
                        }
                    }
                    _ => None,
                };
                if let Some(snapshot) = acknowledged_snapshot.as_ref() {
                    reset_snapshot.set(Some(snapshot.clone()));
                }
                let no_supervisor_is_idle = reset_intent_sender.is_none()
                    && reset_snapshot_receiver.is_none()
                    && reset_target.read().is_none()
                    && reset_snapshot
                        .read()
                        .as_ref()
                        .is_none_or(|snapshot| snapshot.phase() == WorkflowPhase::Browse);
                let phase_is_browse = acknowledged_snapshot
                    .as_ref()
                    .is_some_and(|snapshot| snapshot.phase() == WorkflowPhase::Browse)
                    || no_supervisor_is_idle;
                let mut next_reset_target = None;
                {
                    let mut options_state = account_options.write();
                    let Some(options) = options_state.as_mut() else {
                        continue;
                    };
                    if !tab_switch_is_current(
                        Some(options),
                        &account_id,
                        credential_generation,
                        session_generation,
                        operation_generation,
                        tab,
                        request_id,
                    ) {
                        continue;
                    }
                    options.pending_tab = None;
                    options.pending_tab_request = None;
                    if phase_is_browse {
                        options.tab = tab;
                        options.message = None;
                        next_reset_target = reset_target_for_options(options);
                        inventory_page_start.set(0);
                    } else {
                        options.tab = AccountOptionsTab::Resets;
                        options.message = Some(AccountOptionsMessage::ResetReviewActive);
                    }
                }
                if !phase_is_browse {
                    continue;
                }
                reset_target.set(None);
                if tab == AccountOptionsTab::Resets
                    && let Some(target) = next_reset_target
                    && let Some(sender) = reset_intent_sender.as_ref()
                {
                    reset_target.set(Some(target.clone()));
                    if sender
                        .send_now(ResetSessionIntent::BeginInspection {
                            account_id: target.account_id.clone(),
                            active_credential_generation: target.active_credential_generation,
                            now_unix_seconds: current_unix_seconds(),
                        })
                        .is_err()
                    {
                        reset_target.set(None);
                    }
                }
            }
            AccountOptionsCommand::SavePolicy {
                account_id,
                credential_generation,
                session_generation,
                operation_generation,
                policy,
            } => {
                if !editor_command_is_current(
                    account_options.read().as_ref(),
                    &account_id,
                    credential_generation,
                    session_generation,
                    operation_generation,
                ) {
                    continue;
                }
                let saved_policy = match policy_saver.as_ref() {
                    Some(saver) => saver(account_id.clone(), credential_generation, policy).await,
                    None => Err(CreditUsagePolicySaveError::StateOperationFailed),
                };
                match saved_policy {
                    Err(error) => {
                        if let Some(options) = account_options.write().as_mut()
                            && editor_command_is_current(
                                Some(options),
                                &account_id,
                                credential_generation,
                                session_generation,
                                operation_generation,
                            )
                            && let Some(editor) = options.editor.as_mut()
                        {
                            editor.phase = CreditPolicyEditorPhase::SaveFailed(error);
                        }
                    }
                    Ok(saved_policy) => {
                        let _reload_guard = reload_lock.lock().await;
                        let refreshed_view_model = match loader.as_ref() {
                            Some(loader) => loader().await,
                            None => None,
                        };
                        if let Some(next_view_model) = refreshed_view_model {
                            install_reloaded_view_model(
                                next_view_model,
                                &mut view_model,
                                &mut account_options,
                                &reset_snapshot,
                                &mut reset_target,
                                reset_intent_sender.as_ref(),
                            );
                            if let Some(options) = account_options.write().as_mut()
                                && editor_command_is_current(
                                    Some(options),
                                    &account_id,
                                    credential_generation,
                                    session_generation,
                                    operation_generation,
                                )
                            {
                                options.editor = None;
                                options.message = None;
                            }
                        } else if let Some(options) = account_options.write().as_mut()
                            && editor_command_is_current(
                                Some(options),
                                &account_id,
                                credential_generation,
                                session_generation,
                                operation_generation,
                            )
                            && let Some(editor) = options.editor.as_mut()
                        {
                            editor.saved_policy = saved_policy;
                            editor.draft_policy = saved_policy;
                            editor.phase = CreditPolicyEditorPhase::SavedReloadFailed;
                        }
                    }
                }
            }
            AccountOptionsCommand::ReloadAfterSave {
                account_id,
                credential_generation,
                session_generation,
                operation_generation,
            } => {
                if !saved_reload_is_current(
                    account_options.read().as_ref(),
                    &account_id,
                    credential_generation,
                    session_generation,
                    operation_generation,
                ) {
                    continue;
                }
                let _reload_guard = reload_lock.lock().await;
                let refreshed_view_model = match loader.as_ref() {
                    Some(loader) => loader().await,
                    None => None,
                };
                let Some(next_view_model) = refreshed_view_model else {
                    if let Some(options) = account_options.write().as_mut()
                        && saved_reload_is_current(
                            Some(options),
                            &account_id,
                            credential_generation,
                            session_generation,
                            operation_generation,
                        )
                    {
                        options.message = Some(AccountOptionsMessage::RefreshFailed);
                    }
                    continue;
                };
                install_reloaded_view_model(
                    next_view_model,
                    &mut view_model,
                    &mut account_options,
                    &reset_snapshot,
                    &mut reset_target,
                    reset_intent_sender.as_ref(),
                );
                if let Some(options) = account_options.write().as_mut()
                    && saved_reload_is_current(
                        Some(options),
                        &account_id,
                        credential_generation,
                        session_generation,
                        operation_generation,
                    )
                {
                    options.editor = None;
                    options.message = None;
                }
            }
            AccountOptionsCommand::RefreshCredits {
                account_id,
                credential_generation,
                session_generation,
                operation_generation,
            } => {
                if !refresh_command_is_current(
                    account_options.read().as_ref(),
                    &account_id,
                    credential_generation,
                    session_generation,
                    operation_generation,
                ) {
                    continue;
                }
                let refresh_result = match refresher.as_ref() {
                    Some(refresher) => refresher(account_id.clone(), credential_generation).await,
                    None => Err(super::CreditUsageRefreshError::Failed),
                };
                if let Err(super::CreditUsageRefreshError::Unavailable(reason)) = refresh_result {
                    if let Some(options) = account_options.write().as_mut()
                        && refresh_command_is_current(
                            Some(options),
                            &account_id,
                            credential_generation,
                            session_generation,
                            operation_generation,
                        )
                    {
                        options.message = Some(AccountOptionsMessage::RefreshUnavailable(reason));
                    }
                    continue;
                }
                let _reload_guard = reload_lock.lock().await;
                let refreshed_view_model = match loader.as_ref() {
                    Some(loader) => loader().await,
                    None => None,
                };
                let reload_succeeded = refreshed_view_model.is_some();
                if let Some(next_view_model) = refreshed_view_model {
                    install_reloaded_view_model(
                        next_view_model,
                        &mut view_model,
                        &mut account_options,
                        &reset_snapshot,
                        &mut reset_target,
                        reset_intent_sender.as_ref(),
                    );
                }
                if let Some(options) = account_options.write().as_mut()
                    && refresh_command_is_current(
                        Some(options),
                        &account_id,
                        credential_generation,
                        session_generation,
                        operation_generation,
                    )
                {
                    options.message = if !reload_succeeded || refresh_result.is_err() {
                        Some(AccountOptionsMessage::RefreshFailed)
                    } else {
                        Some(AccountOptionsMessage::Refreshed)
                    };
                }
            }
        }
    }
}

fn tab_switch_is_current(
    options: Option<&AccountOptionsState>,
    account_id: &codex_router_core::ids::AccountId,
    credential_generation: Option<u64>,
    session_generation: u64,
    operation_generation: u64,
    tab: AccountOptionsTab,
    request_id: InspectionTabRequestId,
) -> bool {
    options.is_some_and(|options| {
        options.session_generation == session_generation
            && options.target.account_id == *account_id
            && options.target.credential_generation == credential_generation
            && options.operation_generation == operation_generation
            && options.pending_tab == Some(tab)
            && options.pending_tab_request == Some(request_id)
    })
}

async fn wait_for_inspection_tab_ack(
    receiver: &mut watch::Receiver<ResetWorkflowSnapshot>,
    request_id: InspectionTabRequestId,
) -> Option<ResetWorkflowSnapshot> {
    loop {
        let snapshot = receiver.borrow_and_update().clone();
        match snapshot.last_processed_inspection_tab_request() {
            Some(processed) if processed == request_id => return Some(snapshot),
            Some(processed) if processed > request_id => return None,
            _ => {}
        }
        if receiver.changed().await.is_err() {
            return None;
        }
    }
}

fn reset_target_for_options(options: &AccountOptionsState) -> Option<ResetPaneTarget> {
    let target = &options.target;
    if !target.enabled {
        return None;
    }
    let active_credential_generation = target.credential_generation?;
    Some(ResetPaneTarget {
        account_id: target.account_id.clone(),
        active_credential_generation,
        account_label: target.account_label.clone(),
        account_tag: target.account_tag.clone(),
        saved_reset_credits: target.saved_reset_credits.clone(),
        saved_weekly_window: target.saved_weekly_window.clone(),
    })
}

fn current_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn saved_reload_is_current(
    options: Option<&AccountOptionsState>,
    account_id: &codex_router_core::ids::AccountId,
    credential_generation: Option<u64>,
    session_generation: u64,
    operation_generation: u64,
) -> bool {
    options.is_some_and(|options| {
        options.session_generation == session_generation
            && options.target.account_id == *account_id
            && options.target.credential_generation == credential_generation
            && options.editor.as_ref().is_some_and(|editor| {
                editor.operation_generation == operation_generation
                    && editor.phase == CreditPolicyEditorPhase::SavedReloadFailed
            })
    })
}

fn editor_command_is_current(
    options: Option<&AccountOptionsState>,
    account_id: &codex_router_core::ids::AccountId,
    credential_generation: Option<u64>,
    session_generation: u64,
    operation_generation: u64,
) -> bool {
    options.is_some_and(|options| {
        options.session_generation == session_generation
            && options.target.account_id == *account_id
            && options.target.credential_generation == credential_generation
            && options.editor.as_ref().is_some_and(|editor| {
                editor.operation_generation == operation_generation
                    && editor.phase == CreditPolicyEditorPhase::Saving
            })
    })
}

fn refresh_command_is_current(
    options: Option<&AccountOptionsState>,
    account_id: &codex_router_core::ids::AccountId,
    credential_generation: Option<u64>,
    session_generation: u64,
    operation_generation: u64,
) -> bool {
    options.is_some_and(|options| {
        options.session_generation == session_generation
            && options.target.account_id == *account_id
            && options.target.credential_generation == credential_generation
            && options.operation_generation == operation_generation
            && options.message == Some(AccountOptionsMessage::Refreshing)
    })
}

pub(in crate::presentation::quota) fn install_reloaded_view_model(
    next_view_model: QuotaStatusViewModel,
    view_model: &mut State<QuotaStatusViewModel>,
    account_options: &mut State<Option<AccountOptionsState>>,
    reset_snapshot: &State<Option<ResetWorkflowSnapshot>>,
    reset_target: &mut State<Option<ResetPaneTarget>>,
    reset_intent_sender: Option<&ResetIntentSender>,
) {
    view_model.set(next_view_model.clone());
    let mut updated_options = account_options.read().clone();
    let target_remains_current =
        update_account_options_from_report(&mut updated_options, &next_view_model);
    account_options.set(updated_options);
    if target_remains_current || reset_target.read().is_none() {
        return;
    }
    let phase = reset_snapshot
        .read()
        .as_ref()
        .map_or(WorkflowPhase::Browse, ResetWorkflowSnapshot::phase);
    if phase == WorkflowPhase::Committing || phase == WorkflowPhase::Result {
        return;
    }
    if let Some(sender) = reset_intent_sender {
        let _cancelled = sender.send_now(ResetSessionIntent::Cancel);
    }
    reset_target.set(None);
}

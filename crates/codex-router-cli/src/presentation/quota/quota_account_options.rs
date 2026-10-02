use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;

use codex_router_core::credit_usage::CreditAvailability;
use codex_router_core::credit_usage::CreditUsagePolicy;
use codex_router_core::ids::AccountId;
use iocraft::prelude::KeyCode;
use iocraft::prelude::KeyModifiers;

use super::quota_status_view_model::QuotaStatusAccountViewModel;
use super::quota_status_view_model::QuotaStatusViewModel;
use crate::quota::CreditUsageFreshness;
use crate::quota::CreditUsageStatus;
use crate::quota_reset::reset_session_supervisor::InspectionTabRequestId;

pub(crate) type CreditUsagePolicySaver = Arc<
    dyn Fn(AccountId, Option<u64>, CreditUsagePolicy) -> CreditUsagePolicySaveFuture + Send + Sync,
>;
pub(crate) type CreditUsagePolicySaveFuture =
    Pin<Box<dyn Future<Output = Result<CreditUsagePolicy, CreditUsagePolicySaveError>> + Send>>;
pub(crate) type CreditUsageRefresher =
    Arc<dyn Fn(AccountId, Option<u64>) -> CreditUsageRefreshFuture + Send + Sync>;
pub(crate) type CreditUsageRefreshFuture =
    Pin<Box<dyn Future<Output = Result<(), CreditUsageRefreshError>> + Send>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CreditUsagePolicySaveError {
    AccountUnavailable,
    TargetChanged,
    DatabaseBusy,
    SchemaUpgradeRequired,
    StateOperationFailed,
}

impl CreditUsagePolicySaveError {
    pub(super) const fn message(self) -> &'static str {
        match self {
            Self::AccountUnavailable => "account is no longer available",
            Self::TargetChanged => "account credentials changed; reopen account options",
            Self::DatabaseBusy => "database busy; retry save",
            Self::SchemaUpgradeRequired => "router database needs an upgrade",
            Self::StateOperationFailed => "credit preference save failed",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CreditUsageRefreshError {
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum AccountOptionsTab {
    Resets,
    Credits,
}

impl AccountOptionsTab {
    pub(super) const fn other(self) -> Self {
        match self {
            Self::Resets => Self::Credits,
            Self::Credits => Self::Resets,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct AccountOptionsTarget {
    pub(super) account_id: AccountId,
    pub(super) credential_generation: Option<u64>,
    pub(super) account_label: String,
    pub(super) account_tag: String,
    pub(super) enabled: bool,
    pub(super) credit_usage: CreditUsageStatus,
    pub(super) weekly_quota_floor_percent: u16,
    pub(super) saved_reset_credits: String,
    pub(super) saved_weekly_window: String,
}

impl AccountOptionsTarget {
    fn from_row(row: &QuotaStatusAccountViewModel) -> Self {
        Self {
            account_id: row.account_id.clone(),
            credential_generation: row.active_credential_generation,
            account_label: row.account.clone(),
            account_tag: row.account_tag.clone(),
            enabled: row.enabled,
            credit_usage: row.credit_usage.clone(),
            weekly_quota_floor_percent: row.weekly_quota_floor_percent,
            saved_reset_credits: row.reset_credits.clone(),
            saved_weekly_window: row.weekly_window.clone(),
        }
    }

    pub(super) fn matches_row(&self, row: &QuotaStatusAccountViewModel) -> bool {
        self.account_id == row.account_id
            && self.credential_generation == row.active_credential_generation
    }

    pub(super) fn update_from_row(&mut self, row: &QuotaStatusAccountViewModel) {
        self.account_label.clone_from(&row.account);
        self.account_tag.clone_from(&row.account_tag);
        self.enabled = row.enabled;
        self.credit_usage.clone_from(&row.credit_usage);
        self.weekly_quota_floor_percent = row.weekly_quota_floor_percent;
        self.saved_reset_credits.clone_from(&row.reset_credits);
        self.saved_weekly_window.clone_from(&row.weekly_window);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CreditPolicyEditorPhase {
    Editing,
    Saving,
    SaveFailed(CreditUsagePolicySaveError),
    SavedReloadFailed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct CreditPolicyEditorState {
    pub(super) saved_policy: CreditUsagePolicy,
    pub(super) draft_policy: CreditUsagePolicy,
    pub(super) phase: CreditPolicyEditorPhase,
    pub(super) operation_generation: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum AccountOptionsMessage {
    Refreshing,
    RefreshFailed,
    Refreshed,
    ResetReviewActive,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct AccountOptionsState {
    pub(super) target: AccountOptionsTarget,
    pub(super) tab: AccountOptionsTab,
    pub(super) session_generation: u64,
    pub(super) operation_generation: u64,
    pub(super) editor: Option<CreditPolicyEditorState>,
    pub(super) message: Option<AccountOptionsMessage>,
    pub(super) pending_tab: Option<AccountOptionsTab>,
    pub(super) pending_tab_request: Option<InspectionTabRequestId>,
}

impl AccountOptionsState {
    pub(super) fn new(row: &QuotaStatusAccountViewModel, session_generation: u64) -> Self {
        Self {
            target: AccountOptionsTarget::from_row(row),
            tab: AccountOptionsTab::Resets,
            session_generation,
            operation_generation: 0,
            editor: None,
            message: None,
            pending_tab: None,
            pending_tab_request: None,
        }
    }

    fn allocate_operation_generation(&mut self) -> u64 {
        self.operation_generation = self.operation_generation.saturating_add(1);
        self.operation_generation
    }

    fn start_editing(&mut self) {
        let generation = self.allocate_operation_generation();
        let saved_policy = self.target.credit_usage.policy;
        self.editor = Some(CreditPolicyEditorState {
            saved_policy,
            draft_policy: saved_policy,
            phase: CreditPolicyEditorPhase::Editing,
            operation_generation: generation,
        });
        self.message = None;
    }

    fn adjust_policy(&mut self, allow: bool) {
        if let Some(editor) = self.editor.as_mut()
            && matches!(
                editor.phase,
                CreditPolicyEditorPhase::Editing | CreditPolicyEditorPhase::SaveFailed(_)
            )
        {
            editor.draft_policy = if allow {
                CreditUsagePolicy::Allow
            } else {
                CreditUsagePolicy::Disallow
            };
            editor.phase = CreditPolicyEditorPhase::Editing;
        }
    }

    fn start_saving(&mut self) -> Option<AccountOptionsCommand> {
        let editor = self.editor.as_mut()?;
        if !matches!(
            editor.phase,
            CreditPolicyEditorPhase::Editing | CreditPolicyEditorPhase::SaveFailed(_)
        ) {
            return None;
        }
        editor.phase = CreditPolicyEditorPhase::Saving;
        self.message = None;
        Some(AccountOptionsCommand::SavePolicy {
            account_id: self.target.account_id.clone(),
            credential_generation: self.target.credential_generation,
            session_generation: self.session_generation,
            operation_generation: editor.operation_generation,
            policy: editor.draft_policy,
        })
    }

    fn retry_saved_reload(&mut self) -> Option<AccountOptionsCommand> {
        let editor = self.editor.as_ref()?;
        if editor.phase != CreditPolicyEditorPhase::SavedReloadFailed {
            return None;
        }
        Some(AccountOptionsCommand::ReloadAfterSave {
            account_id: self.target.account_id.clone(),
            credential_generation: self.target.credential_generation,
            session_generation: self.session_generation,
            operation_generation: editor.operation_generation,
        })
    }

    fn start_refresh(&mut self) -> AccountOptionsCommand {
        let operation_generation = self.allocate_operation_generation();
        self.message = Some(AccountOptionsMessage::Refreshing);
        AccountOptionsCommand::RefreshCredits {
            account_id: self.target.account_id.clone(),
            credential_generation: self.target.credential_generation,
            session_generation: self.session_generation,
            operation_generation,
        }
    }

    fn request_tab_switch(
        &mut self,
        tab: AccountOptionsTab,
        expected_inspection_attempt: Option<u64>,
    ) -> Option<AccountOptionsCommand> {
        if self.pending_tab.is_some() || self.tab == tab {
            return None;
        }
        let operation_generation = self.allocate_operation_generation();
        let request_id = InspectionTabRequestId::new(self.session_generation, operation_generation);
        if self.editor.as_ref().is_some_and(|editor| {
            !matches!(editor.phase, CreditPolicyEditorPhase::SavedReloadFailed)
        }) {
            self.editor = None;
        }
        self.pending_tab = Some(tab);
        self.pending_tab_request = Some(request_id);
        self.message = None;
        Some(AccountOptionsCommand::SwitchTab {
            account_id: self.target.account_id.clone(),
            credential_generation: self.target.credential_generation,
            session_generation: self.session_generation,
            operation_generation,
            tab,
            request_id,
            expected_inspection_attempt,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum AccountOptionsCommand {
    SwitchTab {
        account_id: AccountId,
        credential_generation: Option<u64>,
        session_generation: u64,
        operation_generation: u64,
        tab: AccountOptionsTab,
        request_id: InspectionTabRequestId,
        expected_inspection_attempt: Option<u64>,
    },
    SavePolicy {
        account_id: AccountId,
        credential_generation: Option<u64>,
        session_generation: u64,
        operation_generation: u64,
        policy: CreditUsagePolicy,
    },
    ReloadAfterSave {
        account_id: AccountId,
        credential_generation: Option<u64>,
        session_generation: u64,
        operation_generation: u64,
    },
    RefreshCredits {
        account_id: AccountId,
        credential_generation: Option<u64>,
        session_generation: u64,
        operation_generation: u64,
    },
}

#[derive(Clone)]
pub(super) struct AccountOptionsCommandPort {
    sender: tokio::sync::mpsc::UnboundedSender<AccountOptionsCommand>,
    receiver: Arc<Mutex<Option<tokio::sync::mpsc::UnboundedReceiver<AccountOptionsCommand>>>>,
}

impl AccountOptionsCommandPort {
    pub(super) fn new() -> Self {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        Self {
            sender,
            receiver: Arc::new(Mutex::new(Some(receiver))),
        }
    }

    pub(super) fn send(&self, command: AccountOptionsCommand) -> bool {
        self.sender.send(command).is_ok()
    }

    pub(super) fn take_receiver(
        &self,
    ) -> Option<tokio::sync::mpsc::UnboundedReceiver<AccountOptionsCommand>> {
        self.receiver
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum AccountOptionsKeyAction {
    Ignore,
    ResetWorkflow,
    ExitPending,
    Close,
    SwitchTab(AccountOptionsTab),
    BeginEditing,
    SelectAllow,
    SelectDisallow,
    SavePolicy,
    RetryReload,
    RefreshCredits,
    CancelEditing,
    BeginResetInspection,
}

pub(super) fn account_options_key_action(
    state: &AccountOptionsState,
    reset_phase: crate::quota_reset::reset_session_supervisor::WorkflowPhase,
    code: KeyCode,
    modifiers: KeyModifiers,
) -> AccountOptionsKeyAction {
    if reset_phase_is_review_or_later(reset_phase) {
        return if state.tab == AccountOptionsTab::Resets {
            AccountOptionsKeyAction::ResetWorkflow
        } else {
            AccountOptionsKeyAction::Ignore
        };
    }
    if state.pending_tab.is_some() {
        if code == KeyCode::Esc {
            return AccountOptionsKeyAction::Close;
        }
        if reset_phase != crate::quota_reset::reset_session_supervisor::WorkflowPhase::Browse
            && super::quota_status_component::forced_quota_exit_key(&code, modifiers)
        {
            return AccountOptionsKeyAction::ResetWorkflow;
        }
        if reset_phase == crate::quota_reset::reset_session_supervisor::WorkflowPhase::Browse
            && (code == KeyCode::Char('q')
                || super::quota_status_component::forced_quota_exit_key(&code, modifiers))
        {
            return AccountOptionsKeyAction::ExitPending;
        }
        return AccountOptionsKeyAction::Ignore;
    }
    if super::quota_status_component::forced_quota_exit_key(&code, modifiers) {
        return if reset_phase == crate::quota_reset::reset_session_supervisor::WorkflowPhase::Browse
        {
            AccountOptionsKeyAction::ExitPending
        } else {
            AccountOptionsKeyAction::ResetWorkflow
        };
    }
    if let Some(editor) = state.editor.as_ref() {
        return match (editor.phase, code) {
            (
                CreditPolicyEditorPhase::Editing | CreditPolicyEditorPhase::SaveFailed(_),
                KeyCode::Left,
            ) => AccountOptionsKeyAction::SelectDisallow,
            (
                CreditPolicyEditorPhase::Editing | CreditPolicyEditorPhase::SaveFailed(_),
                KeyCode::Right,
            ) => AccountOptionsKeyAction::SelectAllow,
            (
                CreditPolicyEditorPhase::Editing | CreditPolicyEditorPhase::SaveFailed(_),
                KeyCode::Enter,
            ) => AccountOptionsKeyAction::SavePolicy,
            (CreditPolicyEditorPhase::SavedReloadFailed, KeyCode::Enter) => {
                AccountOptionsKeyAction::RetryReload
            }
            (
                CreditPolicyEditorPhase::Editing | CreditPolicyEditorPhase::SaveFailed(_),
                KeyCode::Esc,
            ) => AccountOptionsKeyAction::CancelEditing,
            (CreditPolicyEditorPhase::SavedReloadFailed, KeyCode::Esc) => {
                AccountOptionsKeyAction::Close
            }
            (
                CreditPolicyEditorPhase::Editing
                | CreditPolicyEditorPhase::SaveFailed(_)
                | CreditPolicyEditorPhase::SavedReloadFailed,
                KeyCode::Tab,
            ) => AccountOptionsKeyAction::SwitchTab(state.tab.other()),
            (_, KeyCode::Esc) => AccountOptionsKeyAction::CancelEditing,
            _ => AccountOptionsKeyAction::Ignore,
        };
    }

    match (state.tab, code) {
        (_, KeyCode::Tab) => AccountOptionsKeyAction::SwitchTab(state.tab.other()),
        (AccountOptionsTab::Credits, KeyCode::Left) => {
            AccountOptionsKeyAction::SwitchTab(AccountOptionsTab::Resets)
        }
        (AccountOptionsTab::Credits, KeyCode::Right) => {
            AccountOptionsKeyAction::SwitchTab(AccountOptionsTab::Resets)
        }
        (AccountOptionsTab::Resets, KeyCode::Left | KeyCode::Right)
            if reset_phase
                == crate::quota_reset::reset_session_supervisor::WorkflowPhase::Confirming =>
        {
            AccountOptionsKeyAction::ResetWorkflow
        }
        (AccountOptionsTab::Resets, KeyCode::Left) => {
            AccountOptionsKeyAction::SwitchTab(AccountOptionsTab::Credits)
        }
        (AccountOptionsTab::Resets, KeyCode::Right) => {
            AccountOptionsKeyAction::SwitchTab(AccountOptionsTab::Credits)
        }
        (AccountOptionsTab::Credits, KeyCode::Enter) => AccountOptionsKeyAction::BeginEditing,
        (AccountOptionsTab::Credits, KeyCode::Char('r'))
            if !modifiers.contains(KeyModifiers::CONTROL) =>
        {
            AccountOptionsKeyAction::RefreshCredits
        }
        (AccountOptionsTab::Resets, KeyCode::Enter)
            if reset_phase
                == crate::quota_reset::reset_session_supervisor::WorkflowPhase::Browse =>
        {
            if state.target.enabled && state.target.credential_generation.is_some() {
                AccountOptionsKeyAction::BeginResetInspection
            } else {
                AccountOptionsKeyAction::Ignore
            }
        }
        (AccountOptionsTab::Resets, KeyCode::Esc)
            if reset_phase
                != crate::quota_reset::reset_session_supervisor::WorkflowPhase::Browse =>
        {
            AccountOptionsKeyAction::ResetWorkflow
        }
        (_, KeyCode::Esc) => AccountOptionsKeyAction::Close,
        (AccountOptionsTab::Resets, _) => AccountOptionsKeyAction::ResetWorkflow,
        (AccountOptionsTab::Credits, _) => AccountOptionsKeyAction::Ignore,
    }
}

fn reset_phase_is_review_or_later(
    phase: crate::quota_reset::reset_session_supervisor::WorkflowPhase,
) -> bool {
    matches!(
        phase,
        crate::quota_reset::reset_session_supervisor::WorkflowPhase::Confirming
            | crate::quota_reset::reset_session_supervisor::WorkflowPhase::Revalidating
            | crate::quota_reset::reset_session_supervisor::WorkflowPhase::Committing
            | crate::quota_reset::reset_session_supervisor::WorkflowPhase::Result
    )
}

pub(super) fn update_credit_policy_editor(
    state: &mut AccountOptionsState,
    action: AccountOptionsKeyAction,
    expected_inspection_attempt: Option<u64>,
) -> Option<AccountOptionsCommand> {
    match action {
        AccountOptionsKeyAction::BeginEditing => state.start_editing(),
        AccountOptionsKeyAction::SelectAllow => state.adjust_policy(true),
        AccountOptionsKeyAction::SelectDisallow => state.adjust_policy(false),
        AccountOptionsKeyAction::SavePolicy => return state.start_saving(),
        AccountOptionsKeyAction::RetryReload => return state.retry_saved_reload(),
        AccountOptionsKeyAction::RefreshCredits => return Some(state.start_refresh()),
        AccountOptionsKeyAction::CancelEditing => {
            if state
                .editor
                .as_ref()
                .is_some_and(|editor| editor.phase != CreditPolicyEditorPhase::Saving)
            {
                state.editor = None;
                state.message = None;
            }
        }
        AccountOptionsKeyAction::SwitchTab(tab) => {
            return state.request_tab_switch(tab, expected_inspection_attempt);
        }
        AccountOptionsKeyAction::Close => {
            state.editor = None;
            state.message = None;
        }
        AccountOptionsKeyAction::Ignore
        | AccountOptionsKeyAction::ExitPending
        | AccountOptionsKeyAction::ResetWorkflow
        | AccountOptionsKeyAction::BeginResetInspection => {}
    }
    None
}

pub(super) fn update_account_options_from_report(
    state: &mut Option<AccountOptionsState>,
    view_model: &QuotaStatusViewModel,
) -> bool {
    let Some(options) = state.as_mut() else {
        return true;
    };
    let Some(row) = view_model
        .rows
        .iter()
        .find(|row| row.account_id == options.target.account_id)
    else {
        *state = None;
        return false;
    };
    if !options.target.matches_row(row) {
        *state = None;
        return false;
    }
    options.target.update_from_row(row);
    true
}

pub(super) fn account_options_footer(state: &AccountOptionsState) -> String {
    if state.pending_tab.is_some() {
        return "tab change pending  esc back  q/ctrl-c exit".to_owned();
    }
    match (state.tab, state.editor.as_ref()) {
        (AccountOptionsTab::Resets, _)
            if state.target.enabled && state.target.credential_generation.is_some() =>
        {
            "enter inspect  tab credits  esc back".to_owned()
        }
        (AccountOptionsTab::Resets, _) => "tab credits  esc back".to_owned(),
        (AccountOptionsTab::Credits, None) => {
            "tab resets  enter edit  r refresh  esc back".to_owned()
        }
        (AccountOptionsTab::Credits, Some(editor)) => match editor.phase {
            CreditPolicyEditorPhase::Editing | CreditPolicyEditorPhase::SaveFailed(_) => {
                "←/→ choose  enter save  esc cancel".to_owned()
            }
            CreditPolicyEditorPhase::Saving => "saving credit policy".to_owned(),
            CreditPolicyEditorPhase::SavedReloadFailed => {
                "enter retry refresh  esc close options".to_owned()
            }
        },
    }
}

pub(super) use commands::AccountOptionsCommandContext;
pub(super) use commands::install_reloaded_view_model;
pub(super) use commands::run_account_options_commands;
pub(super) use events::AccountOptionsKeyEventContext;
pub(super) use events::handle_account_options_key_event;
pub(super) use events::open_account_options_for_account;
pub(super) use rendering::account_options_content_height;
pub(super) use rendering::account_options_inspection_footer;
#[path = "quota_account_options_commands.rs"]
mod commands;
#[path = "quota_account_options_events.rs"]
mod events;
#[path = "quota_account_options_rendering.rs"]
mod rendering;
pub(super) use rendering::AccountOptionsPanelProps;
pub(super) use rendering::render_account_options_panel;

pub(crate) fn credit_usage_compact_summary(usage: &CreditUsageStatus) -> String {
    let freshness = match usage.freshness {
        CreditUsageFreshness::Fresh => "fresh",
        CreditUsageFreshness::Stale => "stale",
        CreditUsageFreshness::Unknown => "unknown",
    };
    match usage.provider_observation.availability() {
        CreditAvailability::Unknown => "unknown".to_owned(),
        CreditAvailability::Depleted => format!("depleted {freshness}"),
        CreditAvailability::Unlimited => format!("unlimited {freshness}"),
        CreditAvailability::Available {
            balance: Some(balance),
        } => format!("{} {freshness}", balance.as_str()),
        CreditAvailability::Available { balance: None } => format!("available {freshness}"),
    }
}

#[cfg(test)]
#[path = "quota_account_options_test.rs"]
mod tests;

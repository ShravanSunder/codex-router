use codex_router_core::ids::AccountId;
use codex_router_core::provider::Provider;
use iocraft::prelude::*;

use super::quota_account_options::AccountOptionsState;
use super::quota_floor_editor::WeeklyFloorEditorState;
use super::quota_reset_presentation_model::ResetPaneTarget;
use super::quota_status_view_model::{QuotaStatusAccountViewModel, QuotaStatusViewModel};
use crate::quota_reset::reset_session_supervisor::{ResetWorkflowSnapshot, WorkflowPhase};

pub(super) fn quota_filter_modal_owns_input(
    account_options: Option<&AccountOptionsState>,
    floor_editor: Option<&WeeklyFloorEditorState>,
    reset_snapshot: Option<&ResetWorkflowSnapshot>,
    reset_target: Option<&ResetPaneTarget>,
) -> bool {
    account_options.is_some()
        || floor_editor.is_some()
        || reset_target.is_some()
        || reset_snapshot.is_some_and(|snapshot| snapshot.phase() != WorkflowPhase::Browse)
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum QuotaInteractionMode {
    #[default]
    Interactive,
    Static,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum AccountProviderFilter {
    #[default]
    All,
    Provider(Provider),
}

impl AccountProviderFilter {
    pub(super) const CHOICES: [Self; 3] = [
        Self::All,
        Self::Provider(Provider::Openai),
        Self::Provider(Provider::Claude),
    ];

    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Provider(Provider::Openai) => "OpenAI",
            Self::Provider(Provider::Claude) => "Claude",
        }
    }

    fn moved(self, forward: bool) -> Self {
        match (self, forward) {
            (Self::All, true) => Self::Provider(Provider::Openai),
            (Self::Provider(Provider::Openai), true) => Self::Provider(Provider::Claude),
            (Self::Provider(Provider::Openai), false) => Self::All,
            (Self::Provider(Provider::Claude), false) => Self::Provider(Provider::Openai),
            (Self::All, false) | (Self::Provider(Provider::Claude), true) => self,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum DisabledAccountVisibility {
    #[default]
    Shown,
    Hidden,
}

impl DisabledAccountVisibility {
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Shown => "Shown",
            Self::Hidden => "Hidden",
        }
    }

    pub(super) const fn toggled(self) -> Self {
        match self {
            Self::Shown => Self::Hidden,
            Self::Hidden => Self::Shown,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct QuotaAccountFilters {
    pub(super) provider: AccountProviderFilter,
    pub(super) disabled: DisabledAccountVisibility,
}

impl QuotaAccountFilters {
    fn includes_row(self, row: &QuotaStatusAccountViewModel) -> bool {
        (self.provider == AccountProviderFilter::All
            || self.provider == AccountProviderFilter::Provider(row.provider))
            && (self.disabled == DisabledAccountVisibility::Shown || row.enabled)
    }

    pub(super) fn visible_count(self, rows: &[QuotaStatusAccountViewModel]) -> usize {
        rows.iter().filter(|row| self.includes_row(row)).count()
    }

    pub(super) fn visible_rows(
        self,
        rows: &[QuotaStatusAccountViewModel],
    ) -> Vec<QuotaStatusAccountViewModel> {
        rows.iter()
            .filter(|row| self.includes_row(row))
            .cloned()
            .collect()
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum QuotaFilterFocus {
    #[default]
    Provider,
    Disabled,
    Apply,
    Cancel,
}

impl QuotaFilterFocus {
    fn moved(self, backwards: bool) -> Self {
        match (self, backwards) {
            (Self::Provider, false) | (Self::Apply, true) => Self::Disabled,
            (Self::Disabled, false) | (Self::Cancel, true) => Self::Apply,
            (Self::Apply, false) | (Self::Provider, true) => Self::Cancel,
            (Self::Cancel, false) | (Self::Disabled, true) => Self::Provider,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct QuotaFilterDraft {
    pub(super) filters: QuotaAccountFilters,
    pub(super) focus: QuotaFilterFocus,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct QuotaFilterState {
    pub(super) committed: QuotaAccountFilters,
    pub(super) draft: Option<QuotaFilterDraft>,
}

#[derive(Clone, Copy)]
pub(super) enum QuotaFilterAction {
    Open,
    Provider(AccountProviderFilter),
    ToggleDisabled,
    Apply,
    Cancel,
}

#[derive(Clone, Copy)]
pub(super) struct QuotaFilterController {
    pub(super) state: State<QuotaFilterState>,
    pub(super) view_model: State<QuotaStatusViewModel>,
    pub(super) focused_account_id: State<Option<AccountId>>,
}

impl QuotaFilterController {
    pub(super) fn dispatch(mut self, action: QuotaFilterAction) {
        let mut next = *self.state.read();
        match action {
            QuotaFilterAction::Open => {
                next.draft = Some(QuotaFilterDraft {
                    filters: next.committed,
                    focus: QuotaFilterFocus::Provider,
                })
            }
            QuotaFilterAction::Cancel => next.draft = None,
            QuotaFilterAction::Apply => {
                if let Some(draft) = next.draft.take() {
                    next.committed = draft.filters;
                    let rows = next.committed.visible_rows(&self.view_model.read().rows);
                    reconcile_quota_filter_focus(&rows, &mut self.focused_account_id);
                }
            }
            QuotaFilterAction::Provider(provider) => {
                if let Some(draft) = next.draft.as_mut() {
                    draft.filters.provider = provider;
                    draft.focus = QuotaFilterFocus::Provider;
                }
            }
            QuotaFilterAction::ToggleDisabled => {
                if let Some(draft) = next.draft.as_mut() {
                    draft.filters.disabled = draft.filters.disabled.toggled();
                    draft.focus = QuotaFilterFocus::Disabled;
                }
            }
        }
        self.state.set(next);
    }

    pub(super) fn handle_key(mut self, code: KeyCode, modifiers: KeyModifiers) -> bool {
        let Some(mut draft) = self.state.read().draft else {
            return false;
        };
        match code {
            KeyCode::Esc => self.dispatch(QuotaFilterAction::Cancel),
            KeyCode::Enter => self.dispatch(if draft.focus == QuotaFilterFocus::Cancel {
                QuotaFilterAction::Cancel
            } else {
                QuotaFilterAction::Apply
            }),
            KeyCode::Tab | KeyCode::BackTab => {
                draft.focus = draft
                    .focus
                    .moved(code == KeyCode::BackTab || modifiers.contains(KeyModifiers::SHIFT));
                let mut next = *self.state.read();
                next.draft = Some(draft);
                self.state.set(next);
            }
            KeyCode::Up | KeyCode::Down | KeyCode::Left | KeyCode::Right
                if draft.focus == QuotaFilterFocus::Provider =>
            {
                self.dispatch(QuotaFilterAction::Provider(
                    draft
                        .filters
                        .provider
                        .moved(matches!(code, KeyCode::Down | KeyCode::Right)),
                ))
            }
            KeyCode::Char(' ') if draft.focus == QuotaFilterFocus::Disabled => {
                self.dispatch(QuotaFilterAction::ToggleDisabled)
            }
            _ => {}
        }
        true
    }
}

pub(super) fn reconcile_quota_filter_focus(
    rows: &[QuotaStatusAccountViewModel],
    focused: &mut State<Option<AccountId>>,
) {
    let retained = focused
        .read()
        .as_ref()
        .is_some_and(|account_id| rows.iter().any(|row| &row.account_id == account_id));
    if !retained {
        let next = rows.first().map(|row| row.account_id.clone());
        if *focused.read() != next {
            focused.set(next);
        }
    }
}

pub(super) fn opens_quota_filters(code: KeyCode, modifiers: KeyModifiers) -> bool {
    code == KeyCode::F(3)
        || (code == KeyCode::Char('f') && modifiers.contains(KeyModifiers::CONTROL))
}

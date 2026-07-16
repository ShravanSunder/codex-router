use iocraft::prelude::KeyCode;
use iocraft::prelude::KeyModifiers;

use crate::quota_reset::supervisor::ConfirmationSelection;
use crate::quota_reset::supervisor::WorkflowPhase;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ResetKeyAction {
    None,
    Cancel,
    OpenConfirmation,
    SelectNo,
    SelectYes,
    Confirm,
    DismissResult,
    PreviousInventoryPage,
    NextInventoryPage,
    ExitPrecommit,
}

pub(super) fn reset_key_action(
    phase: WorkflowPhase,
    selection: ConfirmationSelection,
    yes_enabled: bool,
    code: KeyCode,
    modifiers: KeyModifiers,
) -> ResetKeyAction {
    if phase == WorkflowPhase::Browse || phase == WorkflowPhase::Committing {
        return ResetKeyAction::None;
    }
    if code == KeyCode::Esc
        || code == KeyCode::Char('r') && modifiers.contains(KeyModifiers::CONTROL)
    {
        return if phase == WorkflowPhase::Result {
            ResetKeyAction::DismissResult
        } else {
            ResetKeyAction::Cancel
        };
    }
    if matches!(
        code,
        KeyCode::Char('c' | 'd') | KeyCode::Char('\u{3}' | '\u{4}')
    ) && (modifiers.contains(KeyModifiers::CONTROL)
        || matches!(code, KeyCode::Char('\u{3}' | '\u{4}')))
    {
        return ResetKeyAction::ExitPrecommit;
    }
    match (phase, code) {
        (WorkflowPhase::Inspected, KeyCode::Enter) => ResetKeyAction::OpenConfirmation,
        (WorkflowPhase::Confirming, KeyCode::Enter) => {
            if selection == ConfirmationSelection::Yes && yes_enabled {
                ResetKeyAction::Confirm
            } else {
                ResetKeyAction::Cancel
            }
        }
        (WorkflowPhase::Confirming, KeyCode::Left) => ResetKeyAction::SelectNo,
        (WorkflowPhase::Confirming, KeyCode::Right) if yes_enabled => ResetKeyAction::SelectYes,
        (WorkflowPhase::Result, KeyCode::Enter) => ResetKeyAction::DismissResult,
        (_, KeyCode::PageUp) => ResetKeyAction::PreviousInventoryPage,
        (_, KeyCode::PageDown) => ResetKeyAction::NextInventoryPage,
        _ => ResetKeyAction::None,
    }
}

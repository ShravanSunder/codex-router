//! Details event effects: selection uses captured state; clipboard output stays here.
use super::details_dialog_state::{ClipboardRequestStatus, DetailsAction};
use super::session_id_clipboard::SessionIdCopier;
use super::{SessionsPickerOutcome, picker_model::SessionsPickerModel};
use crate::sessions::SessionActionSelection;
use iocraft::prelude::*;

pub(super) fn dispatch_details_action(
    mut model: State<SessionsPickerModel>,
    mut outcome: State<Option<SessionsPickerOutcome>>,
    action: DetailsAction,
    copier: &SessionIdCopier,
) {
    let selected = apply_details_action(&mut model.write(), action, copier);
    if let Some(selected) = selected {
        outcome.set(Some(selected));
    }
}

pub(super) fn apply_details_action(
    model: &mut SessionsPickerModel,
    action: DetailsAction,
    copy_session_id: &super::session_id_clipboard::SessionIdCopier,
) -> Option<SessionsPickerOutcome> {
    if model.source_action.pending.is_some() || model.show_help {
        return None;
    }
    let dialog = model.details_dialog.clone()?;
    if !dialog.actions().contains(&action) {
        return None;
    }
    match action {
        DetailsAction::Close => model.details_dialog = None,
        DetailsAction::StartNew => {
            model.details_dialog = None;
            return model.start_new_action();
        }
        DetailsAction::Resume => {
            let record = dialog.record()?;
            let selection = SessionActionSelection::from_picker_record(record);
            if model.begin_source_selection(&selection, false, record.cwd.clone()) {
                return None;
            }
            model.details_dialog = None;
            return Some(SessionsPickerOutcome::ResumeSession(selection));
        }
        DetailsAction::Fork => {
            let record = dialog.record()?;
            model.fork_confirmation = Some(
                super::picker_fork_confirmation::ForkConfirmation::capture(record, &model.request),
            );
            model.details_dialog = None;
        }
        DetailsAction::CopySessionId => {
            let copied = copy_session_id(&dialog.record()?.session_id);
            if let Some(current) = &mut model.details_dialog {
                current.clipboard_status = if copied.is_ok() {
                    ClipboardRequestStatus::Sent
                } else {
                    ClipboardRequestStatus::Failed
                };
            }
        }
    }
    None
}

pub(super) fn handle_details_key(
    model: &mut SessionsPickerModel,
    code: KeyCode,
    modifiers: KeyModifiers,
    copier: &SessionIdCopier,
) -> Option<SessionsPickerOutcome> {
    let control_enter = code == KeyCode::Enter
        && modifiers.contains(KeyModifiers::CONTROL)
        && !modifiers.intersects(KeyModifiers::ALT | KeyModifiers::SUPER);
    let action = if control_enter && modifiers.contains(KeyModifiers::SHIFT) {
        Some(DetailsAction::Fork)
    } else if control_enter {
        Some(
            if model.details_dialog.as_ref().is_some_and(|dialog| {
                matches!(
                    dialog.target,
                    super::details_dialog_state::SessionDetailsTarget::NewSession
                )
            }) {
                DetailsAction::StartNew
            } else {
                DetailsAction::Resume
            },
        )
    } else {
        match code {
            KeyCode::Esc => Some(DetailsAction::Close),
            KeyCode::Tab | KeyCode::BackTab => {
                if let Some(dialog) = &mut model.details_dialog {
                    dialog.move_focus(
                        code == KeyCode::BackTab || modifiers.contains(KeyModifiers::SHIFT),
                    );
                }
                None
            }
            KeyCode::Enter | KeyCode::Char(' ') if modifiers.is_empty() => model
                .details_dialog
                .as_ref()
                .map(|dialog| dialog.focused_action),
            _ => None,
        }
    };
    action.and_then(|action| apply_details_action(model, action, copier))
}

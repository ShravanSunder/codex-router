//! Shared dialog chrome with session-owned, captured Details actions.
use super::super::details_dialog_state::{ClipboardRequestStatus, SessionDetailsTarget};
use super::super::session_id_clipboard::SessionIdCopier;
use super::*;
use codex_router_tui::{
    DialogAction, DialogFrame, DialogKeyboardOwner, DialogMargins, DialogPresentation, UiText,
    UiTextRole,
};

pub(super) fn render_details_dialog(
    model: &SessionsPickerModel,
    model_state: State<SessionsPickerModel>,
    selected_outcome: State<Option<SessionsPickerOutcome>>,
    selected_conversation: Option<&SessionConversationPreview>,
    width: u16,
    height: u16,
    copier: SessionIdCopier,
) -> AnyElement<'static> {
    let Some(dialog) = model.details_dialog.as_ref() else {
        return element!(View).into_any();
    };
    let title = match &dialog.target {
        SessionDetailsTarget::Session(record) => format!("Session details · {}", record.title),
        SessionDetailsTarget::NewSession => "New session details".to_owned(),
    };
    let expected_target = dialog.target.clone();
    let actions = dialog.actions().iter().map(|action| {
        let action = *action;
        let copier = copier.clone();
        let expected_target = expected_target.clone();
        element!(DialogAction(label: action.label(), has_focus: action == dialog.focused_action,
            keyboard_owner: DialogKeyboardOwner::Caller, handler: move |_| {
                let still_current = model_state.read().details_dialog.as_ref().is_some_and(|current| current.target == expected_target);
                if still_current {
                    super::super::details_dialog_actions::dispatch_details_action(model_state, selected_outcome, action, &copier);
                }
            })).into_any()
    }).collect::<Vec<_>>();
    let content_width = usize::from(width)
        .saturating_sub(4)
        .min(90)
        .saturating_sub(4);
    let content = match &dialog.target {
        SessionDetailsTarget::Session(record) => {
            render_details(record, content_width, selected_conversation, Size::Auto)
        }
        SessionDetailsTarget::NewSession => {
            render_start_new_details(model, content_width, START_NEW_DETAILS_HEIGHT)
        }
    };
    let notice = match dialog.clipboard_status {
        ClipboardRequestStatus::Idle => None,
        ClipboardRequestStatus::Sent => Some("Copy requested"),
        ClipboardRequestStatus::Failed => Some("Could not copy session ID"),
    };
    element! {
        DialogFrame(width, height, title, help_action: Some(super::super::picker_help_view::help_action(model_state)),
            presentation: if model.width < NARROW_PICKER_WIDTH { DialogPresentation::FullScreen } else { DialogPresentation::Framed { maximum_width: 100, maximum_height: 28 } },
            margins: DialogMargins { top: 1, right: 2, bottom: 1, left: 2 }, keyboard_scroll: true, actions) {
            #(content)
            #(notice.map(|notice| element!(UiText(content: notice, role: UiTextRole::Secondary))))
            #(super::super::source_action_view::source_error(model))
        }
    }.into_any()
}

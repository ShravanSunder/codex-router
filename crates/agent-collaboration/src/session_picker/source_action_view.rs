//! Checking is one modal over the retained picker or choice surface.
use super::{picker_model::SessionsPickerModel, source_action_state::PendingSourceAction};
use crate::sessions::source_ssh_protocol::SourceSessionAction;
use codex_router_tui::{DialogAction, DialogFrame, DialogPresentation, UiText, UiTextRole};
use iocraft::prelude::*;

pub(super) fn source_error(model: &SessionsPickerModel) -> Option<AnyElement<'static>> {
    model.source_action.error.map(|error| {
        element!(UiText(content: error.to_string(), role: UiTextRole::Secondary)).into_any()
    })
}

pub(super) fn checking_dialog(
    request: &PendingSourceAction,
    mut state: State<SessionsPickerModel>,
    height: usize,
) -> AnyElement<'static> {
    let model = state.read();
    let action = match &request.action {
        SourceSessionAction::New { .. } => "New Codex session",
        SourceSessionAction::Resume { .. } => "Resume session",
        SourceSessionAction::Fork { .. } => "Fork session",
    };
    element! {
        DialogFrame(presentation: if model.width < 72 { DialogPresentation::FullScreen } else { DialogPresentation::Framed { maximum_width: 72, maximum_height: 12 } },
            width: u16::try_from(model.width).unwrap_or(u16::MAX), height: u16::try_from(height).unwrap_or(u16::MAX), title: "Checking source".to_owned(),
            actions: vec![element!(DialogAction(label: "Cancel", handler: move |_| state.write().cancel_source_action())).into_any()]) {
            UiText(content: format!("{} · {action}", request.profile.name.as_str()), role: UiTextRole::Body)
            #(request.source_directory.as_ref().map(|directory| element!(UiText(content: format!("Source directory: {directory}"), role: UiTextRole::Secondary))))
            UiText(content: "Verifying the selected source before handoff".to_owned(), role: UiTextRole::Secondary)
        }
    }.into_any()
}

//! A Details action uses its captured record, never a refreshed background focus.
use super::picker_model::SessionsPickerModel;
use crate::sessions::SessionPickerRecord;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum SessionDetailsTarget {
    Session(Box<SessionPickerRecord>),
    NewSession,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum DetailsAction {
    Resume,
    Fork,
    CopySessionId,
    StartNew,
    Close,
}

impl DetailsAction {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Resume => "Resume",
            Self::Fork => "Fork",
            Self::CopySessionId => "Copy session ID",
            Self::StartNew => "Start",
            Self::Close => "Close",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum ClipboardRequestStatus {
    #[default]
    Idle,
    Sent,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct SessionDetailsDialog {
    pub(super) target: SessionDetailsTarget,
    pub(super) focused_action: DetailsAction,
    pub(super) clipboard_status: ClipboardRequestStatus,
}

impl SessionDetailsDialog {
    pub(super) fn capture(record: Option<&SessionPickerRecord>) -> Self {
        let target = record.map_or(SessionDetailsTarget::NewSession, |record| {
            SessionDetailsTarget::Session(Box::new(record.clone()))
        });
        let focused_action = match &target {
            SessionDetailsTarget::NewSession => DetailsAction::StartNew,
            SessionDetailsTarget::Session(record) if record.identity.is_provider() => {
                DetailsAction::CopySessionId
            }
            SessionDetailsTarget::Session(_) => DetailsAction::Resume,
        };
        Self {
            target,
            focused_action,
            clipboard_status: ClipboardRequestStatus::Idle,
        }
    }

    pub(super) fn record(&self) -> Option<&SessionPickerRecord> {
        match &self.target {
            SessionDetailsTarget::Session(record) => Some(record),
            SessionDetailsTarget::NewSession => None,
        }
    }

    pub(super) fn actions(&self) -> &'static [DetailsAction] {
        match &self.target {
            SessionDetailsTarget::NewSession => &[DetailsAction::StartNew, DetailsAction::Close],
            SessionDetailsTarget::Session(record) if record.identity.is_provider() => {
                &[DetailsAction::CopySessionId, DetailsAction::Close]
            }
            SessionDetailsTarget::Session(_) => &[
                DetailsAction::Resume,
                DetailsAction::Fork,
                DetailsAction::CopySessionId,
                DetailsAction::Close,
            ],
        }
    }

    pub(super) fn move_focus(&mut self, backwards: bool) {
        let actions = self.actions();
        let index = actions
            .iter()
            .position(|action| *action == self.focused_action)
            .unwrap_or(0);
        let next_index = if backwards {
            (index + actions.len() - 1) % actions.len()
        } else {
            (index + 1) % actions.len()
        };
        if let Some(action) = actions.get(next_index) {
            self.focused_action = *action;
        }
    }
}

impl SessionsPickerModel {
    pub(super) fn open_captured_details(&mut self) {
        self.details_dialog = Some(SessionDetailsDialog::capture(self.focused_record()));
    }
}

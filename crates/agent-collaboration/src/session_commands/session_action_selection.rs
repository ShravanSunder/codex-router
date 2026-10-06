//! Frozen source identity and model metadata for one selected session action.
use super::{SessionPickerIdentity, SessionPickerRecord};
use codex_native_integration::ResumeModelChoice;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SessionActionSelection {
    pub(crate) identity: SessionPickerIdentity,
    pub(crate) model_choice: ResumeModelChoice,
}

impl SessionActionSelection {
    pub(crate) fn from_picker_record(record: &SessionPickerRecord) -> Self {
        let model = record.model.as_deref();
        let effort = record.reasoning_effort.as_deref();
        if ResumeModelChoice::rejects_stored_value(model, effort) {
            eprintln!(
                "agent-sessions: stored model or reasoning effort for session {} contains characters that cannot be passed to Codex; resuming without it",
                record.session_id
            );
        }
        Self {
            identity: record.identity.clone(),
            model_choice: ResumeModelChoice::from_stored_values(model, effort),
        }
    }

    pub(crate) fn default_catalog(session_id: String, model_choice: ResumeModelChoice) -> Self {
        Self {
            identity: SessionPickerIdentity::LocalCodex(session_id),
            model_choice,
        }
    }

    pub(crate) fn session_id(&self) -> String {
        match &self.identity {
            SessionPickerIdentity::LocalCodex(session_id) => session_id.clone(),
            SessionPickerIdentity::HostedCodex(target)
            | SessionPickerIdentity::HostedProvider(target) => {
                String::from(target.session_id.clone())
            }
        }
    }
}

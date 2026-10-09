//! One guarded source-view publication, including read progress and optional record replacement.
use super::{PickerSourceContext, SourceInventoryRejection};
use crate::picker_runtime_status::PickerRecordsSnapshot;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum SourceReadState {
    Loading,
    Ready { record_count: usize },
    Rejected { reason: SourceInventoryRejection },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct SourceReadProgress {
    pub(super) source_context: PickerSourceContext,
    pub(super) read_state: SourceReadState,
}

impl SourceReadProgress {
    pub(super) fn source_name(&self) -> &str {
        match &self.source_context {
            PickerSourceContext::DefaultHosted => "This machine",
            PickerSourceContext::LocalCodex => "Local Codex",
            PickerSourceContext::ConfiguredHosted(profile) => profile.name.as_str(),
        }
    }
    pub(super) fn display_line(&self) -> String {
        let name = self.source_name();
        match self.read_state {
            SourceReadState::Loading => format!("{name}: Loading"),
            SourceReadState::Ready { record_count } => format!("{name}: {record_count} sessions"),
            SourceReadState::Rejected { reason } => format!("{name}: Unavailable ({reason})"),
        }
    }
}

pub(super) enum SourceRecordsUpdate {
    Pending,
    Ready(PickerRecordsSnapshot),
    Rejected(SourceInventoryRejection),
}

pub(super) struct SourceReloadUpdate {
    pub(super) source_progress: Vec<SourceReadProgress>,
    pub(super) records_update: SourceRecordsUpdate,
}

impl SourceReloadUpdate {
    #[cfg(test)]
    pub(super) fn into_snapshot(
        self,
    ) -> Option<Result<PickerRecordsSnapshot, SourceInventoryRejection>> {
        match self.records_update {
            SourceRecordsUpdate::Pending => None,
            SourceRecordsUpdate::Ready(snapshot) => Some(Ok(snapshot)),
            SourceRecordsUpdate::Rejected(reason) => Some(Err(reason)),
        }
    }
}

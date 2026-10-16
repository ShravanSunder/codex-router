//! One guarded source-view publication, including read progress and optional record replacement.
use super::{PickerSourceContext, SourceInventoryRejection};
use crate::picker_runtime_status::PickerRecordsSnapshot;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum SourceReadState {
    Loading,
    Complete { record_count: usize },
    Suspended { record_count: usize },
    LoadingMore { record_count: usize },
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
            SourceReadState::Complete { record_count } => {
                format!("{name}: {record_count} sessions")
            }
            SourceReadState::LoadingMore { record_count } => {
                format!("{name}: Loading ({record_count} fetched; unfinished)")
            }
            SourceReadState::Suspended { record_count } => {
                format!("{name}: Suspended ({record_count} fetched; unfinished)")
            }
            SourceReadState::Rejected { reason } => format!("{name}: Unavailable ({reason})"),
        }
    }
}

/// Aggregate only observed read states; a registry name is never an availability result.
pub(super) fn all_read_notice(progress: &[SourceReadProgress]) -> Option<String> {
    let mut loading = Vec::new();
    let mut unavailable = Vec::new();
    let mut unfinished = Vec::new();
    for source in progress {
        match source.read_state {
            SourceReadState::Loading => loading.push(source.source_name()),
            SourceReadState::Rejected { .. } => unavailable.push(source.source_name()),
            SourceReadState::LoadingMore { .. } | SourceReadState::Suspended { .. } => {
                unfinished.push(source.source_name())
            }
            SourceReadState::Complete { .. } => {}
        }
    }
    let partial = !unavailable.is_empty() || !unfinished.is_empty();
    let mut details = Vec::new();
    for (kind, names) in [
        ("unavailable", unavailable),
        ("unfinished", unfinished),
        ("loading", loading),
    ] {
        if !names.is_empty() {
            details.push(format!("{kind}: {}", names.join(", ")));
        }
    }
    if details.is_empty() {
        None
    } else {
        Some(format!(
            "{} — {}",
            if partial {
                "Partial view"
            } else {
                "Loading view"
            },
            details.join("; ")
        ))
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

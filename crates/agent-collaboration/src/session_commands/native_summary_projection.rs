//! Source facts only; native summaries never borrow invoking-machine paths or history.
use super::{
    SessionPathSpace, SessionPickerIdentity, SessionPickerRecord, SessionRecord,
    SessionRowProvenance,
};
use crate::picker_runtime_status::PickerRuntimeStatus;
use collaboration_client::protocol::{NativeSessionObservation, NativeSessionSummary};

impl SessionPickerRecord {
    pub(crate) fn from_native_summary(summary: &NativeSessionSummary) -> Self {
        let updated_at_ms = match &summary.observation {
            NativeSessionObservation::Stored { updated_at } => {
                chrono::DateTime::parse_from_rfc3339(&String::from(updated_at.clone()))
                    .ok()
                    .map(|timestamp| timestamp.timestamp_millis())
            }
            NativeSessionObservation::Runtime { .. } => None,
        };
        let mut record = Self::from_record_in_path_space(
            &SessionRecord {
                session_id: String::from(summary.target.session_id.clone()),
                rollout_path: None,
                cwd: Some(String::from(summary.working_directory.clone())),
                provider: None,
                model: summary.model.clone(),
                reasoning_effort: summary.reasoning_effort.clone(),
                source: None,
                thread_source: None,
                git_branch: summary.git_branch.clone(),
                git_origin_url: None,
                name: summary.name.clone(),
                title: Some(summary.title.clone()),
                preview: None,
                first_user_message: None,
                created_at_ms: None,
                updated_at_ms,
                recency_at_ms: updated_at_ms,
            },
            SessionPathSpace::SourceMachine,
        );
        record.identity = SessionPickerIdentity::HostedCodex(summary.target.clone());
        record.provenance = SessionRowProvenance::ObservedHosted;
        record.native_source = Some(summary.source);
        record.runtime_status = match &summary.observation {
            NativeSessionObservation::Stored { .. } => PickerRuntimeStatus::Unknown,
            NativeSessionObservation::Runtime { status, .. } => {
                PickerRuntimeStatus::from_native(status)
            }
        };
        record
    }
}

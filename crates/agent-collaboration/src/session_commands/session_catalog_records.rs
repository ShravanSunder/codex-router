//! CLI and picker projection of reusable stored-session records.

use super::{
    SESSION_CONVERSATION_SNIPPET_MAX_CHARS, display_title_from_session_fields,
    format_recency_at_ms, normalize_path, session_context_from_cwd, truncate_end,
};
use collaboration_client::protocol::{
    ClaudeCodeInteractiveStatus, EndpointRef, NativeSessionSource, ProviderSessionState,
    ProviderSessionSummary, SessionRef,
};
use collaboration_client::session_catalog::{
    SessionHistorySource, StoredSessionRecord, read_session_conversation_history,
};
use std::path::Path;

use crate::picker_runtime_status::PickerRuntimeStatus;

pub(super) type SessionRecord = StoredSessionRecord;
pub(crate) type SessionConversationSource = SessionHistorySource;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) enum SessionPickerIdentity {
    LocalCodex(String),
    HostedCodex(SessionRef),
    HostedProvider(SessionRef),
}

impl SessionPickerIdentity {
    pub(crate) fn is_provider(&self) -> bool {
        matches!(self, Self::HostedProvider(_))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SessionRowProvenance {
    LocalHomeCatalog,
    DefaultAttributed,
    ObservedHosted,
    ObservedProvider,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SessionPickerRecord {
    pub(crate) identity: SessionPickerIdentity,
    pub(crate) provenance: SessionRowProvenance,
    pub(crate) source_context: Option<crate::presentation::session_picker::PickerSourceContext>,
    pub(crate) machine_display_label: Option<String>,
    pub(crate) endpoint_label: Option<String>,
    pub(crate) provider_state: Option<ProviderSessionState>,
    pub(crate) session_id: String,
    pub(crate) title: String,
    pub(crate) full_title: String,
    pub(crate) explicit_name: Option<String>,
    pub(crate) recency: String,
    pub(crate) created: String,
    pub(crate) recency_at_ms: Option<i64>,
    pub(crate) created_at_ms: Option<i64>,
    pub(crate) branch: String,
    pub(crate) persisted_branch: String,
    pub(crate) context: String,
    pub(crate) cwd: Option<String>,
    pub(crate) normalized_cwd: Option<String>,
    pub(crate) git_origin_url: Option<String>,
    pub(crate) provider: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) reasoning_effort: Option<String>,
    pub(crate) preview: Option<String>,
    pub(crate) first_user_message: String,
    pub(crate) conversation: SessionConversationPreview,
    pub(crate) conversation_source: Option<SessionConversationSource>,
    pub(crate) source: Option<String>,
    pub(crate) thread_source: Option<String>,
    pub(crate) native_source: Option<NativeSessionSource>,
    pub(crate) runtime_status: PickerRuntimeStatus,
}

enum SessionPathSpace {
    InvokingMachine,
    SourceMachine,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SessionConversationPreview {
    pub(crate) snippets: Vec<String>,
    pub(crate) unavailable_reason: Option<String>,
}

impl SessionPickerRecord {
    pub(crate) fn machine_label(&self) -> &str {
        if let Some(label) = &self.machine_display_label {
            return label;
        }
        match &self.source_context {
            Some(crate::presentation::session_picker::PickerSourceContext::ConfiguredHosted(
                profile,
            )) => profile.name.as_str(),
            Some(crate::presentation::session_picker::PickerSourceContext::LocalCodex) => {
                "Local Codex"
            }
            Some(crate::presentation::session_picker::PickerSourceContext::DefaultHosted)
            | None => "This machine",
        }
    }
    pub(crate) fn matches_search(
        &self,
        expression: &collaboration_client::session_catalog::SessionSearchExpression,
    ) -> bool {
        let normalized_origin = self
            .git_origin_url
            .as_deref()
            .and_then(collaboration_client::board::normalize_git_origin_url)
            .unwrap_or_default();
        expression.matches(
            &collaboration_client::session_catalog::SessionSearchDocument {
                session_id: &self.session_id,
                name: self.explicit_name.as_deref().unwrap_or_default(),
                title: &self.full_title,
                preview: self.preview.as_deref().unwrap_or_default(),
                first_user_message: &self.first_user_message,
                branch: &self.persisted_branch,
                origin: &normalized_origin,
                cwd: self.cwd.as_deref().unwrap_or_default(),
            },
        )
    }

    pub(super) fn from_record(record: &SessionRecord) -> Self {
        Self::from_record_in_path_space(record, SessionPathSpace::InvokingMachine)
    }

    fn from_record_in_path_space(record: &SessionRecord, path_space: SessionPathSpace) -> Self {
        let display_title = display_title_from_session_fields(
            record.name.as_deref(),
            record.title.as_deref(),
            record.preview.as_deref(),
            record.first_user_message.as_deref(),
        )
        .unwrap_or_else(|| "Untitled session".to_owned());
        Self {
            identity: SessionPickerIdentity::LocalCodex(record.session_id.clone()),
            source_context: None,
            machine_display_label: None,
            provenance: SessionRowProvenance::LocalHomeCatalog,
            endpoint_label: None,
            provider_state: None,
            session_id: record.session_id.clone(),
            title: display_title,
            explicit_name: record.name.clone(),
            full_title: record.title.clone().unwrap_or_default(),
            recency: format_recency_at_ms(record.recency_at_ms),
            created: format_recency_at_ms(record.created_at_ms),
            recency_at_ms: record.recency_at_ms,
            created_at_ms: record.created_at_ms,
            branch: record.git_branch.clone().unwrap_or_else(|| "-".to_owned()),
            persisted_branch: record.git_branch.clone().unwrap_or_default(),
            context: record
                .cwd
                .as_deref()
                .map(session_context_from_cwd)
                .unwrap_or_else(|| "-".to_owned()),
            cwd: record.cwd.clone(),
            normalized_cwd: match path_space {
                SessionPathSpace::InvokingMachine => record.cwd.as_deref().map(|cwd| {
                    normalize_path(Path::new(cwd))
                        .to_string_lossy()
                        .into_owned()
                }),
                SessionPathSpace::SourceMachine => None,
            },
            git_origin_url: record.git_origin_url.clone(),
            provider: record.provider.clone(),
            model: record.model.clone(),
            reasoning_effort: record.reasoning_effort.clone(),
            preview: record.preview.clone(),
            first_user_message: record.first_user_message.clone().unwrap_or_default(),
            conversation: SessionConversationPreview::unavailable("history not loaded"),
            conversation_source: record.rollout_path.clone(),
            source: record.source.clone(),
            thread_source: record.thread_source.clone(),
            native_source: None,
            runtime_status: PickerRuntimeStatus::Unknown,
        }
    }

    pub(crate) fn with_hosted_codex(mut self, endpoint: &EndpointRef) -> Self {
        if let Ok(session_id) = self.session_id.clone().try_into() {
            self.provenance = SessionRowProvenance::DefaultAttributed;
            self.identity = SessionPickerIdentity::HostedCodex(SessionRef {
                endpoint: endpoint.clone(),
                session_id,
            });
        }
        self
    }

    pub(crate) fn from_provider_summary(
        summary: &ProviderSessionSummary,
        endpoint_label: &str,
    ) -> Self {
        let (
            target,
            working_directory,
            updated_at_seconds,
            provider_state,
            name,
            created_at_seconds,
            runtime_status,
        ) = match summary {
            ProviderSessionSummary::HostedProvider {
                target,
                working_directory,
                updated_at,
                state,
                ..
            } => (
                target,
                working_directory,
                *updated_at,
                Some(*state),
                None,
                None,
                PickerRuntimeStatus::from_provider(state),
            ),
            ProviderSessionSummary::ClaudeCodeInteractive {
                target,
                working_directory,
                updated_at,
                started_at,
                name,
                status,
                ..
            } => (
                target,
                working_directory,
                *updated_at,
                None,
                name.clone(),
                Some(*started_at),
                match status {
                    ClaudeCodeInteractiveStatus::Busy => PickerRuntimeStatus::Active,
                    ClaudeCodeInteractiveStatus::Idle | ClaudeCodeInteractiveStatus::Shell => {
                        PickerRuntimeStatus::Idle
                    }
                    ClaudeCodeInteractiveStatus::Waiting => PickerRuntimeStatus::Blocked,
                    ClaudeCodeInteractiveStatus::Unreported
                    | ClaudeCodeInteractiveStatus::Other => PickerRuntimeStatus::Unknown,
                },
            ),
        };
        let session_id = String::from(target.session_id.clone());
        let cwd = String::from(working_directory.clone());
        let updated_at_ms = updated_at_seconds.saturating_mul(1_000);
        let title = name
            .clone()
            .unwrap_or_else(|| format!("{endpoint_label} · {session_id}"));
        let context = session_context_from_cwd(&cwd);
        Self {
            identity: SessionPickerIdentity::HostedProvider(target.clone()),
            source_context: None,
            machine_display_label: None,
            provenance: SessionRowProvenance::ObservedProvider,
            endpoint_label: Some(endpoint_label.to_owned()),
            provider_state,
            session_id,
            title: title.clone(),
            full_title: title,
            explicit_name: name,
            recency: format_recency_at_ms(Some(updated_at_ms)),
            created: created_at_seconds.map_or_else(
                || "-".to_owned(),
                |value| format_recency_at_ms(Some(value.saturating_mul(1_000))),
            ),
            recency_at_ms: Some(updated_at_ms),
            created_at_ms: created_at_seconds.map(|value| value.saturating_mul(1_000)),
            branch: "-".to_owned(),
            persisted_branch: String::new(),
            context,
            cwd: Some(cwd.clone()),
            normalized_cwd: Some(normalize_path(Path::new(&cwd)).display().to_string()),
            git_origin_url: None,
            provider: None,
            model: None,
            reasoning_effort: None,
            preview: None,
            first_user_message: String::new(),
            conversation: SessionConversationPreview::unavailable(
                "Provider history opens through its session face",
            ),
            conversation_source: None,
            source: None,
            thread_source: None,
            native_source: None,
            runtime_status,
        }
    }
}

#[path = "native_summary_projection.rs"]
mod native_summary_projection;

impl SessionConversationPreview {
    pub(crate) fn from_rollout_source(source: Option<&SessionConversationSource>) -> Self {
        let history = read_session_conversation_history(source);
        Self {
            snippets: history
                .snippets
                .into_iter()
                .map(|snippet| truncate_end(&snippet, SESSION_CONVERSATION_SNIPPET_MAX_CHARS))
                .collect(),
            unavailable_reason: history.unavailable_reason,
        }
    }

    pub(crate) fn unavailable(reason: &str) -> Self {
        Self {
            snippets: Vec::new(),
            unavailable_reason: Some(reason.to_owned()),
        }
    }
}

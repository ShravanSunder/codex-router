//! CLI and picker projection of reusable stored-session records.

use super::{
    SESSION_CONVERSATION_SNIPPET_MAX_CHARS, display_title_from_session_fields,
    format_recency_at_ms, normalize_path, session_context_from_cwd, truncate_end,
};
use collaboration_client::session_catalog::{
    SessionHistorySource, StoredSessionRecord, read_session_conversation_history,
};
use std::path::Path;

use crate::picker_runtime_status::PickerRuntimeStatus;

pub(super) type SessionRecord = StoredSessionRecord;
pub(crate) type SessionConversationSource = SessionHistorySource;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SessionPickerRecord {
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
    pub(crate) runtime_status: PickerRuntimeStatus,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SessionConversationPreview {
    pub(crate) snippets: Vec<String>,
    pub(crate) unavailable_reason: Option<String>,
}

impl SessionPickerRecord {
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
        let display_title = display_title_from_session_fields(
            record.name.as_deref(),
            record.title.as_deref(),
            record.preview.as_deref(),
            record.first_user_message.as_deref(),
        )
        .unwrap_or_else(|| "Untitled session".to_owned());
        Self {
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
            normalized_cwd: record.cwd.as_deref().map(|cwd| {
                normalize_path(Path::new(cwd))
                    .to_string_lossy()
                    .into_owned()
            }),
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
            runtime_status: PickerRuntimeStatus::Unknown,
        }
    }
}

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

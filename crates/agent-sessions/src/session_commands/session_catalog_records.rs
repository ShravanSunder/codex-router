//! Stored-session records and human picker projection with deferred history reads.
use super::{
    extract_recent_conversation_snippets, format_recency_at_ms, non_empty_trimmed,
    normalize_git_origin_url, normalize_path, read_history_tail, session_context_from_cwd,
    validated_rollout_path,
};
use codex_native_integration::{SessionSearchDocument, SessionSearchExpression};
use serde::Serialize;
use std::path::{Path, PathBuf};

use crate::picker_runtime_status::PickerRuntimeStatus;

#[derive(Debug, Serialize)]
pub(super) struct SessionRecord {
    pub(super) session_id: String,
    #[serde(skip)]
    pub(super) rollout_path: Option<SessionConversationSource>,
    #[serde(skip)]
    pub(super) display_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) thread_source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) git_branch: Option<String>,
    #[serde(skip)]
    pub(super) git_origin_url: Option<String>,
    #[serde(skip)]
    pub(super) name: Option<String>,
    #[serde(skip)]
    pub(super) title: Option<String>,
    #[serde(skip)]
    pub(super) preview: Option<String>,
    #[serde(skip)]
    pub(super) first_user_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) created_at_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) updated_at_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) recency_at_ms: Option<i64>,
}

impl SessionRecord {
    pub(super) fn display_title(&self) -> &str {
        self.display_title.as_deref().unwrap_or("Untitled session")
    }

    pub(super) fn branch(&self) -> &str {
        self.git_branch.as_deref().unwrap_or("-")
    }

    pub(super) fn matches_search(&self, expression: &SessionSearchExpression) -> bool {
        let normalized_origin = self
            .git_origin_url
            .as_deref()
            .and_then(normalize_git_origin_url)
            .unwrap_or_default();
        expression.matches(&SessionSearchDocument {
            session_id: &self.session_id,
            name: self.name.as_deref().unwrap_or_default(),
            title: self.title.as_deref().unwrap_or_default(),
            preview: self.preview.as_deref().unwrap_or_default(),
            first_user_message: self.first_user_message.as_deref().unwrap_or_default(),
            branch: self.git_branch.as_deref().unwrap_or_default(),
            origin: &normalized_origin,
            cwd: self.cwd.as_deref().unwrap_or_default(),
        })
    }
}

/// Picker display row for one session.
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
    pub(crate) preview: Option<String>,
    pub(crate) first_user_message: String,
    pub(crate) conversation: SessionConversationPreview,
    pub(crate) conversation_source: Option<SessionConversationSource>,
    pub(crate) source: Option<String>,
    pub(crate) thread_source: Option<String>,
    pub(crate) runtime_status: PickerRuntimeStatus,
}

/// Sanitized conversation snippets for human-only session detail UI.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SessionConversationPreview {
    pub(crate) snippets: Vec<String>,
    pub(crate) unavailable_reason: Option<String>,
}

/// Deferred, validated-on-read conversation history source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SessionConversationSource {
    pub(super) rollout_path: String,
    pub(super) codex_home_path: PathBuf,
}

#[cfg(test)]
impl SessionConversationSource {
    pub(crate) fn for_test(rollout_path: impl Into<String>, codex_home_path: PathBuf) -> Self {
        Self {
            rollout_path: rollout_path.into(),
            codex_home_path,
        }
    }
}

impl SessionPickerRecord {
    pub(crate) fn matches_search(&self, expression: &SessionSearchExpression) -> bool {
        let normalized_origin = self
            .git_origin_url
            .as_deref()
            .and_then(normalize_git_origin_url)
            .unwrap_or_default();
        expression.matches(&SessionSearchDocument {
            session_id: &self.session_id,
            name: self.explicit_name.as_deref().unwrap_or_default(),
            title: &self.full_title,
            preview: self.preview.as_deref().unwrap_or_default(),
            first_user_message: &self.first_user_message,
            branch: &self.persisted_branch,
            origin: &normalized_origin,
            cwd: self.cwd.as_deref().unwrap_or_default(),
        })
    }

    pub(super) fn from_record(record: &SessionRecord) -> Self {
        Self {
            session_id: record.session_id.clone(),
            title: record.display_title().to_owned(),
            explicit_name: record.name.clone(),
            full_title: record.title.clone().unwrap_or_default(),
            recency: format_recency_at_ms(record.recency_at_ms),
            created: format_recency_at_ms(record.created_at_ms),
            recency_at_ms: record.recency_at_ms,
            created_at_ms: record.created_at_ms,
            branch: record.branch().to_owned(),
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
        let Some(source) = source else {
            return Self::unavailable("history unavailable");
        };
        let Some(path) =
            validated_rollout_path(&source.codex_home_path, Some(&source.rollout_path))
        else {
            return Self::unavailable("history unavailable");
        };
        Self::from_rollout_path(Some(&path))
    }

    pub(crate) fn from_rollout_path(rollout_path: Option<&str>) -> Self {
        let Some(rollout_path) = rollout_path.and_then(non_empty_trimmed) else {
            return Self::unavailable("history unavailable");
        };
        let path = Path::new(rollout_path);
        if !path.is_file() {
            return Self::unavailable("history unavailable");
        }

        let Ok(text) = read_history_tail(path) else {
            return Self::unavailable("history unavailable");
        };
        let snippets = extract_recent_conversation_snippets(&text);
        if snippets.is_empty() {
            return Self::unavailable("no recent messages");
        }
        Self {
            snippets,
            unavailable_reason: None,
        }
    }

    pub(crate) fn unavailable(reason: &str) -> Self {
        Self {
            snippets: Vec::new(),
            unavailable_reason: Some(reason.to_owned()),
        }
    }
}

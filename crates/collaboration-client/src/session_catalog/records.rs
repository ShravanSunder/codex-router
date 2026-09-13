use codex_native_integration::{SessionSearchDocument, SessionSearchExpression};
use serde::Serialize;

use super::SessionHistorySource;
use super::repository::normalize_git_origin_url;

/// One raw stored Codex session returned by the reusable catalog reader.
#[derive(Clone, Debug, Serialize)]
pub struct StoredSessionRecord {
    pub session_id: String,
    #[serde(skip)]
    pub rollout_path: Option<SessionHistorySource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_branch: Option<String>,
    #[serde(skip)]
    pub git_origin_url: Option<String>,
    #[serde(skip)]
    pub name: Option<String>,
    #[serde(skip)]
    pub title: Option<String>,
    #[serde(skip)]
    pub preview: Option<String>,
    #[serde(skip)]
    pub first_user_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recency_at_ms: Option<i64>,
}

impl StoredSessionRecord {
    /// Tests this record against the catalog's complete persisted search fields.
    #[must_use]
    pub fn matches_search(&self, expression: &SessionSearchExpression) -> bool {
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

//! Read-only stored Codex session discovery and bounded rollout-history access.

mod history;
mod query;
mod records;
mod repository;

pub use codex_native_integration::{SessionSearchDocument, SessionSearchExpression};
pub use history::{
    SessionConversationHistory, SessionHistorySource, read_session_conversation_history,
};
pub use query::{
    SessionCatalogError, SessionCatalogProvider, SessionCatalogQuery, SessionCatalogRoot,
    SessionCatalogSort, SessionCatalogSource, current_session_provider, load_stored_sessions,
};
pub use records::StoredSessionRecord;
pub use repository::{
    RepositoryIdentity, discover_repository_identity, normalize_path,
    normalized_paths_resolve_to_same_location, path_identity_candidates,
    paths_resolve_to_same_location, repository_contains_session,
};

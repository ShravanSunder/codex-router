//! Reusable stored-session filtering, keyset paging and provider selection.

use sqlx::Row;
use std::{
    fs,
    path::{Path, PathBuf},
};
use thiserror::Error;

use super::repository::{
    discover_repository_identity, find_worktree_root, normalize_path, path_identity_candidates,
    paths_resolve_to_same_location, repository_contains_session,
};
use super::{
    RepositoryIdentity, SessionHistorySource, SessionSearchExpression, StoredSessionRecord,
};

const SESSION_RECORD_PAGE_SIZE: usize = 250;

/// Directory scope for stored-session discovery.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionCatalogRoot {
    Cwd,
    Checkout,
    Repository,
    Any,
}

/// Provider selection for stored-session discovery.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SessionCatalogProvider {
    Any,
    Current,
    Id(String),
}

/// Native session source class.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionCatalogSource {
    Interactive,
    All,
    Subagents,
}

/// Stored-session ordering.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionCatalogSort {
    Updated,
    Created,
}

/// Explicit inputs for one read-only stored-session query.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionCatalogQuery {
    pub codex_home: PathBuf,
    pub current_dir: PathBuf,
    pub root: SessionCatalogRoot,
    pub provider: SessionCatalogProvider,
    pub source: SessionCatalogSource,
    pub sort: SessionCatalogSort,
    pub last: bool,
    pub limit: usize,
    pub search: String,
    pub repository_identity: Option<RepositoryIdentity>,
}

/// Failures produced by neutral stored-session discovery.
#[derive(Debug, Error)]
pub enum SessionCatalogError {
    #[error(
        "could not find model_provider in CODEX_HOME/codex-router.config.toml or CODEX_HOME/config.toml"
    )]
    CurrentProviderUnavailable,
    #[error("failed to read Codex config {path}: {source}")]
    ConfigRead {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to read Codex sessions state: {0}")]
    Sqlx(#[from] sqlx::Error),
}

/// Loads raw stored records through the provider-owned read-only SQLite adapter.
pub async fn load_stored_sessions(
    query: SessionCatalogQuery,
) -> Result<Vec<StoredSessionRecord>, SessionCatalogError> {
    let root_filter = RootFilter::from_query(&query);
    let provider_filter = ProviderFilter::from_query(&query.provider, &query.codex_home)?;
    let catalog = codex_native_integration::StoredThreadCatalog::open(&query.codex_home).await?;

    let mut records = Vec::new();
    let search_expression = SessionSearchExpression::parse(&query.search);
    let target_limit = if query.last { 1 } else { query.limit };
    let mut page_cursor = None;
    while target_limit == 0 || records.len() < target_limit {
        let page_query = native_session_record_query(
            &root_filter,
            &provider_filter,
            query.source,
            query.sort,
            SESSION_RECORD_PAGE_SIZE,
            page_cursor.as_ref(),
        );
        let rows = catalog.read_page(&page_query).await?;
        if rows.is_empty() {
            break;
        }
        let page_was_full = rows.len() == SESSION_RECORD_PAGE_SIZE;
        page_cursor = rows.last().map(|row| SessionRecordPageCursor {
            sort_value: match query.sort {
                SessionCatalogSort::Created => row.get::<Option<i64>, _>("created_at_ms"),
                SessionCatalogSort::Updated => row.get::<Option<i64>, _>("recency_at_ms"),
            },
            session_id: row.get("id"),
        });

        for row in rows {
            let record = StoredSessionRecord {
                session_id: row.get("id"),
                rollout_path: SessionHistorySource::from_catalog_path(
                    &query.codex_home,
                    row.get::<Option<String>, _>("rollout_path").as_deref(),
                ),
                cwd: row.get("cwd"),
                provider: row.get("model_provider"),
                model: row.get("model"),
                source: row.get("source"),
                thread_source: row.get("thread_source"),
                git_branch: row.get("git_branch"),
                git_origin_url: row.get("git_origin_url"),
                name: row.get("name"),
                title: row.get("title"),
                preview: row.get("preview"),
                first_user_message: row.get("first_user_message"),
                created_at_ms: row.get("created_at_ms"),
                updated_at_ms: row.get("updated_at_ms"),
                recency_at_ms: row.get("recency_at_ms"),
            };
            if !session_record_matches_root(&record, &root_filter)
                || !record.matches_search(&search_expression)
            {
                continue;
            }
            records.push(record);
            if target_limit != 0 && records.len() >= target_limit {
                break;
            }
        }
        if !page_was_full {
            break;
        }
    }
    catalog.close().await;
    Ok(records)
}

/// Resolves the configured provider from the router profile and then the default profile.
pub fn current_session_provider(codex_home: &Path) -> Result<String, SessionCatalogError> {
    for config_path in [
        codex_home.join("codex-router.config.toml"),
        codex_home.join("config.toml"),
    ] {
        match fs::read_to_string(&config_path) {
            Ok(content) => {
                if let Some(provider) = parse_model_provider(&content) {
                    return Ok(provider);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(source) => {
                return Err(SessionCatalogError::ConfigRead {
                    path: config_path,
                    source,
                });
            }
        }
    }
    Err(SessionCatalogError::CurrentProviderUnavailable)
}

fn parse_model_provider(content: &str) -> Option<String> {
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() != "model_provider" {
            continue;
        }
        let value = value.trim();
        if value.len() < 2 || !value.starts_with('"') || !value.ends_with('"') {
            continue;
        }
        return value
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
            .map(str::to_owned);
    }
    None
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SessionRecordPageCursor {
    sort_value: Option<i64>,
    session_id: String,
}

#[derive(Debug)]
enum ProviderFilter {
    Any,
    Id(String),
}

impl ProviderFilter {
    fn from_query(
        provider: &SessionCatalogProvider,
        codex_home: &Path,
    ) -> Result<Self, SessionCatalogError> {
        match provider {
            SessionCatalogProvider::Any => Ok(Self::Any),
            SessionCatalogProvider::Id(provider_id) => Ok(Self::Id(provider_id.clone())),
            SessionCatalogProvider::Current => Ok(Self::Id(current_session_provider(codex_home)?)),
        }
    }
}

#[derive(Debug)]
enum RootFilter {
    Any,
    Cwd(Vec<PathBuf>),
    Checkout(PathBuf),
    Repository(RepositoryIdentity),
}

impl RootFilter {
    fn from_query(query: &SessionCatalogQuery) -> Self {
        match query.root {
            SessionCatalogRoot::Any => Self::Any,
            SessionCatalogRoot::Cwd => Self::Cwd(path_identity_candidates(&query.current_dir)),
            SessionCatalogRoot::Checkout => {
                let identity = discover_repository_identity(&query.current_dir);
                if identity.fallback_cwd.is_some() {
                    Self::Cwd(path_identity_candidates(&query.current_dir))
                } else {
                    find_worktree_root(&query.current_dir).map_or_else(
                        || Self::Cwd(path_identity_candidates(&query.current_dir)),
                        Self::Checkout,
                    )
                }
            }
            SessionCatalogRoot::Repository => Self::Repository(
                query
                    .repository_identity
                    .clone()
                    .unwrap_or_else(|| discover_repository_identity(&query.current_dir)),
            ),
        }
    }
}

fn session_record_matches_root(record: &StoredSessionRecord, root_filter: &RootFilter) -> bool {
    match root_filter {
        RootFilter::Cwd(current_dirs) => record.cwd.as_deref().is_some_and(|cwd| {
            current_dirs
                .iter()
                .any(|current_dir| paths_resolve_to_same_location(Path::new(cwd), current_dir))
        }),
        RootFilter::Repository(identity) => record.cwd.as_deref().is_some_and(|cwd| {
            repository_contains_session(
                identity,
                record.git_origin_url.as_deref(),
                &normalize_path(Path::new(cwd)),
            )
        }),
        RootFilter::Any | RootFilter::Checkout(_) => true,
    }
}

fn native_session_record_query(
    root_filter: &RootFilter,
    provider_filter: &ProviderFilter,
    source: SessionCatalogSource,
    sort: SessionCatalogSort,
    page_size: usize,
    page_cursor: Option<&SessionRecordPageCursor>,
) -> codex_native_integration::StoredThreadQuery {
    use codex_native_integration::{
        StoredThreadCursor, StoredThreadProvider, StoredThreadQuery, StoredThreadRoot,
        StoredThreadSort, StoredThreadSource,
    };
    StoredThreadQuery {
        root: match root_filter {
            RootFilter::Checkout(path) => StoredThreadRoot::Checkout(path.clone()),
            _ => StoredThreadRoot::Any,
        },
        provider: match provider_filter {
            ProviderFilter::Any => StoredThreadProvider::Any,
            ProviderFilter::Id(id) => StoredThreadProvider::Id(id.clone()),
        },
        source: match source {
            SessionCatalogSource::All => StoredThreadSource::All,
            SessionCatalogSource::Interactive => StoredThreadSource::Interactive,
            SessionCatalogSource::Subagents => StoredThreadSource::Subagents,
        },
        sort: match sort {
            SessionCatalogSort::Created => StoredThreadSort::Created,
            SessionCatalogSort::Updated => StoredThreadSort::Updated,
        },
        page_size,
        cursor: page_cursor.map(|cursor| StoredThreadCursor {
            sort_value: cursor.sort_value,
            session_id: cursor.session_id.clone(),
        }),
        query: None,
    }
}

#[cfg(test)]
pub(super) fn session_record_page_query(
    root: SessionCatalogRoot,
    provider: SessionCatalogProvider,
    source: SessionCatalogSource,
    sort: SessionCatalogSort,
    cursor: Option<(&str, Option<i64>)>,
) -> sqlx::QueryBuilder<sqlx::Sqlite> {
    let current_dir = PathBuf::from("/test/current");
    let query = SessionCatalogQuery {
        codex_home: PathBuf::from("/test/codex"),
        current_dir,
        root,
        provider,
        source,
        sort,
        last: false,
        limit: 100,
        search: String::new(),
        repository_identity: None,
    };
    let root_filter = RootFilter::from_query(&query);
    let provider_filter = match query.provider {
        SessionCatalogProvider::Any | SessionCatalogProvider::Current => ProviderFilter::Any,
        SessionCatalogProvider::Id(id) => ProviderFilter::Id(id),
    };
    let cursor = cursor.map(|(session_id, sort_value)| SessionRecordPageCursor {
        sort_value,
        session_id: session_id.to_owned(),
    });
    codex_native_integration::stored_thread_page_query(&native_session_record_query(
        &root_filter,
        &provider_filter,
        query.source,
        query.sort,
        SESSION_RECORD_PAGE_SIZE,
        cursor.as_ref(),
    ))
}

#[cfg(test)]
#[path = "tests/query_tests.rs"]
mod tests;

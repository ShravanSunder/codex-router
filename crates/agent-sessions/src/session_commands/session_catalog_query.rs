//! Read-only catalog filtering, keyset paging and native-query projection.
use super::{
    CliContext, DEFAULT_SESSION_RECORD_LIMIT, RepositoryIdentity, SESSION_RECORD_PAGE_SIZE,
    SessionRecord, SessionSearchExpression, SessionsCommand, SessionsCommandError,
    SessionsPickerDataQuery, SessionsProvider, SessionsRoot, SessionsSort, SessionsSource,
    deferred_rollout_source, display_title_from_session_fields, find_worktree_root, normalize_path,
    path_identity_candidates, paths_resolve_to_same_location, session_belongs_to_repository,
};
use sqlx::Row;
#[cfg(test)]
use sqlx::{QueryBuilder, Sqlite};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct SessionRecordPageCursor {
    pub(super) sort_value: Option<i64>,
    pub(super) session_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct SessionRecordQuery {
    pub(super) root: SessionsRoot,
    pub(super) provider: SessionsProvider,
    pub(super) source: SessionsSource,
    pub(super) sort: SessionsSort,
    pub(super) last: bool,
    pub(super) limit: usize,
    pub(super) search: String,
}

impl SessionRecordQuery {
    pub(super) fn from_command(command: &SessionsCommand) -> Self {
        Self {
            root: command.root,
            provider: command.provider.clone(),
            source: command.source,
            sort: command.sort,
            last: command.last,
            limit: command.limit,
            search: String::new(),
        }
    }

    pub(super) fn from_picker_query(query: SessionsPickerDataQuery) -> Self {
        Self {
            root: query.root.into(),
            provider: query.provider,
            source: query.source,
            sort: query.sort,
            last: false,
            limit: DEFAULT_SESSION_RECORD_LIMIT,
            search: query.search,
        }
    }
}

pub(super) async fn load_session_records(
    command: SessionsCommand,
    context: &CliContext,
) -> Result<Vec<SessionRecord>, SessionsCommandError> {
    load_session_records_for_query(SessionRecordQuery::from_command(&command), context).await
}

async fn load_session_records_for_query(
    query: SessionRecordQuery,
    context: &CliContext,
) -> Result<Vec<SessionRecord>, SessionsCommandError> {
    load_session_records_for_query_with_identity(query, context, None).await
}

pub(super) async fn load_session_records_for_query_with_identity(
    query: SessionRecordQuery,
    context: &CliContext,
    repository_identity: Option<RepositoryIdentity>,
) -> Result<Vec<SessionRecord>, SessionsCommandError> {
    let root_filter = RootFilter::from_query(query.root, context, repository_identity);
    let codex_home_path = codex_home(context)?;
    let provider_filter = ProviderFilter::from_command(&query.provider, &codex_home_path)?;

    let catalog = codex_native_integration::StoredThreadCatalog::open(&codex_home_path)
        .await
        .map_err(SessionsCommandError::Sqlx)?;

    let mut records = Vec::new();
    let search_expression = SessionSearchExpression::parse(&query.search);
    let target_limit = if query.last { 1 } else { query.limit };
    let mut page_cursor = None;
    while target_limit == 0 || records.len() < target_limit {
        let page_size = session_record_candidate_page_size();
        let page_query = native_session_record_query(
            &root_filter,
            &provider_filter,
            query.source,
            query.sort,
            page_size,
            page_cursor.as_ref(),
        );
        let rows = catalog
            .read_page(&page_query)
            .await
            .map_err(SessionsCommandError::Sqlx)?;

        if rows.is_empty() {
            break;
        }
        let page_was_full = rows.len() == page_size;
        page_cursor = rows.last().map(|row| SessionRecordPageCursor {
            sort_value: match query.sort {
                SessionsSort::Created => row.get::<Option<i64>, _>("created_at_ms"),
                SessionsSort::Updated => row.get::<Option<i64>, _>("recency_at_ms"),
            },
            session_id: row.get("id"),
        });

        for row in rows {
            let source = row.get::<Option<String>, _>("source");
            let thread_source = row.get::<Option<String>, _>("thread_source");
            let cwd = row.get::<Option<String>, _>("cwd");
            let name = row.get::<Option<String>, _>("name");
            let title = row.get::<Option<String>, _>("title");
            let preview = row.get::<Option<String>, _>("preview");
            let first_user_message = row.get::<Option<String>, _>("first_user_message");
            let record = SessionRecord {
                session_id: row.get("id"),
                rollout_path: deferred_rollout_source(
                    &codex_home_path,
                    row.get::<Option<String>, _>("rollout_path").as_deref(),
                ),
                cwd,
                provider: row.get::<Option<String>, _>("model_provider"),
                model: row.get::<Option<String>, _>("model"),
                source,
                thread_source,
                git_branch: row.get::<Option<String>, _>("git_branch"),
                git_origin_url: row.get::<Option<String>, _>("git_origin_url"),
                name: name.clone(),
                title: title.clone(),
                preview: preview.clone(),
                first_user_message: first_user_message.clone(),
                display_title: display_title_from_session_fields(
                    name.as_deref(),
                    title.as_deref(),
                    preview.as_deref(),
                    first_user_message.as_deref(),
                ),
                created_at_ms: row.get::<Option<i64>, _>("created_at_ms"),
                updated_at_ms: row.get::<Option<i64>, _>("updated_at_ms"),
                recency_at_ms: row.get::<Option<i64>, _>("recency_at_ms"),
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

pub(super) fn session_record_candidate_page_size() -> usize {
    SESSION_RECORD_PAGE_SIZE
}

#[cfg(test)]
pub(super) fn session_record_page_query(
    root: &RootFilter,
    provider: &ProviderFilter,
    source: SessionsSource,
    sort: SessionsSort,
    page_size: usize,
    cursor: Option<&SessionRecordPageCursor>,
) -> QueryBuilder<Sqlite> {
    codex_native_integration::stored_thread_page_query(&native_session_record_query(
        root, provider, source, sort, page_size, cursor,
    ))
}

fn native_session_record_query(
    root_filter: &RootFilter,
    provider_filter: &ProviderFilter,
    source: SessionsSource,
    sort: SessionsSort,
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
            SessionsSource::All => StoredThreadSource::All,
            SessionsSource::Interactive => StoredThreadSource::Interactive,
            SessionsSource::Subagents => StoredThreadSource::Subagents,
        },
        sort: match sort {
            SessionsSort::Created => StoredThreadSort::Created,
            SessionsSort::Updated => StoredThreadSort::Updated,
        },
        page_size,
        cursor: page_cursor.map(|cursor| StoredThreadCursor {
            sort_value: cursor.sort_value,
            session_id: cursor.session_id.clone(),
        }),
    }
}

#[derive(Debug)]
pub(super) enum ProviderFilter {
    Any,
    Id(String),
}

impl ProviderFilter {
    pub(super) fn from_command(
        provider: &SessionsProvider,
        codex_home: &Path,
    ) -> Result<Self, SessionsCommandError> {
        match provider {
            SessionsProvider::Any => Ok(Self::Any),
            SessionsProvider::Id(provider_id) => Ok(Self::Id(provider_id.clone())),
            SessionsProvider::Current => Ok(Self::Id(resolve_current_provider(codex_home)?)),
        }
    }
}

#[derive(Debug)]
pub(super) enum RootFilter {
    Any,
    Cwd(Vec<PathBuf>),
    Checkout(PathBuf),
    Repo(RepositoryIdentity),
}

impl RootFilter {
    pub(super) fn from_query(
        root: SessionsRoot,
        context: &CliContext,
        repository_identity: Option<RepositoryIdentity>,
    ) -> Self {
        match root {
            SessionsRoot::Any => Self::Any,
            SessionsRoot::Cwd => Self::Cwd(path_identity_candidates(context.current_dir())),
            SessionsRoot::Checkout => {
                let identity = RepositoryIdentity::discover(context.current_dir());
                if identity.fallback_cwd.is_some() {
                    Self::Cwd(path_identity_candidates(context.current_dir()))
                } else {
                    find_worktree_root(context.current_dir()).map_or_else(
                        || Self::Cwd(path_identity_candidates(context.current_dir())),
                        Self::Checkout,
                    )
                }
            }
            SessionsRoot::Repo => Self::Repo(
                repository_identity
                    .unwrap_or_else(|| RepositoryIdentity::discover(context.current_dir())),
            ),
        }
    }
}

pub(super) fn codex_home(context: &CliContext) -> Result<PathBuf, SessionsCommandError> {
    codex_home_from_environment(
        context.env_var("CODEX_HOME").map(PathBuf::from),
        context.env_var("HOME").map(PathBuf::from),
    )
}

pub(super) fn codex_home_from_environment(
    codex_home: Option<PathBuf>,
    home: Option<PathBuf>,
) -> Result<PathBuf, SessionsCommandError> {
    if let Some(codex_home) = codex_home {
        return Ok(codex_home);
    }
    let Some(home) = home else {
        return Err(SessionsCommandError::CodexHomeUnavailable);
    };
    Ok(home.join(".codex"))
}

fn resolve_current_provider(codex_home: &Path) -> Result<String, SessionsCommandError> {
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
                return Err(SessionsCommandError::ConfigRead {
                    path: config_path,
                    source,
                });
            }
        }
    }
    Err(SessionsCommandError::CurrentProviderUnavailable)
}

pub(super) fn current_provider_for_picker(context: &CliContext) -> Option<String> {
    codex_home(context)
        .ok()
        .and_then(|codex_home| resolve_current_provider(&codex_home).ok())
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
        let Some(value) = value
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
        else {
            continue;
        };
        return Some(value.to_owned());
    }
    None
}

pub(super) fn session_record_matches_root(
    record: &SessionRecord,
    root_filter: &RootFilter,
) -> bool {
    match root_filter {
        RootFilter::Cwd(current_dirs) => record.cwd.as_deref().is_some_and(|cwd| {
            current_dirs
                .iter()
                .any(|current_dir| paths_resolve_to_same_location(Path::new(cwd), current_dir))
        }),
        RootFilter::Repo(identity) => record.cwd.as_deref().is_some_and(|cwd| {
            let normalized_cwd = normalize_path(Path::new(cwd));
            session_belongs_to_repository(
                identity,
                record.git_origin_url.as_deref(),
                &normalized_cwd,
            )
        }),
        RootFilter::Any | RootFilter::Checkout(_) => true,
    }
}

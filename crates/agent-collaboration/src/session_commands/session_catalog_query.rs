//! CLI conversion into the reusable stored-session catalog.

use super::{
    CliContext, DEFAULT_SESSION_RECORD_LIMIT, RepositoryIdentity, SessionRecord, SessionsCommand,
    SessionsCommandError, SessionsPickerDataQuery, SessionsProvider, SessionsRoot, SessionsSort,
    SessionsSource,
};
use collaboration_client::session_catalog::{
    SessionCatalogProvider, SessionCatalogQuery, SessionCatalogRoot, SessionCatalogSort,
    SessionCatalogSource, current_session_provider, load_stored_sessions,
};
use std::path::PathBuf;

/// A session-id search is a substring match, so read a few rows and keep the exact one.
const SESSION_ID_LOOKUP_LIMIT: usize = 8;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct SessionRecordQuery {
    root: SessionCatalogRoot,
    provider: SessionCatalogProvider,
    source: SessionCatalogSource,
    sort: SessionCatalogSort,
    last: bool,
    limit: usize,
    search: String,
}

impl SessionRecordQuery {
    pub(super) fn from_command(command: &SessionsCommand) -> Self {
        Self {
            root: catalog_root(command.root),
            provider: catalog_provider(&command.provider),
            source: catalog_source(command.source),
            sort: catalog_sort(command.sort),
            last: command.last,
            limit: command.limit,
            search: String::new(),
        }
    }

    /// Query that isolates one stored record by its session id.
    pub(super) fn for_session_id(session_id: &str) -> Self {
        Self {
            root: SessionCatalogRoot::Any,
            provider: SessionCatalogProvider::Any,
            source: SessionCatalogSource::All,
            sort: SessionCatalogSort::Updated,
            last: false,
            limit: SESSION_ID_LOOKUP_LIMIT,
            search: format!("id:{session_id}"),
        }
    }

    pub(super) fn from_picker_query(query: SessionsPickerDataQuery) -> Self {
        Self {
            root: catalog_root(query.root.into()),
            provider: catalog_provider(&query.provider),
            source: catalog_source(query.source),
            sort: catalog_sort(query.sort),
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
    load_session_records_for_query_with_identity(
        SessionRecordQuery::from_command(&command),
        context,
        None,
    )
    .await
}

pub(super) async fn load_session_records_for_query_with_identity(
    query: SessionRecordQuery,
    context: &CliContext,
    repository_identity: Option<RepositoryIdentity>,
) -> Result<Vec<SessionRecord>, SessionsCommandError> {
    Ok(load_stored_sessions(SessionCatalogQuery {
        codex_home: codex_home(context)?,
        current_dir: context.current_dir().to_path_buf(),
        root: query.root,
        provider: query.provider,
        source: query.source,
        sort: query.sort,
        last: query.last,
        limit: query.limit,
        search: query.search,
        repository_identity,
    })
    .await?)
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
    home.map(|home| home.join(".codex"))
        .ok_or(SessionsCommandError::CodexHomeUnavailable)
}

pub(super) fn current_provider_for_picker(context: &CliContext) -> Option<String> {
    codex_home(context)
        .ok()
        .and_then(|codex_home| current_session_provider(&codex_home).ok())
}

fn catalog_root(root: SessionsRoot) -> SessionCatalogRoot {
    match root {
        SessionsRoot::Cwd => SessionCatalogRoot::Cwd,
        SessionsRoot::Checkout => SessionCatalogRoot::Checkout,
        SessionsRoot::Repo => SessionCatalogRoot::Repository,
        SessionsRoot::Any => SessionCatalogRoot::Any,
    }
}

fn catalog_provider(provider: &SessionsProvider) -> SessionCatalogProvider {
    match provider {
        SessionsProvider::Any => SessionCatalogProvider::Any,
        SessionsProvider::Current => SessionCatalogProvider::Current,
        SessionsProvider::Id(provider_id) => SessionCatalogProvider::Id(provider_id.clone()),
    }
}

fn catalog_source(source: SessionsSource) -> SessionCatalogSource {
    match source {
        SessionsSource::Interactive => SessionCatalogSource::Interactive,
        SessionsSource::All => SessionCatalogSource::All,
        SessionsSource::Subagents => SessionCatalogSource::Subagents,
    }
}

fn catalog_sort(sort: SessionsSort) -> SessionCatalogSort {
    match sort {
        SessionsSort::Updated => SessionCatalogSort::Updated,
        SessionsSort::Created => SessionCatalogSort::Created,
    }
}

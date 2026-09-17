//! Shared read-only Codex thread query with stable keyset ordering.
use sqlx::{QueryBuilder, Sqlite};
use std::path::{Path, PathBuf};
#[derive(Clone, Debug)]
pub enum StoredThreadRoot {
    Any,
    Cwd(PathBuf),
    Checkout(PathBuf),
    Repo {
        live_roots: Vec<PathBuf>,
        normalized_origin: Option<String>,
        basename: String,
        fallback_cwd: Option<PathBuf>,
    },
}
#[derive(Clone, Debug)]
pub enum StoredThreadProvider {
    Any,
    Id(String),
}
#[derive(Clone, Copy, Debug)]
pub enum StoredThreadSource {
    All,
    Interactive,
    Subagents,
}
#[derive(Clone, Copy, Debug)]
pub enum StoredThreadSort {
    Created,
    Updated,
}
#[derive(Clone, Debug)]
pub struct StoredThreadCursor {
    pub sort_value: Option<i64>,
    pub session_id: String,
}
#[derive(Clone, Debug)]
pub struct StoredThreadQuery {
    pub root: StoredThreadRoot,
    pub provider: StoredThreadProvider,
    pub source: StoredThreadSource,
    pub sort: StoredThreadSort,
    pub page_size: usize,
    pub cursor: Option<StoredThreadCursor>,
    pub query: Option<String>,
}
pub fn stored_thread_page_query(query: &StoredThreadQuery) -> QueryBuilder<Sqlite> {
    let StoredThreadQuery {
        root: root_filter,
        provider: provider_filter,
        source,
        sort,
        page_size,
        cursor,
        query,
    } = query;
    let page_cursor = cursor.as_ref();
    let source = *source;
    let sort = *sort;
    let page_size = *page_size;
    let (sort_column, sort_index) = match sort {
        StoredThreadSort::Created => ("created_at_ms", "idx_threads_created_at_ms"),
        StoredThreadSort::Updated => ("updated_at_ms", "idx_threads_updated_at_ms"),
    };
    let mut builder = QueryBuilder::<Sqlite>::new(
        r#"
            SELECT
                id, rollout_path, cwd, model_provider, model, reasoning_effort, source, thread_source, git_branch,
                git_origin_url, name, title, preview, first_user_message,
                created_at_ms, updated_at_ms, updated_at_ms AS recency_at_ms
            FROM threads INDEXED BY "#,
    );
    builder.push(sort_index).push(" WHERE archived = 0");
    append_session_record_filters(&mut builder, root_filter, provider_filter, source);
    if let Some(query) = query.as_deref() {
        let pattern = format!("%{}%", escape_like(query));
        builder
            .push(" AND (name LIKE ")
            .push_bind(pattern.clone())
            .push(" ESCAPE '\\' COLLATE NOCASE OR title LIKE ")
            .push_bind(pattern)
            .push(" ESCAPE '\\' COLLATE NOCASE)");
    }
    if let Some(cursor) = page_cursor {
        builder.push(" AND (").push(sort_column);
        if let Some(sort_value) = cursor.sort_value {
            builder
                .push(" < ")
                .push_bind(sort_value)
                .push(" OR ")
                .push(sort_column)
                .push(" IS NULL OR (")
                .push(sort_column)
                .push(" = ")
                .push_bind(sort_value)
                .push(" AND id < ")
                .push_bind(cursor.session_id.clone())
                .push(")");
        } else {
            builder
                .push(" IS NULL AND id < ")
                .push_bind(cursor.session_id.clone());
        }
        builder.push(")");
    }
    builder
        .push(" ORDER BY ")
        .push(sort_column)
        .push(" DESC, id DESC LIMIT ")
        .push_bind(i64::try_from(page_size).unwrap_or(i64::MAX));
    builder
}

fn append_session_record_filters(
    builder: &mut QueryBuilder<Sqlite>,
    root_filter: &StoredThreadRoot,
    provider_filter: &StoredThreadProvider,
    source: StoredThreadSource,
) {
    append_root_filter(builder, root_filter);
    append_provider_filter(builder, provider_filter);
    append_source_filter(builder, source);
}

fn append_root_filter(builder: &mut QueryBuilder<Sqlite>, root_filter: &StoredThreadRoot) {
    match root_filter {
        StoredThreadRoot::Any => {}
        StoredThreadRoot::Cwd(cwd) => {
            builder.push(" AND (");
            for (index, value) in path_sql_values(cwd).into_iter().enumerate() {
                if index > 0 {
                    builder.push(" OR ");
                }
                builder.push("cwd = ").push_bind(value);
            }
            builder.push(")");
        }
        StoredThreadRoot::Checkout(checkout_root) => {
            builder.push(" AND (");
            append_path_scope_filter(builder, checkout_root);
            builder.push(")");
        }
        StoredThreadRoot::Repo {
            live_roots,
            normalized_origin,
            basename,
            fallback_cwd,
        } => {
            builder.push(" AND (");
            if let Some(cwd) = fallback_cwd {
                // Without a repository identity the scope is exactly this directory.
                let mut has_clause = false;
                for value in path_sql_values(cwd) {
                    if has_clause {
                        builder.push(" OR ");
                    }
                    builder.push("cwd = ").push_bind(value);
                    has_clause = true;
                }
                if !has_clause {
                    builder.push("0");
                }
                builder.push(")");
                return;
            }
            // Mirrors repository_contains_session: a row that names an origin is
            // decided by that origin alone, so a matching basename can never
            // rescue a row belonging to another repository. Path and basename
            // evidence applies only to rows that name no origin.
            let mut has_clause = false;
            if let Some(origin) = normalized_origin {
                builder.push("git_origin_url IN (");
                for (index, spelling) in origin_spellings(origin).into_iter().enumerate() {
                    if index > 0 {
                        builder.push(", ");
                    }
                    builder.push_bind(spelling);
                }
                builder.push(")");
                has_clause = true;
            } else {
                for root in live_roots {
                    if has_clause {
                        builder.push(" OR ");
                    }
                    append_path_scope_filter(builder, root);
                    has_clause = true;
                }
            }
            let mut originless = Vec::new();
            if normalized_origin.is_some() {
                originless.extend(live_roots.iter().cloned());
            }
            let has_originless_paths = !originless.is_empty();
            if has_originless_paths || !basename.is_empty() {
                if has_clause {
                    builder.push(" OR ");
                }
                builder.push("((git_origin_url IS NULL OR git_origin_url = '') AND (");
                let mut inner = false;
                for root in &originless {
                    if inner {
                        builder.push(" OR ");
                    }
                    append_path_scope_filter(builder, root);
                    inner = true;
                }
                if !basename.is_empty() {
                    if inner {
                        builder.push(" OR ");
                    }
                    append_basename_filter(builder, basename);
                    inner = true;
                }
                if !inner {
                    builder.push("0");
                }
                builder.push("))");
                has_clause = true;
            }
            if !has_clause {
                builder.push("0");
            }
            builder.push(")");
        }
    }
}

/// The stored column holds whatever spelling the runtime captured, and SQLite
/// cannot normalize it, so one normalized origin is compared against the
/// spellings that normalize back to it.
fn origin_spellings(normalized_origin: &str) -> Vec<String> {
    let host_path = normalized_origin.replacen('/', ":", 1);
    let mut spellings = vec![normalized_origin.to_owned()];
    for prefix in ["https://", "http://", "ssh://git@", "git://"] {
        spellings.push(format!("{prefix}{normalized_origin}"));
        spellings.push(format!("{prefix}{normalized_origin}.git"));
    }
    spellings.push(format!("git@{host_path}"));
    spellings.push(format!("git@{host_path}.git"));
    spellings.push(format!("{normalized_origin}.git"));
    spellings
}

/// Matches a checkout directory named for the repository, including the
/// `name.suffix` and `name-suffix` worktree spellings.
fn append_basename_filter(builder: &mut QueryBuilder<Sqlite>, basename: &str) {
    let escaped = escape_like(basename);
    builder
        .push("cwd LIKE ")
        .push_bind(format!("%/{escaped}"))
        .push(" ESCAPE '\\' OR cwd LIKE ")
        .push_bind(format!("%/{escaped}.%"))
        .push(" ESCAPE '\\' OR cwd LIKE ")
        .push_bind(format!("%/{escaped}-%"))
        .push(" ESCAPE '\\'");
}

fn append_path_scope_filter(builder: &mut QueryBuilder<Sqlite>, root: &Path) {
    for (index, path_value) in path_sql_values(root).into_iter().enumerate() {
        if index > 0 {
            builder.push(" OR ");
        }
        builder
            .push("cwd = ")
            .push_bind(path_value.clone())
            .push(" OR cwd LIKE ")
            .push_bind(path_child_like_pattern(&path_value))
            .push(" ESCAPE '\\'");
    }
}

fn append_provider_filter(
    builder: &mut QueryBuilder<Sqlite>,
    provider_filter: &StoredThreadProvider,
) {
    match provider_filter {
        StoredThreadProvider::Any => {}
        StoredThreadProvider::Id(provider_id) => {
            builder
                .push(" AND model_provider = ")
                .push_bind(provider_id.clone());
        }
    }
}

fn append_source_filter(builder: &mut QueryBuilder<Sqlite>, source: StoredThreadSource) {
    match source {
        StoredThreadSource::All => {}
        StoredThreadSource::Interactive => {
            builder.push(
                " AND (thread_source = 'user' OR (source IN ('cli', 'vscode') \
                 AND (thread_source IS NULL OR thread_source NOT IN ('system', 'exec', 'app_server', 'subagent', 'guardian_review', 'memory_consolidation'))))",
            );
        }
        StoredThreadSource::Subagents => {
            builder.push(
                " AND (thread_source IN ('subagent', 'guardian_review', 'memory_consolidation') \
                 OR source = 'subagent' \
                 OR source LIKE ",
            );
            builder.push_bind("%subagent%").push(" ESCAPE '\\')");
        }
    }
}

#[must_use]
pub fn path_sql_values(path: &Path) -> Vec<String> {
    let path = path.to_string_lossy().into_owned();
    let mut values = vec![path.clone()];
    if let Some(stripped_path) = path.strip_prefix("/private/") {
        values.push(format!("/{stripped_path}"));
    } else if path.starts_with("/var/") {
        values.push(format!("/private{path}"));
    }
    values.sort();
    values.dedup();
    values
}

fn path_child_like_pattern(path: &str) -> String {
    let path = path.trim_end_matches('/');
    format!("{}/%", escape_like(path))
}

fn escape_like(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        if matches!(character, '\\' | '%' | '_') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

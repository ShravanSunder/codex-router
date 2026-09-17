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
            let mut has_clause = false;
            if let Some(cwd) = fallback_cwd {
                for value in path_sql_values(cwd) {
                    if has_clause {
                        builder.push(" OR ");
                    }
                    builder.push("cwd = ").push_bind(value);
                    has_clause = true;
                }
            } else {
                // Mirrors `repository_contains_session`: an origin on the row decides the
                // match on its own, and the path and basename clauses only rescue a row
                // that carries no origin at all.
                if normalized_origin.is_some() {
                    append_origin_match(builder, normalized_origin.as_deref());
                    builder.push(" OR (");
                    append_row_has_no_origin(builder);
                    builder.push(" AND (");
                    has_clause = append_path_and_basename_clauses(builder, live_roots, basename);
                    if !has_clause {
                        builder.push("0");
                    }
                    builder.push("))");
                    has_clause = true;
                } else {
                    for root in live_roots {
                        if has_clause {
                            builder.push(" OR ");
                        }
                        append_path_scope_filter(builder, root);
                        has_clause = true;
                    }
                    if !basename.is_empty() {
                        if has_clause {
                            builder.push(" OR ");
                        }
                        builder.push("(");
                        append_row_has_no_origin(builder);
                        builder.push(" AND (");
                        append_basename_clauses(builder, basename);
                        builder.push("))");
                        has_clause = true;
                    }
                }
            }
            if !has_clause {
                builder.push("0");
            }
            builder.push(")");
        }
    }
}

/// A row with no origin is the only row the path and basename clauses may rescue once the
/// caller's repository has an origin of its own.
fn append_row_has_no_origin(builder: &mut QueryBuilder<Sqlite>) {
    builder.push("(git_origin_url IS NULL OR TRIM(git_origin_url) = '')");
}

/// Matches stored origins against a normalized `host/path` identity.
///
/// Codex stores the raw remote URL, and `normalize_git_origin_url` strips the scheme,
/// any user prefix, a `.git` suffix, trailing slashes and query or fragment parts before
/// lowercasing the host. SQLite cannot reproduce that rewrite, so this clause is a
/// deliberate superset: it requires the host and the repository path to appear in the
/// stored URL with the path at its end. A row for a different repository — the work fork
/// whose directory leaf matches the basename — is excluded, which is the behaviour the
/// canonical predicate demands.
fn append_origin_match(builder: &mut QueryBuilder<Sqlite>, normalized_origin: Option<&str>) {
    let Some(origin) = normalized_origin else {
        builder.push("0");
        return;
    };
    builder
        .push("(git_origin_url = ")
        .push_bind(origin.to_owned());
    let (host, repository_path) = match origin.split_once('/') {
        Some((host, repository_path)) if !host.is_empty() && !repository_path.is_empty() => {
            (host, repository_path)
        }
        _ => {
            builder.push(")");
            return;
        }
    };
    let escaped_host = escape_like(host);
    let escaped_path = escape_like(repository_path);
    builder
        .push(" OR (git_origin_url LIKE ")
        .push_bind(format!("%{escaped_host}%"))
        .push(" ESCAPE '\\' AND (");
    for (index, suffix) in ["", ".git", "/", ".git/"].into_iter().enumerate() {
        if index > 0 {
            builder.push(" OR ");
        }
        builder
            .push("git_origin_url LIKE ")
            .push_bind(format!("%{escaped_path}{suffix}"))
            .push(" ESCAPE '\\'");
    }
    builder.push(")))");
}

/// Appends the live-worktree and historical-basename clauses shared by both origin cases.
fn append_path_and_basename_clauses(
    builder: &mut QueryBuilder<Sqlite>,
    live_roots: &[PathBuf],
    basename: &str,
) -> bool {
    let mut has_clause = false;
    for root in live_roots {
        if has_clause {
            builder.push(" OR ");
        }
        append_path_scope_filter(builder, root);
        has_clause = true;
    }
    if !basename.is_empty() {
        if has_clause {
            builder.push(" OR ");
        }
        append_basename_clauses(builder, basename);
        has_clause = true;
    }
    has_clause
}

/// Historical worktree leaves: the repository name itself, or that name followed by `.` or `-`.
fn append_basename_clauses(builder: &mut QueryBuilder<Sqlite>, basename: &str) {
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

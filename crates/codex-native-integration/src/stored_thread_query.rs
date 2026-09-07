//! Shared read-only Codex thread query with stable keyset ordering.
use sqlx::{QueryBuilder, Sqlite};
use std::path::{Path, PathBuf};
#[derive(Clone, Debug)]
pub enum StoredThreadRoot {
    Any,
    Checkout(PathBuf),
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
}
pub fn stored_thread_page_query(query: &StoredThreadQuery) -> QueryBuilder<Sqlite> {
    let StoredThreadQuery {
        root: root_filter,
        provider: provider_filter,
        source,
        sort,
        page_size,
        cursor,
    } = query;
    let page_cursor = cursor.as_ref();
    let source = *source;
    let sort = *sort;
    let page_size = *page_size;
    let (sort_column, sort_index) = match sort {
        StoredThreadSort::Created => ("created_at_ms", "idx_threads_created_at_ms"),
        StoredThreadSort::Updated => ("recency_at_ms", "idx_threads_recency_at_ms"),
    };
    let mut builder = QueryBuilder::<Sqlite>::new(
        r#"
            SELECT
                id, rollout_path, cwd, model_provider, model, source, thread_source, git_branch,
                git_origin_url, name, title, preview, first_user_message,
                created_at_ms, updated_at_ms, recency_at_ms
            FROM threads INDEXED BY "#,
    );
    builder.push(sort_index).push(" WHERE archived = 0");
    append_session_record_filters(&mut builder, root_filter, provider_filter, source);
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
        StoredThreadRoot::Checkout(checkout_root) => {
            builder.push(" AND (");
            append_path_scope_filter(builder, checkout_root);
            builder.push(")");
        }
    }
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
                " AND source IN ('cli', 'vscode') \
                 AND (thread_source IS NULL OR thread_source NOT IN ('exec', 'app_server', 'subagent'))",
            );
        }
        StoredThreadSource::Subagents => {
            builder.push(
                " AND (thread_source = 'subagent' \
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

//! Repository scope in SQL must decide rows the way the human catalog does.
#![allow(clippy::unwrap_used)]
use codex_native_integration::{
    StoredThreadCatalog, StoredThreadProvider, StoredThreadQuery, StoredThreadRoot,
    StoredThreadSort, StoredThreadSource,
};
use sqlx::{Connection, Row, SqliteConnection};
use std::path::PathBuf;

struct StoredRow {
    id: &'static str,
    cwd: &'static str,
    origin: Option<&'static str>,
    reasoning_effort: Option<&'static str>,
}

async fn seeded_codex_home(label: &str, rows: &[StoredRow]) -> PathBuf {
    let home = std::env::temp_dir().join(format!(
        "stored-thread-{label}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&home).unwrap();
    let path = home.join("state_5.sqlite");
    let mut connection =
        SqliteConnection::connect(&format!("sqlite://{}?mode=rwc", path.display()))
            .await
            .unwrap();
    sqlx::query(
        "CREATE TABLE threads(
            id TEXT PRIMARY KEY, rollout_path TEXT, cwd TEXT, model_provider TEXT, model TEXT,
            reasoning_effort TEXT, source TEXT, thread_source TEXT, git_branch TEXT,
            git_origin_url TEXT, name TEXT, title TEXT, preview TEXT, first_user_message TEXT,
            created_at_ms INTEGER, updated_at_ms INTEGER, archived INTEGER NOT NULL DEFAULT 0)",
    )
    .execute(&mut connection)
    .await
    .unwrap();
    for statement in [
        "CREATE INDEX idx_threads_created_at_ms ON threads(created_at_ms)",
        "CREATE INDEX idx_threads_updated_at_ms ON threads(updated_at_ms)",
    ] {
        sqlx::query(statement)
            .execute(&mut connection)
            .await
            .unwrap();
    }
    for (ordinal, row) in rows.iter().enumerate() {
        sqlx::query(
            "INSERT INTO threads(id,cwd,git_origin_url,reasoning_effort,source,thread_source,model,created_at_ms,updated_at_ms,archived)
             VALUES(?,?,?,?, 'interactive','user','gpt-5.6-sol',?,?,0)",
        )
        .bind(row.id)
        .bind(row.cwd)
        .bind(row.origin)
        .bind(row.reasoning_effort)
        .bind(1_000 + ordinal as i64)
        .bind(1_000 + ordinal as i64)
        .execute(&mut connection)
        .await
        .unwrap();
    }
    connection.close().await.unwrap();
    home
}

fn repo_query(root: StoredThreadRoot) -> StoredThreadQuery {
    StoredThreadQuery {
        root,
        provider: StoredThreadProvider::Any,
        source: StoredThreadSource::All,
        sort: StoredThreadSort::Updated,
        page_size: 50,
        cursor: None,
        query: None,
    }
}

#[tokio::test]
async fn repository_scope_decides_rows_by_origin_then_path_like_the_human_catalog() {
    // Arrange: rows covering every cell of repository_contains_session.
    let rows = [
        StoredRow {
            id: "same-origin",
            cwd: "/elsewhere/checkout",
            origin: Some("git@github.com:example/router.git"),
            reasoning_effort: Some("medium"),
        },
        StoredRow {
            id: "other-origin-matching-basename",
            cwd: "/work/router",
            origin: Some("https://github.com/other/router.git"),
            reasoning_effort: Some("high"),
        },
        StoredRow {
            id: "no-origin-live-root",
            cwd: "/work/router",
            origin: None,
            reasoning_effort: None,
        },
        StoredRow {
            id: "no-origin-worktree-basename",
            cwd: "/elsewhere/router.listening",
            origin: None,
            reasoning_effort: Some("low"),
        },
        StoredRow {
            id: "unrelated",
            cwd: "/somewhere/other",
            origin: None,
            reasoning_effort: Some("medium"),
        },
    ];
    let home = seeded_codex_home("repo-scope", &rows).await;
    let catalog = StoredThreadCatalog::open(&home).await.unwrap();

    // Act: the identity of a checkout that knows its origin.
    let with_origin = catalog
        .read_page(&repo_query(StoredThreadRoot::Repo {
            live_roots: vec![PathBuf::from("/work/router")],
            normalized_origin: Some("github.com/example/router".to_owned()),
            basename: "router".to_owned(),
            fallback_cwd: None,
        }))
        .await
        .unwrap();
    let selected: Vec<String> = with_origin
        .iter()
        .map(|row| row.get::<String, _>("id"))
        .collect();

    // Assert: the same repository by origin, plus origin-free rows by path or
    // basename; never another repository's row rescued by its basename.
    assert!(selected.contains(&"same-origin".to_owned()));
    assert!(selected.contains(&"no-origin-live-root".to_owned()));
    assert!(selected.contains(&"no-origin-worktree-basename".to_owned()));
    assert!(!selected.contains(&"other-origin-matching-basename".to_owned()));
    assert!(!selected.contains(&"unrelated".to_owned()));

    // Act: an identity with no origin falls back to live roots, and basename
    // only for rows that name no origin.
    let without_origin = catalog
        .read_page(&repo_query(StoredThreadRoot::Repo {
            live_roots: vec![PathBuf::from("/work/router")],
            normalized_origin: None,
            basename: "router".to_owned(),
            fallback_cwd: None,
        }))
        .await
        .unwrap();
    let selected: Vec<String> = without_origin
        .iter()
        .map(|row| row.get::<String, _>("id"))
        .collect();

    // Assert.
    assert!(selected.contains(&"other-origin-matching-basename".to_owned()));
    assert!(selected.contains(&"no-origin-live-root".to_owned()));
    assert!(selected.contains(&"no-origin-worktree-basename".to_owned()));
    assert!(!selected.contains(&"same-origin".to_owned()));
    assert!(!selected.contains(&"unrelated".to_owned()));

    // Assert: a NULL reasoning effort stays absent rather than becoming a value.
    let effort: Option<String> = with_origin
        .iter()
        .find(|row| row.get::<String, _>("id") == "no-origin-live-root")
        .unwrap()
        .get("reasoning_effort");
    assert_eq!(effort, None);
    std::fs::remove_file(home.join("state_5.sqlite")).unwrap();
    std::fs::remove_dir(home).unwrap();
}

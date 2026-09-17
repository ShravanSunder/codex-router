//! The stored `--repo` SQL scope must admit exactly the rows the human catalog admits.
//!
//! `repository_contains_session` is the canonical truth table. These tests execute the
//! real SQL against a real catalog and compare it row for row against that predicate,
//! so the Control listing and the human picker cannot drift apart again.

#[cfg(test)]
mod tests {
    use codex_native_integration::{
        StoredThreadCatalog, StoredThreadProvider, StoredThreadQuery, StoredThreadRoot,
        StoredThreadSort, StoredThreadSource,
    };
    use collaboration_client::session_catalog::{RepositoryIdentity, repository_contains_session};
    use sqlx::Row;
    use std::path::{Path, PathBuf};

    struct ThreadRow {
        id: &'static str,
        cwd: &'static str,
        git_origin_url: Option<&'static str>,
        name: Option<&'static str>,
        title: &'static str,
        source: &'static str,
        thread_source: Option<&'static str>,
        reasoning_effort: Option<&'static str>,
        updated_at_ms: i64,
    }

    impl ThreadRow {
        const fn new(id: &'static str, cwd: &'static str, updated_at_ms: i64) -> Self {
            Self {
                id,
                cwd,
                git_origin_url: None,
                name: None,
                title: "derived title",
                source: "cli",
                thread_source: Some("user"),
                reasoning_effort: None,
                updated_at_ms,
            }
        }

        const fn with_origin(mut self, git_origin_url: &'static str) -> Self {
            self.git_origin_url = Some(git_origin_url);
            self
        }
    }

    async fn catalog_home(fixture_name: &str, rows: &[ThreadRow]) -> PathBuf {
        let home = std::env::temp_dir().join(format!(
            "collaboration-client-{fixture_name}-{}",
            std::process::id()
        ));
        let _ignored = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        let options = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(home.join("state_5.sqlite"))
            .create_if_missing(true);
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();
        sqlx::raw_sql(
            "CREATE TABLE threads (id TEXT PRIMARY KEY, rollout_path TEXT, cwd TEXT, \
             model_provider TEXT, model TEXT, reasoning_effort TEXT, source TEXT, \
             thread_source TEXT, git_branch TEXT, git_origin_url TEXT, name TEXT, title TEXT, \
             preview TEXT, first_user_message TEXT, created_at_ms INTEGER, updated_at_ms INTEGER, \
             recency_at_ms INTEGER, archived INTEGER); \
             CREATE INDEX idx_threads_updated_at_ms ON threads(updated_at_ms DESC, id DESC);",
        )
        .execute(&pool)
        .await
        .unwrap();
        for row in rows {
            sqlx::query(
                "INSERT INTO threads (id, cwd, model, reasoning_effort, source, thread_source, \
                 git_origin_url, name, title, created_at_ms, updated_at_ms, recency_at_ms, archived) \
                 VALUES (?, ?, 'gpt-5.6-sol', ?, ?, ?, ?, ?, ?, ?, ?, ?, 0)",
            )
            .bind(row.id)
            .bind(row.cwd)
            .bind(row.reasoning_effort)
            .bind(row.source)
            .bind(row.thread_source)
            .bind(row.git_origin_url)
            .bind(row.name)
            .bind(row.title)
            .bind(row.updated_at_ms)
            .bind(row.updated_at_ms)
            .bind(row.updated_at_ms)
            .execute(&pool)
            .await
            .unwrap();
        }
        pool.close().await;
        home
    }

    async fn matching_ids(home: &Path, query: &StoredThreadQuery) -> Vec<String> {
        let catalog = StoredThreadCatalog::open(home).await.unwrap();
        let rows = catalog.read_page(query).await.unwrap();
        let ids = rows
            .iter()
            .map(|row| row.get::<String, _>("id"))
            .collect::<Vec<_>>();
        catalog.close().await;
        ids
    }

    fn repo_query(identity: &RepositoryIdentity, page_size: usize) -> StoredThreadQuery {
        StoredThreadQuery {
            root: StoredThreadRoot::Repo {
                live_roots: identity.live_roots.clone(),
                normalized_origin: identity.normalized_origin.clone(),
                basename: identity.repository_basename.clone(),
                fallback_cwd: identity.fallback_cwd.clone(),
            },
            provider: StoredThreadProvider::Any,
            source: StoredThreadSource::All,
            sort: StoredThreadSort::Updated,
            page_size,
            cursor: None,
            query: None,
        }
    }

    fn canonical_ids(identity: &RepositoryIdentity, rows: &[ThreadRow]) -> Vec<String> {
        let mut accepted = rows
            .iter()
            .filter(|row| {
                repository_contains_session(identity, row.git_origin_url, Path::new(row.cwd))
            })
            .collect::<Vec<_>>();
        accepted.sort_by(|left, right| {
            right
                .updated_at_ms
                .cmp(&left.updated_at_ms)
                .then_with(|| right.id.cmp(left.id))
        });
        accepted.iter().map(|row| row.id.to_owned()).collect()
    }

    const SCOPE_ROWS: &[ThreadRow] = &[
        // Row origin equals the caller's repository, written in a different URL spelling.
        ThreadRow::new("same-origin-ssh", "/dev/anywhere", 100)
            .with_origin("git@github.com:shravan/ai-tools.git"),
        // The work fork: a different origin whose directory leaf matches the basename.
        ThreadRow::new("other-origin-matching-leaf", "/dev/ai-tools-work", 90)
            .with_origin("https://github.com/shravan/ai-tools-work.git"),
        // No origin at all, inside a live worktree root.
        ThreadRow::new("no-origin-live-root", "/dev/ai-tools/crates", 80),
        // No origin, deleted worktree whose leaf is exactly the repository name.
        ThreadRow::new("no-origin-exact-leaf", "/history/ai-tools", 70),
        // No origin, deleted worktree whose leaf is the repository name plus a suffix.
        ThreadRow::new("no-origin-suffixed-leaf", "/history/ai-tools-listening", 60),
        // No origin, unrelated directory.
        ThreadRow::new("no-origin-unrelated", "/history/other-project", 50),
        // An origin present while the caller has none, inside a live root.
        ThreadRow::new("origin-inside-live-root", "/dev/ai-tools/docs", 40)
            .with_origin("https://github.com/shravan/ai-tools.git"),
    ];

    fn identity_with_origin() -> RepositoryIdentity {
        RepositoryIdentity {
            normalized_origin: Some("github.com/shravan/ai-tools".to_owned()),
            live_roots: vec![PathBuf::from("/dev/ai-tools")],
            repository_basename: "ai-tools".to_owned(),
            fallback_cwd: None,
        }
    }

    fn identity_without_origin() -> RepositoryIdentity {
        RepositoryIdentity {
            normalized_origin: None,
            live_roots: vec![PathBuf::from("/dev/ai-tools")],
            repository_basename: "ai-tools".to_owned(),
            fallback_cwd: None,
        }
    }

    #[tokio::test]
    async fn repo_scope_sql_admits_exactly_the_rows_the_human_catalog_admits() {
        // Arrange
        let home = catalog_home("repo-scope-truth-table", SCOPE_ROWS).await;

        for (case, identity) in [
            ("identity with origin", identity_with_origin()),
            ("identity without origin", identity_without_origin()),
        ] {
            // Act
            let sql_ids = matching_ids(&home, &repo_query(&identity, 100)).await;

            // Assert
            assert_eq!(
                sql_ids,
                canonical_ids(&identity, SCOPE_ROWS),
                "{case}: SQL scope diverged from repository_contains_session"
            );
        }

        // The work fork is the divergence this test exists to pin.
        let with_origin = matching_ids(&home, &repo_query(&identity_with_origin(), 100)).await;
        assert!(
            !with_origin.contains(&"other-origin-matching-leaf".to_owned()),
            "a different origin must not be rescued by a matching directory leaf"
        );
        assert!(with_origin.contains(&"same-origin-ssh".to_owned()));
        assert!(with_origin.contains(&"no-origin-suffixed-leaf".to_owned()));

        let without_origin =
            matching_ids(&home, &repo_query(&identity_without_origin(), 100)).await;
        assert!(
            !without_origin.contains(&"other-origin-matching-leaf".to_owned()),
            "a row that carries an origin may only match through a live worktree root"
        );
        assert!(without_origin.contains(&"origin-inside-live-root".to_owned()));

        std::fs::remove_dir_all(home).unwrap();
    }

    const LISTING_ROWS: &[ThreadRow] = &[
        ThreadRow {
            id: "named-match",
            cwd: "/dev/ai-tools",
            git_origin_url: None,
            name: Some("Astra REVIEW seat"),
            title: "unrelated",
            source: "cli",
            thread_source: Some("user"),
            reasoning_effort: Some("high"),
            updated_at_ms: 500,
        },
        ThreadRow {
            id: "titled-match",
            cwd: "/dev/ai-tools",
            git_origin_url: None,
            name: None,
            title: "review the plan",
            source: "cli",
            thread_source: Some("user"),
            reasoning_effort: None,
            updated_at_ms: 400,
        },
        ThreadRow {
            id: "subagent-match",
            cwd: "/dev/ai-tools",
            git_origin_url: None,
            name: Some("review worker"),
            title: "child",
            source: "cli",
            thread_source: Some("subagent"),
            reasoning_effort: Some("low"),
            updated_at_ms: 300,
        },
        ThreadRow {
            id: "no-match",
            cwd: "/dev/ai-tools",
            git_origin_url: None,
            name: Some("unrelated seat"),
            title: "unrelated",
            source: "cli",
            thread_source: Some("user"),
            reasoning_effort: None,
            updated_at_ms: 200,
        },
    ];

    fn listing_query(
        source: StoredThreadSource,
        query: Option<&str>,
        page_size: usize,
    ) -> StoredThreadQuery {
        StoredThreadQuery {
            root: StoredThreadRoot::Any,
            provider: StoredThreadProvider::Any,
            source,
            sort: StoredThreadSort::Updated,
            page_size,
            cursor: None,
            query: query.map(str::to_owned),
        }
    }

    #[tokio::test]
    async fn source_and_query_filters_run_in_sql_and_keep_pagination_gap_free() {
        // Arrange
        let home = catalog_home("listing-filters", LISTING_ROWS).await;

        // Act / Assert: the query matches name and title, case-insensitively.
        assert_eq!(
            matching_ids(
                &home,
                &listing_query(StoredThreadSource::All, Some("REVIEW"), 100)
            )
            .await,
            ["named-match", "titled-match", "subagent-match"]
        );
        assert_eq!(
            matching_ids(
                &home,
                &listing_query(StoredThreadSource::All, Some("astra"), 100)
            )
            .await,
            ["named-match"],
            "name must match before title, and case must not matter"
        );

        // Act / Assert: source filtering is applied alongside the query.
        assert_eq!(
            matching_ids(
                &home,
                &listing_query(StoredThreadSource::Subagents, Some("review"), 100)
            )
            .await,
            ["subagent-match"]
        );
        assert_eq!(
            matching_ids(
                &home,
                &listing_query(StoredThreadSource::Interactive, Some("review"), 100)
            )
            .await,
            ["named-match", "titled-match"]
        );

        // Act / Assert: keyset pagination over a filtered result stays gap-free.
        let catalog = StoredThreadCatalog::open(&home).await.unwrap();
        let mut paged = Vec::new();
        let mut cursor = None;
        loop {
            let mut query = listing_query(StoredThreadSource::All, Some("review"), 1);
            query.cursor = cursor.take();
            let rows = catalog.read_page(&query).await.unwrap();
            let Some(row) = rows.into_iter().next() else {
                break;
            };
            let id: String = row.get("id");
            cursor = Some(codex_native_integration::StoredThreadCursor {
                sort_value: row.get("recency_at_ms"),
                session_id: id.clone(),
            });
            paged.push(id);
        }
        catalog.close().await;
        assert_eq!(paged, ["named-match", "titled-match", "subagent-match"]);

        std::fs::remove_dir_all(home).unwrap();
    }

    #[tokio::test]
    async fn reasoning_effort_reads_back_as_null_when_the_stored_column_is_null() {
        // Arrange
        let home = catalog_home("reasoning-effort-column", LISTING_ROWS).await;
        let catalog = StoredThreadCatalog::open(&home).await.unwrap();

        // Act
        let rows = catalog
            .read_page(&listing_query(StoredThreadSource::All, None, 100))
            .await
            .unwrap();
        let efforts = rows
            .iter()
            .map(|row| {
                (
                    row.get::<String, _>("id"),
                    row.get::<Option<String>, _>("reasoning_effort"),
                )
            })
            .collect::<Vec<_>>();
        catalog.close().await;

        // Assert
        assert_eq!(
            efforts,
            vec![
                ("named-match".to_owned(), Some("high".to_owned())),
                ("titled-match".to_owned(), None),
                ("subagent-match".to_owned(), Some("low".to_owned())),
                ("no-match".to_owned(), None),
            ]
        );

        std::fs::remove_dir_all(home).unwrap();
    }
}

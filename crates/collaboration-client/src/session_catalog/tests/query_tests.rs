use super::*;
use codex_native_integration::SessionSearchDocument;
use sqlx::Execute;

#[test]
fn session_record_pages_use_keyset_order_index_without_offset() {
    assert_eq!(SESSION_RECORD_PAGE_SIZE, 250);
    let mut first = session_record_page_query(
        SessionCatalogRoot::Any,
        SessionCatalogProvider::Any,
        SessionCatalogSource::All,
        SessionCatalogSort::Updated,
        None,
    );
    let first_sql = first.build().sql().as_str().to_owned();
    let mut later = session_record_page_query(
        SessionCatalogRoot::Any,
        SessionCatalogProvider::Any,
        SessionCatalogSource::All,
        SessionCatalogSort::Updated,
        Some(("thread-cursor", Some(42))),
    );
    let later_sql = later.build().sql().as_str().to_owned();

    assert!(first_sql.contains("INDEXED BY idx_threads_updated_at_ms"));
    assert!(!first_sql.contains("OFFSET"));
    assert!(later_sql.contains("updated_at_ms <"));
    assert!(later_sql.contains("updated_at_ms ="));
    assert!(later_sql.contains("id <"));
    assert!(!later_sql.contains("OFFSET"));

    let mut created = session_record_page_query(
        SessionCatalogRoot::Any,
        SessionCatalogProvider::Any,
        SessionCatalogSource::All,
        SessionCatalogSort::Created,
        Some(("thread-null-cursor", None)),
    );
    let created_sql = created.build().sql().as_str().to_owned();
    assert!(created_sql.contains("INDEXED BY idx_threads_created_at_ms"));
    assert!(created_sql.contains("created_at_ms IS NULL AND id <"));
    assert!(!created_sql.contains("OFFSET"));
}

#[test]
fn session_record_candidate_pages_do_not_shrink_to_the_remaining_match_limit() {
    assert_eq!(SESSION_RECORD_PAGE_SIZE, 250);
}

#[cfg(unix)]
#[test]
fn cwd_scope_defers_all_symlink_spellings_to_the_final_matcher() {
    let fixture_root = std::env::temp_dir().join(format!(
        "collaboration-client-cwd-final-matcher-{}",
        std::process::id()
    ));
    let canonical_parent = fixture_root.join("canonical-parent");
    let canonical_checkout = canonical_parent.join("checkout");
    let first_alias = fixture_root.join("first-alias");
    let second_alias = fixture_root.join("second-alias");
    std::fs::create_dir_all(&canonical_checkout).expect("create canonical checkout");
    std::os::unix::fs::symlink(&canonical_parent, &first_alias).expect("create first alias");
    std::os::unix::fs::symlink(&canonical_parent, &second_alias).expect("create second alias");
    let current_dir = first_alias.join("checkout");
    let persisted_cwd = second_alias.join("checkout");
    let root_filter = RootFilter::Cwd(path_identity_candidates(&current_dir));
    let native_query = native_session_record_query(
        &root_filter,
        &ProviderFilter::Any,
        SessionCatalogSource::All,
        SessionCatalogSort::Updated,
        SESSION_RECORD_PAGE_SIZE,
        None,
    );
    let mut page_query = codex_native_integration::stored_thread_page_query(&native_query);
    let query_sql = page_query.build().sql().as_str().to_owned();
    let record = StoredSessionRecord {
        session_id: "thread-symlink".to_owned(),
        rollout_path: None,
        cwd: Some(persisted_cwd.display().to_string()),
        provider: None,
        model: None,
        reasoning_effort: None,
        source: None,
        thread_source: None,
        git_branch: None,
        git_origin_url: None,
        name: None,
        title: None,
        preview: None,
        first_user_message: None,
        created_at_ms: None,
        updated_at_ms: None,
        recency_at_ms: None,
    };

    assert!(!query_sql.contains("cwd ="));
    assert!(!query_sql.contains("OFFSET"));
    assert!(session_record_matches_root(&record, &root_filter));
    std::fs::remove_file(first_alias).expect("remove first alias");
    std::fs::remove_file(second_alias).expect("remove second alias");
    std::fs::remove_dir_all(fixture_root).expect("remove fixture");
}

#[test]
fn checkout_scope_with_broken_git_metadata_uses_exact_cwd_filter() {
    let fixture_root = std::env::temp_dir().join(format!(
        "collaboration-client-broken-checkout-{}",
        std::process::id()
    ));
    let current_dir = fixture_root.join("nested");
    std::fs::create_dir_all(&current_dir).expect("create nested cwd");
    std::fs::write(fixture_root.join(".git"), "gitdir: missing\n")
        .expect("write broken git metadata");
    let query = SessionCatalogQuery {
        codex_home: fixture_root.join("codex-home"),
        current_dir,
        root: SessionCatalogRoot::Checkout,
        provider: SessionCatalogProvider::Any,
        source: SessionCatalogSource::All,
        sort: SessionCatalogSort::Updated,
        last: false,
        include_empty_sessions: false,
        limit: 100,
        search: String::new(),
        repository_identity: None,
    };

    assert!(matches!(RootFilter::from_query(&query), RootFilter::Cwd(_)));
    std::fs::remove_dir_all(fixture_root).expect("remove fixture");
}

#[test]
fn qualified_search_ands_terms_and_keeps_branch_out_of_bare_matching() {
    let document = SessionSearchDocument {
        session_id: "019abc-session",
        name: "Named router session",
        title: "Fix router crash",
        preview: "Investigate deleted worktree sessions",
        first_user_message: "please make search robust",
        branch: "main",
        origin: "github.com/shravan-agent/codex-router",
        cwd: "/dev/codex-router.impl-search",
    };
    assert!(SessionSearchExpression::parse("019abc").matches(&document));
    assert!(SessionSearchExpression::parse("id:019abc").matches(&document));
    assert!(SessionSearchExpression::parse("b:main").matches(&document));
    assert!(SessionSearchExpression::parse("branch:main crash").matches(&document));
    assert!(SessionSearchExpression::parse("repo:codex-router crash").matches(&document));
    assert!(SessionSearchExpression::parse("\"deleted worktree\"").matches(&document));
    assert!(!SessionSearchExpression::parse("main").matches(&document));
    assert!(!SessionSearchExpression::parse("b:feature").matches(&document));
    assert!(!SessionSearchExpression::parse("main crash").matches(&document));
}

#[test]
fn qualified_search_handles_unicode_literals_unknown_prefixes_and_empty_qualifiers() {
    let document = SessionSearchDocument {
        session_id: "thread-percent",
        name: "ÉCHEC named session",
        title: "ÉCHEC 100%_safe\\path",
        preview: "ticket:router",
        first_user_message: "",
        branch: "feature/Échec",
        origin: "github.com/Org/Repo",
        cwd: "/dev/Repo",
    };

    assert!(SessionSearchExpression::parse("échec").matches(&document));
    assert!(SessionSearchExpression::parse("100%_safe\\path").matches(&document));
    assert!(SessionSearchExpression::parse("ticket:router").matches(&document));
    assert!(!SessionSearchExpression::parse("id:").matches(&document));
    assert!(!SessionSearchExpression::parse("b:").matches(&document));
}

#[tokio::test]
async fn stored_picker_treats_upstream_empty_string_first_message_as_empty() {
    let home = tempfile::tempdir().expect("temporary Codex home");
    let database = home.path().join("state_5.sqlite");
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            sqlx::sqlite::SqliteConnectOptions::new()
                .filename(&database)
                .create_if_missing(true),
        )
        .await
        .expect("Codex state database");
    sqlx::raw_sql(
        "CREATE TABLE threads (
            id TEXT PRIMARY KEY, rollout_path TEXT, cwd TEXT, model_provider TEXT,
            model TEXT, reasoning_effort TEXT, source TEXT, thread_source TEXT,
            git_branch TEXT, git_origin_url TEXT, name TEXT, title TEXT, preview TEXT,
            first_user_message TEXT NOT NULL DEFAULT '', created_at_ms INTEGER,
            updated_at_ms INTEGER, recency_at_ms INTEGER, archived INTEGER
        );
        CREATE INDEX idx_threads_updated_at_ms ON threads(updated_at_ms DESC, id DESC);
        INSERT INTO threads (id,cwd,model,title,created_at_ms,updated_at_ms,recency_at_ms,archived)
            VALUES ('empty-live','/repo','gpt-5.6-sol','Empty live session',2,2,2,0);
        INSERT INTO threads (id,cwd,model,title,preview,first_user_message,created_at_ms,updated_at_ms,recency_at_ms,archived)
            VALUES ('real-session','/repo','gpt-5.6-sol','First message','hello','hello',1,1,1,0);",
    )
    .execute(&pool)
    .await
    .expect("upstream-shaped session rows");
    pool.close().await;

    let query = SessionCatalogQuery {
        codex_home: home.path().to_owned(),
        current_dir: home.path().to_owned(),
        root: SessionCatalogRoot::Any,
        provider: SessionCatalogProvider::Any,
        source: SessionCatalogSource::All,
        sort: SessionCatalogSort::Updated,
        last: false,
        include_empty_sessions: false,
        limit: 10,
        search: String::new(),
        repository_identity: None,
    };

    let visible = load_stored_sessions(query.clone())
        .await
        .expect("default stored inventory");
    assert_eq!(
        visible
            .iter()
            .map(|record| record.session_id.as_str())
            .collect::<Vec<_>>(),
        ["real-session"]
    );
    let all = load_stored_sessions(SessionCatalogQuery {
        include_empty_sessions: true,
        ..query
    })
    .await
    .expect("explicit empty-session inventory");
    assert_eq!(all.len(), 2);
}

#[tokio::test]
async fn stored_picker_hides_upstream_empty_string_rows_by_default() {
    let directory = tempfile::tempdir().expect("temporary Codex home");
    let database = directory.path().join("state_5.sqlite");
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            sqlx::sqlite::SqliteConnectOptions::new()
                .filename(&database)
                .create_if_missing(true),
        )
        .await
        .expect("Codex state database");
    sqlx::raw_sql(
        "CREATE TABLE threads (
            id TEXT PRIMARY KEY, rollout_path TEXT, cwd TEXT, model_provider TEXT,
            model TEXT, reasoning_effort TEXT, source TEXT, thread_source TEXT,
            git_branch TEXT, git_origin_url TEXT, name TEXT, title TEXT, preview TEXT,
            first_user_message TEXT NOT NULL DEFAULT '', created_at_ms INTEGER,
            updated_at_ms INTEGER, recency_at_ms INTEGER, archived INTEGER
        );
        CREATE INDEX idx_threads_updated_at_ms ON threads(updated_at_ms DESC, id DESC);
        INSERT INTO threads (id,cwd,model,title,created_at_ms,updated_at_ms,recency_at_ms,archived)
            VALUES ('empty-live','/repo','gpt-5.6-sol','Empty live session',2,2,2,0);
        INSERT INTO threads (id,cwd,model,title,preview,first_user_message,created_at_ms,updated_at_ms,recency_at_ms,archived)
            VALUES ('real-session','/repo','gpt-5.6-sol','First message','hello','hello',1,1,1,0);",
    )
    .execute(&pool)
    .await
    .expect("upstream-shaped session rows");
    pool.close().await;

    let query = SessionCatalogQuery {
        codex_home: directory.path().to_owned(),
        current_dir: directory.path().to_owned(),
        root: SessionCatalogRoot::Any,
        provider: SessionCatalogProvider::Any,
        source: SessionCatalogSource::All,
        sort: SessionCatalogSort::Updated,
        last: false,
        include_empty_sessions: false,
        limit: 10,
        search: String::new(),
        repository_identity: None,
    };

    let visible = load_stored_sessions(query.clone())
        .await
        .expect("default stored inventory");
    assert_eq!(
        visible
            .iter()
            .map(|record| record.session_id.as_str())
            .collect::<Vec<_>>(),
        ["real-session"]
    );
    let all = load_stored_sessions(SessionCatalogQuery {
        include_empty_sessions: true,
        ..query
    })
    .await
    .expect("explicit empty-session inventory");
    assert_eq!(all.len(), 2);
}

#[test]
fn current_provider_prefers_router_profile_config() {
    let root = std::env::temp_dir().join(format!(
        "collaboration-client-provider-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&root).expect("create Codex home");
    std::fs::write(root.join("config.toml"), "model_provider = \"fallback\"\n")
        .expect("write fallback");
    std::fs::write(
        root.join("codex-router.config.toml"),
        "model_provider = \"router\"\n",
    )
    .expect("write router");

    assert_eq!(current_session_provider(&root).expect("provider"), "router");
    std::fs::remove_dir_all(root).expect("remove fixture");
}

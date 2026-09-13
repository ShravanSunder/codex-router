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

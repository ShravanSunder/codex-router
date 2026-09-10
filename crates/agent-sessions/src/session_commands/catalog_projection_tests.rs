use super::*;

#[test]
fn session_record_pages_use_keyset_order_index_without_offset() {
    let mut first_page_builder = super::session_record_page_query(
        &super::RootFilter::Any,
        &super::ProviderFilter::Any,
        super::SessionsSource::All,
        super::SessionsSort::Updated,
        super::SESSION_RECORD_PAGE_SIZE,
        None,
    );
    let first_page_sql = first_page_builder.build().sql().as_str().to_owned();
    let cursor = super::SessionRecordPageCursor {
        sort_value: Some(42),
        session_id: "thread-cursor".to_owned(),
    };
    let mut later_page_builder = super::session_record_page_query(
        &super::RootFilter::Any,
        &super::ProviderFilter::Any,
        super::SessionsSource::All,
        super::SessionsSort::Updated,
        super::SESSION_RECORD_PAGE_SIZE,
        Some(&cursor),
    );
    let later_page_sql = later_page_builder.build().sql().as_str().to_owned();

    assert!(first_page_sql.contains("INDEXED BY idx_threads_updated_at_ms"));
    assert!(!first_page_sql.contains("OFFSET"));
    assert!(later_page_sql.contains("updated_at_ms <"));
    assert!(later_page_sql.contains("updated_at_ms ="));
    assert!(later_page_sql.contains("id <"));
    assert!(!later_page_sql.contains("OFFSET"));

    let null_cursor = super::SessionRecordPageCursor {
        sort_value: None,
        session_id: "thread-null-cursor".to_owned(),
    };
    let mut created_page_builder = super::session_record_page_query(
        &super::RootFilter::Any,
        &super::ProviderFilter::Any,
        super::SessionsSource::All,
        super::SessionsSort::Created,
        super::SESSION_RECORD_PAGE_SIZE,
        Some(&null_cursor),
    );
    let created_page_sql = created_page_builder.build().sql().as_str().to_owned();
    assert!(created_page_sql.contains("INDEXED BY idx_threads_created_at_ms"));
    assert!(created_page_sql.contains("created_at_ms IS NULL AND id <"));
    assert!(!created_page_sql.contains("OFFSET"));
}

#[cfg(unix)]
#[test]
fn session_record_candidate_pages_do_not_shrink_to_the_remaining_match_limit() {
    assert_eq!(super::session_record_candidate_page_size(), 250);
}

#[test]
fn picker_search_uses_the_same_complete_persisted_fields_as_loader_search() {
    let record = search_consistency_record(None, Some("deploy\nrollback plan"));
    let picker_record = SessionPickerRecord::from_record(&record);
    let expression = SessionSearchExpression::parse("\"deploy rollback\"");

    assert_eq!(
        picker_record.matches_search(&expression),
        record.matches_search(&expression)
    );
    assert!(!picker_record.matches_search(&expression));
}

#[test]
fn picker_search_preserves_raw_persisted_cwd_for_loader_parity() {
    let fixture_root = std::env::temp_dir().join(format!(
        "codex-router-session-picker-search-cwd-{}",
        std::process::id()
    ));
    let canonical_checkout = fixture_root.join("canonical-checkout");
    let checkout_alias = fixture_root.join("checkout-alias");
    fs::create_dir_all(&canonical_checkout).expect("create canonical checkout");
    std::os::unix::fs::symlink(&canonical_checkout, &checkout_alias)
        .expect("create checkout symlink");
    let mut record = search_consistency_record(None, None);
    record.cwd = Some(checkout_alias.display().to_string());
    let picker_record = SessionPickerRecord::from_record(&record);
    let alias_expression = SessionSearchExpression::parse("checkout-alias");
    let canonical_expression = SessionSearchExpression::parse("canonical-checkout");
    let actual = [
        picker_record.matches_search(&alias_expression),
        picker_record.matches_search(&canonical_expression),
    ];
    let expected = [
        record.matches_search(&alias_expression),
        record.matches_search(&canonical_expression),
    ];
    fs::remove_file(&checkout_alias).expect("remove checkout symlink");
    fs::remove_dir(&canonical_checkout).expect("remove canonical checkout");
    fs::remove_dir(&fixture_root).expect("remove fixture root");

    assert_eq!(actual, expected);
}

#[test]
fn picker_search_does_not_match_missing_branch_display_placeholder() {
    let mut record = search_consistency_record(None, None);
    record.git_branch = None;
    let picker_record = SessionPickerRecord::from_record(&record);
    let expression = SessionSearchExpression::parse("b:-");

    assert_eq!(
        picker_record.matches_search(&expression),
        record.matches_search(&expression)
    );
    assert!(!picker_record.matches_search(&expression));
}

#[cfg(unix)]
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

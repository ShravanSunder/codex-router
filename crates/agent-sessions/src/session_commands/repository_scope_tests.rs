use super::*;

#[test]
fn cwd_query_candidates_preserve_raw_symlink_and_canonical_paths() {
    let fixture_root = std::env::temp_dir().join(format!(
        "codex-router-session-cwd-query-candidates-{}",
        std::process::id()
    ));
    let canonical_checkout = fixture_root.join("canonical-checkout");
    let checkout_alias = fixture_root.join("checkout-alias");
    fs::create_dir_all(&canonical_checkout).expect("create canonical checkout");
    std::os::unix::fs::symlink(&canonical_checkout, &checkout_alias)
        .expect("create checkout symlink");

    let candidates = super::path_identity_candidates(&checkout_alias);
    let canonical_path = fs::canonicalize(&canonical_checkout).expect("canonicalize checkout");

    fs::remove_file(&checkout_alias).expect("remove checkout symlink");
    fs::remove_dir(&canonical_checkout).expect("remove canonical checkout");
    fs::remove_dir(&fixture_root).expect("remove fixture root");
    assert_eq!(candidates.len(), 2);
    assert!(candidates.contains(&checkout_alias));
    assert!(candidates.contains(&canonical_path));
}

#[cfg(unix)]
#[test]
fn cwd_scope_defers_all_symlink_spellings_to_the_final_matcher() {
    let fixture_root = std::env::temp_dir().join(format!(
        "codex-router-session-cwd-final-matcher-{}",
        std::process::id()
    ));
    let canonical_parent = fixture_root.join("canonical-parent");
    let canonical_checkout = canonical_parent.join("checkout");
    let first_alias = fixture_root.join("first-alias");
    let second_alias = fixture_root.join("second-alias");
    fs::create_dir_all(&canonical_checkout).expect("create canonical checkout");
    std::os::unix::fs::symlink(&canonical_parent, &first_alias).expect("create first alias");
    std::os::unix::fs::symlink(&canonical_parent, &second_alias).expect("create second alias");
    let current_dir = first_alias.join("checkout");
    let persisted_cwd = second_alias.join("checkout");
    let root_filter = super::RootFilter::Cwd(super::path_identity_candidates(&current_dir));
    let mut record = search_consistency_record(None, None);
    record.cwd = Some(persisted_cwd.display().to_string());
    let mut query = super::session_record_page_query(
        &root_filter,
        &super::ProviderFilter::Any,
        super::SessionsSource::All,
        super::SessionsSort::Updated,
        super::SESSION_RECORD_PAGE_SIZE,
        None,
    );
    let query_sql = query.build().sql().as_str().to_owned();
    let record_matches = super::session_record_matches_root(&record, &root_filter);

    fs::remove_file(&first_alias).expect("remove first alias");
    fs::remove_file(&second_alias).expect("remove second alias");
    fs::remove_dir(&canonical_checkout).expect("remove canonical checkout");
    fs::remove_dir(&canonical_parent).expect("remove canonical parent");
    fs::remove_dir(&fixture_root).expect("remove fixture root");
    assert!(!query_sql.contains("cwd ="));
    assert!(record_matches);
}

#[test]
fn git_origin_normalization_compares_common_transport_forms() {
    let expected = Some("github.com/shravan-agent/codex-router".to_owned());

    assert_eq!(
        normalize_git_origin_url("https://github.com/shravan-agent/codex-router.git"),
        expected
    );
    assert_eq!(
        normalize_git_origin_url("git@github.com:shravan-agent/codex-router.git"),
        expected
    );
    assert_eq!(
        normalize_git_origin_url("ssh://git@github.com/shravan-agent/codex-router/"),
        expected
    );
    assert_eq!(
        normalize_git_origin_url(
            "https://user:secret@GitHub.com/shravan-agent/codex-router.git?token=secret#branch",
        ),
        expected
    );
    assert_eq!(
        normalize_git_origin_url("github.com/shravan-agent/codex-router"),
        expected
    );
}

#[test]
fn picker_repository_matching_normalizes_persisted_origin_exactly_once() {
    let raw_origin = "http://gitlab.internal:8443/team/app.git";
    let identity = RepositoryIdentity {
        normalized_origin: normalize_git_origin_url(raw_origin),
        live_roots: vec![PathBuf::from("/dev/app")],
        repository_basename: "app".to_owned(),
        fallback_cwd: None,
    };
    let record = search_consistency_record(Some(raw_origin), None);
    let picker_record = SessionPickerRecord::from_record(&record);

    assert!(session_belongs_to_repository(
        &identity,
        picker_record.git_origin_url.as_deref(),
        std::path::Path::new(picker_record.cwd.as_deref().expect("record cwd")),
    ));
}

#[test]
fn repository_membership_uses_origin_precedence_and_bounded_path_fallbacks() {
    let identity = RepositoryIdentity {
        normalized_origin: normalize_git_origin_url(
            "https://github.com/shravan-agent/codex-router.git",
        ),
        live_roots: vec![PathBuf::from("/dev/codex-router.live")],
        repository_basename: "codex-router".to_owned(),
        fallback_cwd: None,
    };

    let cases = [
        (
            "matching origin survives deleted worktree",
            Some("git@github.com:shravan-agent/codex-router.git"),
            "/history/unrelated-name",
            true,
        ),
        (
            "conflicting origin overrides live-root shape",
            Some("https://github.com/other/codex-router.git"),
            "/dev/codex-router.live/src",
            false,
        ),
        (
            "missing origin uses live root",
            None,
            "/dev/codex-router.live/src",
            true,
        ),
        (
            "missing origin uses dotted historical basename",
            None,
            "/history/codex-router.impl-search",
            true,
        ),
        (
            "missing origin rejects a prefixed lookalike",
            None,
            "/history/my-codex-router",
            false,
        ),
    ];

    for (name, row_origin, cwd, expected) in cases {
        assert_eq!(
            session_belongs_to_repository(&identity, row_origin, std::path::Path::new(cwd)),
            expected,
            "{name}"
        );
    }
}

#[test]
fn unknown_current_origin_never_uses_deleted_path_fallback_for_present_row_origin() {
    let identity = RepositoryIdentity {
        normalized_origin: None,
        live_roots: vec![PathBuf::from("/dev/codex-router.live")],
        repository_basename: "codex-router".to_owned(),
        fallback_cwd: None,
    };

    assert!(session_belongs_to_repository(
        &identity,
        Some("https://github.com/shravan-agent/codex-router.git"),
        std::path::Path::new("/dev/codex-router.live/src"),
    ));
    assert!(!session_belongs_to_repository(
        &identity,
        Some("https://github.com/shravan-agent/codex-router.git"),
        std::path::Path::new("/history/codex-router.impl-old"),
    ));
    assert!(session_belongs_to_repository(
        &identity,
        None,
        std::path::Path::new("/history/codex-router.impl-old"),
    ));
}

#[test]
fn empty_repository_basename_does_not_admit_dot_or_dash_prefixed_paths() {
    let identity = RepositoryIdentity {
        normalized_origin: None,
        live_roots: Vec::new(),
        repository_basename: String::new(),
        fallback_cwd: None,
    };

    assert!(!session_belongs_to_repository(
        &identity,
        None,
        std::path::Path::new("/history/.cache"),
    ));
    assert!(!session_belongs_to_repository(
        &identity,
        None,
        std::path::Path::new("/history/-scratch"),
    ));
}

#[test]
fn repository_scope_without_git_metadata_matches_only_the_exact_cwd() {
    let current_dir = std::env::temp_dir().join(format!(
        "codex-router-non-repository-sessions-scope-{}",
        std::process::id()
    ));
    let identity = RepositoryIdentity::discover(&current_dir);

    assert!(session_belongs_to_repository(&identity, None, &current_dir,));
    assert!(!session_belongs_to_repository(
        &identity,
        None,
        &current_dir.join("nested-repository"),
    ));
}

#[test]
fn repository_scope_with_broken_git_metadata_falls_back_to_the_exact_cwd() {
    let repository_root = std::env::temp_dir().join(format!(
        "codex-router-broken-repository-sessions-scope-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
    ));
    let current_dir = repository_root.join("nested");
    fs::create_dir_all(&current_dir).expect("nested test cwd should be created");
    fs::write(repository_root.join(".git"), "gitdir: missing\n")
        .expect("broken git metadata should be created");

    let identity = RepositoryIdentity::discover(&current_dir);
    let context = crate::CliContext::new(Vec::new()).with_current_dir(current_dir.clone());

    assert!(session_belongs_to_repository(&identity, None, &current_dir));
    assert!(!session_belongs_to_repository(
        &identity,
        None,
        &repository_root,
    ));
    assert!(matches!(
        RootFilter::from_query(SessionsRoot::Checkout, &context, None),
        RootFilter::Cwd(_)
    ));

    fs::remove_file(repository_root.join(".git")).expect("broken git metadata should be removed");
    fs::remove_dir(&current_dir).expect("nested test cwd should be removed");
    fs::remove_dir(&repository_root).expect("test repository root should be removed");
}

#[test]
fn partial_git_evidence_retains_the_current_checkout_as_a_live_root() {
    let current_checkout = PathBuf::from("/repo/project");

    assert_eq!(
        live_roots_with_current_checkout_fallback(Vec::new(), &current_checkout, true,),
        vec![current_checkout.clone()]
    );
    assert!(
        live_roots_with_current_checkout_fallback(Vec::new(), &current_checkout, false).is_empty()
    );
}

#[test]
fn repository_basename_prefers_origin_and_git_common_directory_over_invoking_worktree() {
    assert_eq!(
        repository_basename_from_evidence(
            normalize_git_origin_url("git@github.com:shravan-agent/codex-router.git").as_deref(),
            Some(std::path::Path::new("/dev/codex-router/.git")),
            Some(std::path::Path::new("/dev/codex-router")),
            std::path::Path::new("/dev/codex-router.impl-y"),
        ),
        "codex-router"
    );
    assert_eq!(
        repository_basename_from_evidence(
            None,
            Some(std::path::Path::new("/dev/codex-router/.git")),
            Some(std::path::Path::new("/dev/codex-router.impl-x")),
            std::path::Path::new("/dev/codex-router.impl-y"),
        ),
        "codex-router"
    );
}

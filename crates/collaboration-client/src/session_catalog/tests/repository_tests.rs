use super::*;

#[cfg(unix)]
#[test]
fn cwd_query_candidates_preserve_raw_symlink_and_canonical_paths() {
    let root = std::env::temp_dir().join(format!(
        "collaboration-client-cwd-candidates-{}",
        std::process::id()
    ));
    let canonical = root.join("canonical");
    let alias = root.join("alias");
    std::fs::create_dir_all(&canonical).expect("create canonical");
    std::os::unix::fs::symlink(&canonical, &alias).expect("create alias");

    let candidates = path_identity_candidates(&alias);
    assert_eq!(candidates.len(), 2);
    assert!(candidates.contains(&alias));
    assert!(candidates.contains(&canonical.canonicalize().expect("canonicalize")));
    std::fs::remove_file(alias).expect("remove alias");
    std::fs::remove_dir_all(root).expect("remove fixture");
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
fn repository_membership_uses_origin_precedence_and_bounded_path_fallbacks() {
    let identity = RepositoryIdentity {
        normalized_origin: normalize_git_origin_url("https://github.com/example/router.git"),
        live_roots: vec![PathBuf::from("/dev/router.live")],
        repository_basename: "router".to_owned(),
        fallback_cwd: None,
    };
    let cases = [
        (
            Some("git@github.com:example/router.git"),
            "/history/unrelated",
            true,
        ),
        (
            Some("https://github.com/other/router.git"),
            "/dev/router.live/src",
            false,
        ),
        (None, "/dev/router.live/src", true),
        (None, "/history/router.impl-search", true),
        (None, "/history/my-router", false),
    ];

    for (origin, cwd, expected) in cases {
        assert_eq!(
            repository_contains_session(&identity, origin, Path::new(cwd)),
            expected
        );
    }
}

#[test]
fn unknown_current_origin_never_uses_deleted_path_fallback_for_present_row_origin() {
    let identity = RepositoryIdentity {
        normalized_origin: None,
        live_roots: vec![PathBuf::from("/dev/router.live")],
        repository_basename: "router".to_owned(),
        fallback_cwd: None,
    };

    assert!(repository_contains_session(
        &identity,
        Some("https://github.com/example/router.git"),
        Path::new("/dev/router.live/src")
    ));
    assert!(!repository_contains_session(
        &identity,
        Some("https://github.com/example/router.git"),
        Path::new("/history/router.impl-old")
    ));
    assert!(repository_contains_session(
        &identity,
        None,
        Path::new("/history/router.impl-old")
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
    assert!(!repository_contains_session(
        &identity,
        None,
        Path::new("/history/.cache")
    ));
    assert!(!repository_contains_session(
        &identity,
        None,
        Path::new("/history/-scratch")
    ));
}

#[test]
fn repository_scope_without_git_metadata_matches_only_the_exact_cwd() {
    let current_dir = std::env::temp_dir().join(format!(
        "collaboration-client-non-repo-{}",
        std::process::id()
    ));
    let identity = discover_repository_identity(&current_dir);
    assert!(repository_contains_session(&identity, None, &current_dir));
    assert!(!repository_contains_session(
        &identity,
        None,
        &current_dir.join("nested")
    ));
}

#[test]
fn repository_scope_with_broken_git_metadata_falls_back_to_the_exact_cwd() {
    let root = std::env::temp_dir().join(format!(
        "collaboration-client-broken-repo-{}",
        std::process::id()
    ));
    let current_dir = root.join("nested");
    std::fs::create_dir_all(&current_dir).expect("create nested");
    std::fs::write(root.join(".git"), "gitdir: missing\n").expect("write broken metadata");

    let identity = discover_repository_identity(&current_dir);
    assert!(repository_contains_session(&identity, None, &current_dir));
    assert!(!repository_contains_session(&identity, None, &root));
    std::fs::remove_dir_all(root).expect("remove fixture");
}

#[test]
fn repository_basename_prefers_origin_and_git_common_directory_over_invoking_worktree() {
    assert_eq!(
        repository_basename_from_evidence(
            normalize_git_origin_url("git@github.com:example/router.git").as_deref(),
            Some(Path::new("/dev/router/.git")),
            Some(Path::new("/dev/router")),
            Path::new("/dev/router.impl-y"),
        ),
        "router"
    );
    assert_eq!(
        repository_basename_from_evidence(
            None,
            Some(Path::new("/dev/router/.git")),
            Some(Path::new("/dev/router.impl-x")),
            Path::new("/dev/router.impl-y"),
        ),
        "router"
    );
}

#[test]
fn partial_git_evidence_retains_the_current_checkout_as_a_live_root() {
    let checkout = PathBuf::from("/repo/project");
    assert_eq!(
        live_roots_with_current_checkout_fallback(Vec::new(), &checkout, true).as_slice(),
        std::slice::from_ref(&checkout)
    );
    assert!(live_roots_with_current_checkout_fallback(Vec::new(), &checkout, false).is_empty());
}

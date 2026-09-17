use super::*;
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

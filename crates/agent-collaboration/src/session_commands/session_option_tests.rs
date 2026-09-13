use super::*;

#[test]
fn debug_session_profile_selection_matches_router_default_boundary() {
    use codex_native_integration::SessionProfile;
    assert_eq!(
        super::session_profile_for_environment(true, false),
        SessionProfile::RouterDebug
    );
    assert_eq!(
        super::session_profile_for_environment(true, true),
        SessionProfile::Router
    );
    assert_eq!(
        super::session_profile_for_environment(false, false),
        SessionProfile::Router
    );
}

#[test]
fn interactive_sessions_default_to_cwd_and_reject_checkout() {
    let command = SessionsCommand::parse(Vec::new()).expect("interactive command should parse");
    assert_eq!(command.root, SessionsRoot::Cwd);

    let repo = SessionsCommand::parse(vec!["--repo".into()])
        .expect("explicit repository scope should parse");
    assert_eq!(repo.root, SessionsRoot::Repo);

    let error = SessionsCommand::parse(vec!["--checkout".into()])
        .expect_err("interactive checkout cannot be represented by the picker");
    assert!(error.contains("--checkout requires --list"), "{error}");
}

#[test]
fn list_sessions_preserves_default_cwd_and_explicit_checkout() {
    let default_list =
        SessionsCommand::parse(vec!["--list".into()]).expect("default list command should parse");
    assert_eq!(default_list.root, SessionsRoot::Cwd);

    let checkout_list = SessionsCommand::parse(vec!["--checkout".into(), "--list".into()])
        .expect("checkout list command should parse");
    assert_eq!(checkout_list.root, SessionsRoot::Checkout);
}

#[test]
fn positional_session_id_after_an_option_is_rejected_instead_of_forwarded() {
    let error = SessionsCommand::parse(vec![
        "--local".into(),
        "019ff0bb-5993-70d3-b1ba-f56724b94919".into(),
    ])
    .expect_err("a misplaced positional session id must not become a Codex argument");

    assert!(
        error.contains("session UUID must be the first argument or use --id"),
        "{error}"
    );
}

#[test]
fn near_miss_positional_session_id_is_rejected_instead_of_forwarded() {
    for near_miss in [
        "019ff05e-77c0-7831-8f68-40bf182509f",
        "019ff05g-77c0-7831-8f68-40bf182509f6",
        "019ff05e77c078318f6840bf182509f6",
    ] {
        let error = SessionsCommand::parse(vec![near_miss.into()])
            .expect_err("a positional UUID typo must not become a Codex argument");

        assert!(error.contains("complete UUID"), "{near_miss}: {error}");
    }
}

#[test]
fn sessions_command_preserves_escaped_uuid_passthrough_without_resume_mode() {
    let command = SessionsCommand::parse(vec![
        "--new".into(),
        "--".into(),
        "--request-id".into(),
        "11111111-1111-4111-8111-111111111111".into(),
    ])
    .expect("an escaped UUID belongs to Codex passthrough arguments");

    assert!(command.new);
    assert!(command.id.is_none());
    assert_eq!(
        command.codex_args,
        [
            std::ffi::OsString::from("--request-id"),
            std::ffi::OsString::from("11111111-1111-4111-8111-111111111111"),
        ]
    );
}

#[test]
fn positional_session_id_conflicts_with_explicit_id() {
    for arguments in [
        vec![
            "019ff05e-77c0-7831-8f68-40bf182509f6".into(),
            "--id".into(),
            "11111111-1111-4111-8111-111111111111".into(),
        ],
        vec![
            "019ff05e-77c0-7831-8f68-40bf182509f6".into(),
            "--id=11111111-1111-4111-8111-111111111111".into(),
        ],
    ] {
        let error = SessionsCommand::parse(arguments)
            .expect_err("two exact resume IDs must not silently choose one");

        assert_eq!(
            error,
            "positional session UUID cannot be combined with --id"
        );
    }
}

#[test]
fn codex_home_resolution_uses_real_home_without_debug_redirect() {
    let home = PathBuf::from("/tmp/codex-router-home-policy");
    let explicit_codex_home = PathBuf::from("/tmp/explicit-codex-home");

    assert_eq!(
        codex_home_from_environment(None, Some(home.clone()))
            .expect("HOME should resolve Codex home"),
        home.join(".codex")
    );
    assert_eq!(
        codex_home_from_environment(Some(explicit_codex_home.clone()), Some(home))
            .expect("CODEX_HOME should win"),
        explicit_codex_home
    );
}

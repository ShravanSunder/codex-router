use super::*;

#[cfg(debug_assertions)]
#[test]
fn debug_plaintext_serve_accepts_explicit_isolated_storage_opt_in() {
    let root = tempfile::tempdir().expect("private parser fixture");
    let arguments = vec![
        OsString::from("codex-router"),
        OsString::from("serve"),
        OsString::from("--require-debug-isolation"),
        OsString::from("--state-db"),
        root.path().join("state.sqlite").into_os_string(),
        OsString::from("--secret-root"),
        root.path().join("secrets").into_os_string(),
        OsString::from("--allow-plaintext-file-secrets"),
    ];
    let command = CliCommand::parse(arguments);
    assert!(
        matches!(command, Ok(CliCommand::Serve(_))),
        "explicit debug plaintext opt-in must parse: {command:?}"
    );
}

#[cfg(debug_assertions)]
#[test]
fn debug_plaintext_account_login_accepts_explicit_isolated_root_opt_in() {
    let root = tempfile::tempdir().expect("private parser fixture");
    let arguments = vec![
        OsString::from("codex-router"),
        OsString::from("account"),
        OsString::from("login"),
        OsString::from("--provider"),
        OsString::from("openai"),
        OsString::from("--label"),
        OsString::from("fixture"),
        OsString::from("--router-root"),
        root.path().to_path_buf().into_os_string(),
        OsString::from("--allow-plaintext-file-secrets"),
    ];
    let command = CliCommand::parse(arguments);
    assert!(
        matches!(
            command,
            Ok(CliCommand::Account(
                crate::account::AccountCommand::Login { .. }
            ))
        ),
        "explicit debug plaintext login opt-in must parse: {command:?}"
    );
}

#[cfg(debug_assertions)]
#[test]
fn debug_plaintext_opt_in_requires_explicit_isolation_paths() {
    for (arguments, missing) in [
        (
            vec!["serve", "--allow-plaintext-file-secrets"],
            "--require-debug-isolation",
        ),
        (
            vec![
                "serve",
                "--allow-plaintext-file-secrets",
                "--require-debug-isolation",
            ],
            "--state-db",
        ),
        (
            vec![
                "account",
                "login",
                "--label",
                "fixture",
                "--allow-plaintext-file-secrets",
            ],
            "--router-root",
        ),
    ] {
        assert!(
            matches!(CliCommand::parse(arguments.into_iter().map(OsString::from)), Err(CliError::MissingOption { option }) if option == missing)
        );
    }
    assert!(
        CliCommand::parse(
            [
                "account",
                "login",
                "--label",
                "fixture",
                "--router-root",
                "relative-root",
                "--allow-plaintext-file-secrets"
            ]
            .map(OsString::from)
        )
        .is_err()
    );
}

#[cfg(debug_assertions)]
#[test]
fn debug_plaintext_help_explains_the_explicit_root_policy() {
    for arguments in [vec!["serve", "--help"], vec!["account", "login", "--help"]] {
        let command = CliCommand::parse(arguments.into_iter().map(OsString::from)).expect("help");
        let text = match command {
            CliCommand::Help(text)
            | CliCommand::Account(crate::account::AccountCommand::Help(text)) => text,
            _ => panic!("expected help"),
        };
        assert!(text.contains("debug-only"));
        assert!(text.contains("--allow-plaintext-file-secrets"));
        assert!(text.contains("plaintext without Keychain"));
        assert!(text.contains("root remembers this mode"));
    }
}

#[cfg(not(debug_assertions))]
#[test]
fn debug_plaintext_release_parser_rejects_both_opt_in_surfaces() {
    for arguments in [
        vec!["serve", "--allow-plaintext-file-secrets"],
        vec![
            "account",
            "login",
            "--label",
            "fixture",
            "--allow-plaintext-file-secrets",
        ],
    ] {
        assert!(
            matches!(CliCommand::parse(arguments.into_iter().map(OsString::from)), Err(CliError::UnknownOption { option }) if option == "--allow-plaintext-file-secrets")
        );
    }
}

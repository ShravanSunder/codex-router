use super::*;

#[test]
fn account_login_auth_json_writes_router_owned_state_and_guides_next_steps() {
    let test_root = TestRoot::new("account-login-auth-json");
    must_ok(fs::create_dir(test_root.path()));
    let router_root = test_root.path().join("router");
    let auth_json = test_root.path().join("auth.json");
    let id_token = fake_id_token_with_chatgpt_account_id("chatgpt-account-id-canary");
    let auth_json_text = format!(
        r#"{{"auth_mode":"chatgpt","tokens":{{"access_token":"access-token-canary","refresh_token":"refresh-token-canary","id_token":"{id_token}"}}}}"#
    );
    must_ok(fs::write(&auth_json, &auth_json_text));

    let output = run_cli(
        [
            "codex-router",
            "account",
            "login",
            "--router-root",
            path_to_str(&router_root),
            "--label",
            "primary",
            "--auth-json",
            path_to_str(&auth_json),
            "--allow-plaintext-file-secrets",
        ],
        CliContext::new(Vec::new()),
    );

    let account_id = account_id("acct_primary");
    let state = must_ok(SqliteStateStore::open(&router_root.join("state.sqlite")));
    let account = must_ok(AccountStateRepository::load_account(&state, &account_id))
        .unwrap_or_else(|| panic!("logged-in account metadata should exist"));
    assert_eq!(account.label(), "primary");
    assert_eq!(account.status(), AccountStatus::Enabled);
    assert_eq!(account.active_credential_generation(), Some(1));

    let secrets = must_ok(FileSecretStore::open(router_root.join("secrets")));
    let bundle_key = must_ok(account_credential_bundle_key(&account_id, 1));
    let bundle = must_ok(AccountCredentialBundle::from_secret_string(must_ok(
        secrets.read_secret(&bundle_key),
    )));
    assert_eq!(bundle.access_token().expose_secret(), "access-token-canary");
    assert_eq!(
        bundle.refresh_token().map(SecretString::expose_secret),
        Some("refresh-token-canary")
    );
    assert_eq!(
        bundle.chatgpt_account_id(),
        Some("chatgpt-account-id-canary")
    );
    assert!(output.stdout.contains("logged in account: primary\n"));
    assert!(output.stdout.contains("account_id: acct_primary\n"));
    assert!(
        output
            .stdout
            .contains("next: codex-router quota refresh --router-root ")
    );
    assert!(!output.stdout.contains("access-token-canary"));
    assert!(!output.stdout.contains("refresh-token-canary"));
    assert!(output.stderr.is_empty());
}

#[test]
fn account_login_device_auth_delegates_to_codex_and_imports_resulting_auth_json() {
    let test_root = TestRoot::new("account-login-device-auth");
    must_ok(fs::create_dir(test_root.path()));
    let router_root = test_root.path().join("router");
    let codex_bin = test_root.path().join("fake-codex");
    let invocation_log = test_root.path().join("codex-invocation.log");
    let codex_home_mode_log = test_root.path().join("codex-home-mode.log");
    must_ok(fs::write(
        &codex_bin,
        format!(
            r#"#!/bin/sh
set -eu
printf '%s\n' "$*" > "{}"
if stat -f %Lp "$CODEX_HOME" > /dev/null 2>&1; then
  stat -f %Lp "$CODEX_HOME" > "{}"
else
  stat -c %a "$CODEX_HOME" > "{}"
fi
test "$1" = "login"
test "$2" = "--device-auth"
test -n "${{CODEX_HOME:-}}"
cat > "$CODEX_HOME/auth.json" <<'JSON'
{{"auth_mode":"chatgpt","tokens":{{"access_token":"device-access-canary","refresh_token":"device-refresh-canary"}}}}
JSON
"#,
            invocation_log.display(),
            codex_home_mode_log.display(),
            codex_home_mode_log.display()
        ),
    ));
    let mut permissions = must_ok(fs::metadata(&codex_bin)).permissions();
    permissions.set_mode(0o700);
    must_ok(fs::set_permissions(&codex_bin, permissions));

    let output = run_cli(
        [
            "codex-router",
            "account",
            "login",
            "--router-root",
            path_to_str(&router_root),
            "--label",
            "device primary",
            "--device-auth",
            "--codex-bin",
            path_to_str(&codex_bin),
            "--allow-plaintext-file-secrets",
        ],
        CliContext::new(Vec::new()),
    );

    let account_id = account_id("acct_device_primary");
    let state = must_ok(SqliteStateStore::open(&router_root.join("state.sqlite")));
    let account = must_ok(AccountStateRepository::load_account(&state, &account_id))
        .unwrap_or_else(|| panic!("device-auth account metadata should exist"));
    assert_eq!(account.label(), "device primary");
    assert_eq!(account.status(), AccountStatus::Enabled);
    assert_eq!(account.active_credential_generation(), Some(1));

    let secrets = must_ok(FileSecretStore::open(router_root.join("secrets")));
    let bundle_key = must_ok(account_credential_bundle_key(&account_id, 1));
    let bundle = must_ok(AccountCredentialBundle::from_secret_string(must_ok(
        secrets.read_secret(&bundle_key),
    )));
    assert_eq!(
        bundle.access_token().expose_secret(),
        "device-access-canary"
    );
    assert_eq!(
        bundle.refresh_token().map(SecretString::expose_secret),
        Some("device-refresh-canary")
    );
    assert_eq!(
        must_ok(fs::read_to_string(invocation_log)),
        "login --device-auth\n"
    );
    assert_eq!(must_ok(fs::read_to_string(codex_home_mode_log)), "700\n");
    assert!(
        output
            .stdout
            .contains("logged in account: device primary\n")
    );
    assert!(output.stdout.contains("account_id: acct_device_primary\n"));
    assert!(!output.stdout.contains("device-access-canary"));
    assert!(!output.stdout.contains("device-refresh-canary"));
    assert!(output.stderr.is_empty());
}

#[test]
fn account_login_device_auth_cleans_temporary_codex_home_on_failure() {
    let test_root = TestRoot::new("account-login-device-auth-failure");
    must_ok(fs::create_dir(test_root.path()));
    let router_root = test_root.path().join("router");
    let codex_bin = test_root.path().join("failing-codex");
    let codex_home_log = test_root.path().join("codex-home.log");
    must_ok(fs::write(
        &codex_bin,
        format!(
            r#"#!/bin/sh
set -eu
printf '%s\n' "$CODEX_HOME" > "{}"
exit 42
"#,
            codex_home_log.display()
        ),
    ));
    let mut permissions = must_ok(fs::metadata(&codex_bin)).permissions();
    permissions.set_mode(0o700);
    must_ok(fs::set_permissions(&codex_bin, permissions));

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let error = match run_with_io(
        vec![
            "codex-router".into(),
            "account".into(),
            "login".into(),
            "--router-root".into(),
            router_root.as_os_str().to_owned(),
            "--label".into(),
            "device primary".into(),
            "--device-auth".into(),
            "--codex-bin".into(),
            codex_bin.as_os_str().to_owned(),
            "--allow-plaintext-file-secrets".into(),
        ],
        &CliContext::new(Vec::new()),
        &mut stdout,
        &mut stderr,
    ) {
        Ok(()) => panic!("device-auth failure should surface"),
        Err(error) => error,
    };

    assert!(error.to_string().contains("codex device-auth login failed"));
    let temporary_codex_home = PathBuf::from(must_ok(fs::read_to_string(codex_home_log)).trim());
    assert!(!temporary_codex_home.exists());
    assert!(stdout.is_empty());
    assert!(stderr.is_empty());
}

#[test]
fn account_import_codex_auth_writes_router_owned_state_and_secrets() {
    let test_root = TestRoot::new("account-import");
    must_ok(fs::create_dir(test_root.path()));
    let router_root = test_root.path().join("router");
    let auth_json = test_root.path().join("auth.json");
    let id_token = fake_id_token_with_chatgpt_account_id("chatgpt-account-id-canary");
    let auth_json_text = format!(
        r#"{{"auth_mode":"chatgpt","tokens":{{"access_token":"access-token-canary","refresh_token":"refresh-token-canary","id_token":"{id_token}"}}}}"#
    );
    must_ok(fs::write(&auth_json, &auth_json_text));

    let output = run_cli(
        [
            "codex-router",
            "account",
            "import-codex-auth",
            "--router-root",
            path_to_str(&router_root),
            "--label",
            "primary",
            "--auth-json",
            path_to_str(&auth_json),
            "--allow-plaintext-file-secrets",
        ],
        CliContext::new(Vec::new()),
    );

    let account_id = account_id("acct_primary");
    let state = must_ok(SqliteStateStore::open(&router_root.join("state.sqlite")));
    let account = must_ok(AccountStateRepository::load_account(&state, &account_id))
        .unwrap_or_else(|| panic!("imported account metadata should exist"));
    assert_eq!(account.label(), "primary");
    assert_eq!(account.status(), AccountStatus::Enabled);
    assert_eq!(account.active_credential_generation(), Some(1));

    let secrets = must_ok(FileSecretStore::open(router_root.join("secrets")));
    let bundle_key = must_ok(account_credential_bundle_key(&account_id, 1));
    let bundle = must_ok(AccountCredentialBundle::from_secret_string(must_ok(
        secrets.read_secret(&bundle_key),
    )));
    assert_eq!(bundle.access_token().expose_secret(), "access-token-canary");
    assert_eq!(
        bundle.refresh_token().map(SecretString::expose_secret),
        Some("refresh-token-canary")
    );
    assert_eq!(
        bundle.chatgpt_account_id(),
        Some("chatgpt-account-id-canary")
    );
    assert!(output.stdout.contains("imported account: primary\n"));
    assert!(output.stdout.contains("account_id: acct_primary\n"));
    assert!(!output.stdout.contains("access-token-canary"));
    assert!(!output.stdout.contains("refresh-token-canary"));
    assert!(output.stderr.is_empty());
    assert_eq!(must_ok(fs::read_to_string(&auth_json)), auth_json_text);
}

#[test]
fn account_import_codex_auth_redacts_refresh_token_in_error_paths() {
    let test_root = TestRoot::new("account-import-redaction");
    must_ok(fs::create_dir(test_root.path()));
    let router_root = test_root.path().join("router");
    let auth_json = test_root.path().join("auth.json");
    must_ok(fs::write(
        &auth_json,
        r#"{"auth_mode":"chatgpt","tokens":{"refresh_token":"refresh-token-canary","access_token":""}}"#,
    ));

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let error = match run_with_io(
        vec![
            "codex-router".into(),
            "account".into(),
            "import-codex-auth".into(),
            "--router-root".into(),
            router_root.as_os_str().to_owned(),
            "--label".into(),
            "primary".into(),
            "--auth-json".into(),
            auth_json.as_os_str().to_owned(),
            "--allow-plaintext-file-secrets".into(),
        ],
        &CliContext::new(Vec::new()),
        &mut stdout,
        &mut stderr,
    ) {
        Ok(()) => panic!("missing access token must fail"),
        Err(error) => error,
    };
    let rendered_error = error.to_string();

    assert_eq!(rendered_error, "access token not found in auth json");
    assert!(!rendered_error.contains("refresh-token-canary"));
    assert!(stdout.is_empty());
    assert!(stderr.is_empty());
}

#[test]
fn account_import_codex_auth_partial_secret_write_disables_account_until_repair() {
    let test_root = TestRoot::new("account-import-partial-secret");
    must_ok(fs::create_dir(test_root.path()));
    let router_root = test_root.path().join("router");
    must_ok(fs::create_dir_all(&router_root));
    let state = must_ok(SqliteStateStore::open(&router_root.join("state.sqlite")));
    let failing_secrets = FailingSecretStore::new();
    let request =
        AccountImportRequest::new(account_id("acct_primary"), "primary", "access-token-canary")
            .with_refresh_token("refresh-token-canary");

    let error = must_err(import_codex_auth_from_request(
        &state,
        &failing_secrets,
        request,
    ));

    assert!(error.to_string().contains("secret store"));
    let account = must_ok(AccountStateRepository::load_account(
        &state,
        &account_id("acct_primary"),
    ))
    .unwrap_or_else(|| panic!("failed import should leave disabled account metadata"));
    assert_eq!(account.status(), AccountStatus::Disabled);
    assert_eq!(account.active_credential_generation(), None);
    assert_eq!(failing_secrets.write_attempts(), 1);
}

#[test]
fn account_import_codex_auth_invalidates_quota_snapshot_on_credential_mutation() {
    let test_root = TestRoot::new("account-import-invalidates-quota");
    must_ok(fs::create_dir(test_root.path()));
    let router_root = test_root.path().join("router");
    let auth_json = test_root.path().join("auth.json");
    must_ok(fs::create_dir_all(&router_root));
    must_ok(fs::write(
        &auth_json,
        r#"{"auth_mode":"chatgpt","tokens":{"access_token":"new-access-token","refresh_token":"new-refresh-token"}}"#,
    ));
    let account_id = account_id("acct_primary");
    let state = must_ok(SqliteStateStore::open(&router_root.join("state.sqlite")));
    must_ok(AccountStateRepository::upsert_account(
        &state,
        &AccountRecord::new(account_id.clone(), "primary", AccountStatus::Enabled)
            .with_active_credential_generation(1),
    ));
    for route_band in [
        "responses",
        "models",
        "memories_trace_summarize",
        "responses_compact",
        "code_review",
    ] {
        must_ok(QuotaSnapshotRepository::upsert_snapshot(
            &state,
            &PersistedQuotaSnapshot::new(account_id.clone(), QuotaSnapshotSource::MockEndpoint)
                .with_observed_unix_seconds(9_000)
                .with_route_band(route_band, 99)
                .with_reset_unix_seconds(10_000)
                .with_stale_penalty(false),
        ));
    }

    let output = run_cli(
        [
            "codex-router",
            "account",
            "import-codex-auth",
            "--router-root",
            path_to_str(&router_root),
            "--label",
            "primary",
            "--auth-json",
            path_to_str(&auth_json),
            "--allow-plaintext-file-secrets",
        ],
        CliContext::new(Vec::new()),
    );

    let account = must_ok(AccountStateRepository::load_account(&state, &account_id))
        .unwrap_or_else(|| panic!("account should remain registered"));
    assert_eq!(account.status(), AccountStatus::Enabled);
    assert_eq!(account.active_credential_generation(), Some(2));
    let secrets = must_ok(FileSecretStore::open(router_root.join("secrets")));
    let bundle_key = must_ok(account_credential_bundle_key(&account_id, 2));
    let bundle = must_ok(AccountCredentialBundle::from_secret_string(must_ok(
        secrets.read_secret(&bundle_key),
    )));
    assert_eq!(bundle.access_token().expose_secret(), "new-access-token");
    assert_eq!(
        bundle.refresh_token().map(SecretString::expose_secret),
        Some("new-refresh-token")
    );
    for route_band in [
        "responses",
        "models",
        "memories_trace_summarize",
        "responses_compact",
        "code_review",
    ] {
        let snapshot = must_ok(QuotaSnapshotRepository::load_snapshot_for_route_band(
            &state,
            &account_id,
            route_band,
        ))
        .unwrap_or_else(|| panic!("{route_band} snapshot should remain as stale marker"));
        assert_eq!(snapshot.remaining_headroom(), 0);
        assert_eq!(snapshot.observed_unix_seconds(), 0);
        assert!(snapshot.stale_penalty());
    }
    assert!(output.stdout.contains("imported account: primary\n"));
    assert!(!output.stdout.contains("new-access-token"));
    assert!(!output.stdout.contains("new-refresh-token"));
    assert!(output.stderr.is_empty());
}

#[test]
fn account_login_defaults_to_device_auth_method() {
    let command = match CliCommand::parse([
        OsString::from("account"),
        OsString::from("login"),
        OsString::from("--label"),
        OsString::from("primary"),
    ]) {
        Ok(CliCommand::Account(command)) => command,
        Ok(other) => panic!("account command should parse, got {other:?}"),
        Err(error) => panic!("account command should parse: {error}"),
    };

    let AccountCommand::LoginDeviceAuth {
        router_root,
        label,
        codex_bin,
        allow_plaintext_file_secrets,
    } = command
    else {
        panic!("account login should default to device auth");
    };
    assert_eq!(router_root, default_router_root_for_test());
    assert_eq!(label, "primary");
    assert_eq!(codex_bin, PathBuf::from("codex"));
    assert!(!allow_plaintext_file_secrets);
}

use super::*;

#[test]
fn explicit_auth_file_account_entrypoints_are_unknown() {
    let login_error = must_err(CliCommand::parse([
        OsString::from("account"),
        OsString::from("login"),
        OsString::from("--label"),
        OsString::from("primary"),
        OsString::from("--auth-json"),
        OsString::from("/tmp/unused-auth.json"),
    ]));
    assert!(matches!(login_error, CliError::UnknownOption { option } if option == "--auth-json"));
    let import_error = must_err(CliCommand::parse([
        OsString::from("account"),
        OsString::from("import-codex-auth"),
    ]));
    assert!(
        matches!(import_error, CliError::UnknownCommand { command } if command == "account import-codex-auth")
    );
    let live_error = must_err(CliCommand::parse([
        OsString::from("live"),
        OsString::from("quota"),
        OsString::from("--auth-json"),
        OsString::from("/tmp/unused-auth.json"),
    ]));
    assert!(matches!(live_error, CliError::UnknownOption { option } if option == "--auth-json"));
}

struct FailingWriteOnlySecretStore {
    write_attempts: AtomicUsize,
}

impl SecretStore for FailingWriteOnlySecretStore {
    fn write_secret(
        &self,
        _key: &SecretKey,
        _secret: &SecretString,
    ) -> Result<(), SecretStoreError> {
        self.write_attempts.fetch_add(1, Ordering::SeqCst);
        Err(SecretStoreError::Filesystem {
            path: PathBuf::from("injected-secret-write"),
            source: std::io::Error::other("injected write failure"),
        })
    }

    fn read_secret(&self, _key: &SecretKey) -> Result<SecretString, SecretStoreError> {
        Err(SecretStoreError::Filesystem {
            path: PathBuf::from("unused-generation"),
            source: std::io::Error::from(std::io::ErrorKind::NotFound),
        })
    }
}

#[test]
fn device_login_commit_preserves_old_secret_and_invalidates_quota() {
    let test_root = TestRoot::new("device-login-existing-credential");
    must_ok(fs::create_dir(test_root.path()));
    let state_path = test_root.path().join("state.sqlite");
    let secret_root = test_root.path().join("secrets");
    let runtime = test_async_runtime();
    let state = must_ok(runtime.block_on(AsyncSqliteStateStore::open(&state_path)));
    let account_id = account_id("device-existing");
    must_ok(
        runtime.block_on(
            state.upsert_account(
                &AccountRecord::new(
                    codex_router_core::provider::Provider::Openai,
                    account_id.clone(),
                    "device",
                    AccountStatus::Enabled,
                )
                .with_active_credential_generation(1),
            ),
        ),
    );
    let secrets = must_ok(FileSecretStore::open(&secret_root));
    let old_key = must_ok(account_credential_bundle_key(&account_id, 1));
    let old_secret = must_ok(
        AccountCredentialBundle::imported_codex_auth(
            "old-access-canary",
            Some("old-refresh-canary".to_owned()),
        )
        .to_secret_string(),
    );
    must_ok(secrets.write_secret(&old_key, &old_secret));
    must_ok(
        runtime.block_on(
            state.upsert_quota_snapshot(
                &PersistedQuotaSnapshot::new(account_id.clone(), QuotaSnapshotSource::MockEndpoint)
                    .with_observed_unix_seconds(9_000)
                    .with_route_band("responses", 99),
            ),
        ),
    );
    must_ok(
        runtime.block_on(import_codex_auth_from_request_async(
            &state,
            &secrets,
            AccountImportRequest::new(account_id.clone(), "device", "new-access-canary")
                .with_refresh_token("new-refresh-canary"),
        )),
    );
    let account =
        must_ok(runtime.block_on(state.load_account(&account_id))).expect("account should remain");
    assert_eq!(account.active_credential_generation(), Some(2));
    assert_eq!(
        must_ok(secrets.read_secret(&old_key)).expose_secret(),
        old_secret.expose_secret()
    );
    let new_key = must_ok(account_credential_bundle_key(&account_id, 2));
    let new_bundle = must_ok(AccountCredentialBundle::from_secret_string(must_ok(
        secrets.read_secret(&new_key),
    )));
    assert_eq!(
        new_bundle.access_token().expose_secret(),
        "new-access-canary"
    );
    let snapshot = must_ok(
        runtime.block_on(state.load_quota_snapshot_for_route_band(&account_id, "responses")),
    )
    .expect("stale quota marker should remain");
    assert!(snapshot.stale_penalty());
    assert_eq!(snapshot.remaining_headroom(), 0);
}

#[derive(Clone)]
struct HeldLoginRaceRefreshClient {
    entered_sender: mpsc::Sender<()>,
    release_receiver: Arc<Mutex<mpsc::Receiver<()>>>,
}

impl CredentialRefreshClient for HeldLoginRaceRefreshClient {
    fn refresh_credentials(
        &self,
        _account_id: &AccountId,
        refresh_token: &SecretString,
    ) -> Result<AccountCredentialBundle, codex_router_auth::resolver::CredentialRefreshFailure>
    {
        assert_eq!(refresh_token.expose_secret(), "old-refresh-canary");
        self.entered_sender
            .send(())
            .expect("provider should report entry");
        self.release_receiver
            .lock()
            .expect("release lock")
            .recv_timeout(Duration::from_secs(2))
            .expect("provider should be released");
        Ok(AccountCredentialBundle::imported_codex_auth(
            "rotated-access-canary",
            Some("rotated-refresh-canary".to_owned()),
        )
        .with_expires_unix_seconds(4_000_000_000))
    }
}

#[test]
fn device_relogin_waits_for_claimed_refresh_then_wins_the_active_generation() {
    let test_root = TestRoot::new("device-relogin-refresh-race");
    must_ok(fs::create_dir(test_root.path()));
    let state_path = test_root.path().join("state.sqlite");
    let secret_root = test_root.path().join("secrets");
    let state = must_ok(SqliteStateStore::open(&state_path));
    let account_id = account_id("device-refresh-race");
    must_ok(AccountStateRepository::upsert_account(
        &state,
        &AccountRecord::new(
            codex_router_core::provider::Provider::Openai,
            account_id.clone(),
            "race",
            AccountStatus::Enabled,
        )
        .with_active_credential_generation(1),
    ));
    let secrets = must_ok(FileSecretStore::open(&secret_root));
    let active_key = must_ok(account_credential_bundle_key(&account_id, 1));
    must_ok(
        secrets.write_secret(
            &active_key,
            &must_ok(
                AccountCredentialBundle::imported_codex_auth(
                    "expired-access-canary",
                    Some("old-refresh-canary".to_owned()),
                )
                .with_expires_unix_seconds(900)
                .to_secret_string(),
            ),
        ),
    );
    drop(state);
    let (entered_sender, entered_receiver) = mpsc::channel();
    let (release_sender, release_receiver) = mpsc::channel();
    let refresh_client = HeldLoginRaceRefreshClient {
        entered_sender,
        release_receiver: Arc::new(Mutex::new(release_receiver)),
    };
    let refresh_state_path = state_path.clone();
    let refresh_secret_root = secret_root.clone();
    let refresh_account = account_id.clone();
    let refresh_thread = thread::spawn(move || {
        let resolver = must_ok(CliCredentialResolver::open_with_refresh_client(
            &refresh_state_path,
            &refresh_secret_root,
            refresh_client,
        ));
        must_ok(resolver.resolve_provider_credentials(&refresh_account)).credential_generation()
    });
    must_ok(entered_receiver.recv_timeout(Duration::from_secs(2)));
    let login_state_path = state_path.clone();
    let login_secret_root = secret_root;
    let login_account = account_id.clone();
    let (login_done_sender, login_done_receiver) = mpsc::channel();
    let login_thread = thread::spawn(move || {
        let runtime = test_async_runtime();
        let state = must_ok(runtime.block_on(AsyncSqliteStateStore::open(&login_state_path)));
        let secrets = must_ok(FileSecretStore::open(&login_secret_root));
        must_ok(
            runtime.block_on(import_codex_auth_from_request_async(
                &state,
                &secrets,
                AccountImportRequest::new(login_account, "race", "login-access-canary")
                    .with_refresh_token("login-refresh-canary"),
            )),
        );
        must_ok(login_done_sender.send(()));
    });
    assert!(matches!(
        login_done_receiver.recv_timeout(Duration::from_millis(100)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    must_ok(release_sender.send(()));
    assert_eq!(
        must_ok(refresh_thread.join().map_err(|_| "refresh thread failed")),
        2
    );
    must_ok(login_done_receiver.recv_timeout(Duration::from_secs(2)));
    must_ok(login_thread.join().map_err(|_| "login thread failed"));
    let state = must_ok(SqliteStateStore::open(&state_path));
    let active = must_ok(AccountStateRepository::load_account(&state, &account_id))
        .expect("account should remain");
    assert_eq!(active.active_credential_generation(), Some(3));
    let login_key = must_ok(account_credential_bundle_key(&account_id, 3));
    let login_bundle = must_ok(AccountCredentialBundle::from_secret_string(must_ok(
        secrets.read_secret(&login_key),
    )));
    assert_eq!(
        login_bundle.access_token().expose_secret(),
        "login-access-canary"
    );
}

#[test]
fn failed_device_login_secret_commit_leaves_new_account_disabled() {
    let test_root = TestRoot::new("device-login-write-failure");
    must_ok(fs::create_dir(test_root.path()));
    let runtime = test_async_runtime();
    let state = must_ok(runtime.block_on(AsyncSqliteStateStore::open(
        &test_root.path().join("state.sqlite"),
    )));
    let account_id = account_id("device-write-failure");
    let secrets = FailingWriteOnlySecretStore {
        write_attempts: AtomicUsize::new(0),
    };
    let error = must_err(
        runtime.block_on(import_codex_auth_from_request_async(
            &state,
            &secrets,
            AccountImportRequest::new(account_id.clone(), "device", "new-access-canary")
                .with_refresh_token("new-refresh-canary"),
        )),
    );
    assert!(error.to_string().contains("secret store"));
    let account = must_ok(runtime.block_on(state.load_account(&account_id)))
        .expect("new account metadata should remain");
    assert_eq!(account.status(), AccountStatus::Disabled);
    assert_eq!(account.active_credential_generation(), None);
    assert_eq!(secrets.write_attempts.load(Ordering::SeqCst), 1);
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
{{"auth_mode":"chatgpt","tokens":{{"access_token":"device-access-canary","refresh_token":"device-refresh-canary","id_token":"header.eyJodHRwczovL2FwaS5vcGVuYWkuY29tL2F1dGgiOnsiY2hhdGdwdF9hY2NvdW50X2lkIjoiY2hhdGdwdC1maXh0dXJlLWlkIn19.signature"}}}}
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
    assert_eq!(bundle.chatgpt_account_id(), Some("chatgpt-fixture-id"));
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
fn account_login_reauthenticates_the_same_openai_account() {
    let test_root = TestRoot::new("account-login-reuses-openai-account");
    must_ok(fs::create_dir(test_root.path()));
    let router_root = test_root.path().join("router");
    must_ok(fs::create_dir(&router_root));
    let state_path = router_root.join("state.sqlite");
    let secret_root = router_root.join("secrets");
    let account_id = account_id("acct_device_primary");
    let runtime = test_async_runtime();
    let state = must_ok(runtime.block_on(AsyncSqliteStateStore::open(&state_path)));
    must_ok(
        runtime.block_on(
            state.upsert_account(
                &AccountRecord::new(
                    codex_router_core::provider::Provider::Openai,
                    account_id.clone(),
                    "device primary",
                    AccountStatus::Disabled,
                )
                .with_active_credential_generation(1),
            ),
        ),
    );
    let secrets = must_ok(FileSecretStore::open(&secret_root));
    let active_key = must_ok(account_credential_bundle_key(&account_id, 1));
    let active_bundle = must_ok(
        AccountCredentialBundle::imported_codex_auth(
            "expired-access-canary",
            Some("expired-refresh-canary".to_owned()),
        )
        .to_secret_string(),
    );
    must_ok(secrets.write_secret(&active_key, &active_bundle));
    assert!(must_ok(
        runtime.block_on(state.mark_credential_unrefreshable(&account_id, 1))
    ));
    must_ok(runtime.block_on(state.close()));

    let codex_bin = test_root.path().join("fake-codex");
    must_ok(fs::write(
        &codex_bin,
        r#"#!/bin/sh
set -eu
cat > "$CODEX_HOME/auth.json" <<'JSON'
{"auth_mode":"chatgpt","tokens":{"access_token":"relogin-access-canary","refresh_token":"relogin-refresh-canary"}}
JSON
"#,
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

    let reopened = must_ok(runtime.block_on(AsyncSqliteStateStore::open(&state_path)));
    let account = must_ok(runtime.block_on(reopened.load_account(&account_id)))
        .unwrap_or_else(|| panic!("re-logged-in account should remain"));
    assert_eq!(account.label(), "device primary");
    assert_eq!(account.status(), AccountStatus::Enabled);
    assert_eq!(account.active_credential_generation(), Some(2));
    assert!(must_ok(runtime.block_on(reopened.load_credential_maintenance(&account_id))).is_none());
    must_ok(runtime.block_on(reopened.close()));

    let secrets = must_ok(FileSecretStore::open(&secret_root));
    let new_key = must_ok(account_credential_bundle_key(&account_id, 2));
    let new_bundle = must_ok(AccountCredentialBundle::from_secret_string(must_ok(
        secrets.read_secret(&new_key),
    )));
    assert_eq!(
        new_bundle.access_token().expose_secret(),
        "relogin-access-canary"
    );
    assert_eq!(
        new_bundle.refresh_token().map(SecretString::expose_secret),
        Some("relogin-refresh-canary")
    );
    assert!(
        output
            .stdout
            .contains("logged in account: device primary\n")
    );
    assert!(!output.stdout.contains("relogin-access-canary"));
    assert!(output.stderr.is_empty());
}

#[test]
fn device_login_missing_access_token_error_redacts_other_auth_fields() {
    let test_root = TestRoot::new("device-login-missing-access");
    must_ok(fs::create_dir(test_root.path()));
    let codex_bin = test_root.path().join("fake-codex");
    must_ok(fs::write(
        &codex_bin,
        r#"#!/bin/sh
set -eu
cat > "$CODEX_HOME/auth.json" <<'JSON'
{"auth_mode":"chatgpt","tokens":{"refresh_token":"refresh-secret-canary","id_token":"id-secret-canary"}}
JSON
"#,
    ));
    let mut permissions = must_ok(fs::metadata(&codex_bin)).permissions();
    permissions.set_mode(0o700);
    must_ok(fs::set_permissions(&codex_bin, permissions));
    let router_root = test_root.path().join("router");
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let error = must_err(run_with_io(
        vec![
            "codex-router".into(),
            "account".into(),
            "login".into(),
            "--router-root".into(),
            router_root.as_os_str().to_owned(),
            "--label".into(),
            "missing access".into(),
            "--device-auth".into(),
            "--codex-bin".into(),
            codex_bin.as_os_str().to_owned(),
            "--allow-plaintext-file-secrets".into(),
        ],
        &CliContext::new(Vec::new()),
        &mut stdout,
        &mut stderr,
    ));
    assert!(error.to_string().contains("access token not found"));
    assert!(!format!("{error:?}").contains("secret-canary"));
    assert!(stdout.is_empty());
    assert!(stderr.is_empty());
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

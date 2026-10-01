use super::*;

use codex_router_auth::openai_oauth::OpenAiOAuthDeviceLoginClient;
use codex_router_core::provider::Provider;
use codex_router_secret_store::test_support::FileReadTraceEvent;
use codex_router_secret_store::test_support::FileWriteTraceEvent;
use std::io::Cursor;
use std::net::SocketAddr;
use std::thread::JoinHandle;

#[test]
fn account_login_defaults_to_openai_device_flow() {
    let command = CliCommand::parse([
        OsString::from("account"),
        OsString::from("login"),
        OsString::from("--label"),
        OsString::from("primary"),
    ])
    .expect("account login should parse");

    let CliCommand::Account(AccountCommand::Login {
        provider_login_flow,
        label,
        ..
    }) = command
    else {
        panic!("account login should produce a login command");
    };
    assert_eq!(label, "primary");
    assert_eq!(
        provider_login_flow,
        crate::account::ProviderLoginFlow::OpenAiDevice
    );
}

#[test]
fn account_login_selects_claude_oauth_flow() {
    let command = CliCommand::parse([
        OsString::from("account"),
        OsString::from("login"),
        OsString::from("--provider"),
        OsString::from("claude"),
        OsString::from("--label"),
        OsString::from("claude-primary"),
    ])
    .expect("Claude account login should parse");

    let CliCommand::Account(AccountCommand::Login {
        provider_login_flow,
        ..
    }) = command
    else {
        panic!("account login should produce a login command");
    };
    assert_eq!(
        provider_login_flow,
        crate::account::ProviderLoginFlow::ClaudeOAuth
    );
}

#[test]
fn account_login_rejects_legacy_codex_child_and_plaintext_file_options() {
    for provider in ["openai", "claude"] {
        for legacy_options in [
            vec!["--device-auth"],
            vec!["--codex-bin", "codex"],
            vec!["--allow-plaintext-file-secrets"],
        ] {
            let mut arguments = vec![
                OsString::from("account"),
                OsString::from("login"),
                OsString::from("--provider"),
                OsString::from(provider),
                OsString::from("--label"),
                OsString::from("primary"),
            ];
            arguments.extend(legacy_options.into_iter().map(OsString::from));

            let error = CliCommand::parse(arguments)
                .expect_err("login must not accept Codex child or plaintext-file options");
            assert!(matches!(error, CliError::UnknownOption { .. }));
        }
    }
}

#[test]
pub(super) fn openai_device_login_activates_fake_issuer_tokens_without_plaintext_files() {
    let test_root = TestRoot::new("openai-device-login-activation");
    must_ok(fs::create_dir(test_root.path()));
    let router_root = test_root.path().join("router");
    let secret_root = router_root.join("secrets");
    let (traced_secret_store, read_trace, write_trace) = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store_with_read_write_traces(
            &secret_root,
        ),
    );
    let issuer = FakeDeviceIssuer::spawn("device-access-canary", "device-refresh-canary");
    let client = OpenAiOAuthDeviceLoginClient::with_test_issuer(
        issuer.base_url.clone(),
        std::time::Duration::from_secs(60),
    );
    let command = parse_openai_login(&router_root, "device primary");
    let mut stdout = Vec::new();
    let mut reader = Cursor::new(Vec::<u8>::new());

    must_ok(
        crate::account::run_account_command_with_input_and_openai_client_and_secret_store(
            &mut stdout,
            &mut reader,
            command,
            &client,
            traced_secret_store,
        ),
    );

    let requests = issuer.finish();
    let read_events = read_trace.events();
    let home_root = PathBuf::from(
        std::env::var_os("HOME").expect("HOME should be available for native-path proof"),
    );
    let native_auth_store_roots = [home_root.join(".codex"), home_root.join(".claude")];
    assert!(
        read_events.iter().any(|event| matches!(
            event,
            FileReadTraceEvent::FileOpenAttempted { path }
                if path.file_name().is_some_and(|name| name.to_string_lossy().ends_with(".v2"))
        )),
        "login must trace the encrypted staged-credential verification read"
    );
    for event in read_events {
        let path = match event {
            FileReadTraceEvent::FileOpenAttempted { path }
            | FileReadTraceEvent::DirectoryOpenAttempted { path } => path,
        };
        assert!(
            path.starts_with(&secret_root),
            "login opened a path outside the Router secret store: {path:?}"
        );
        assert!(
            native_auth_store_roots
                .iter()
                .all(|native_root| !path.starts_with(native_root)),
            "login opened a path under a native auth store: {path:?}"
        );
    }

    assert_eq!(requests.len(), 3);
    assert!(requests[0].starts_with("POST /api/accounts/deviceauth/usercode HTTP/1.1"));
    assert!(requests[0].contains("\"client_id\":\"app_EMoamEEZ73f0CkXaXp7hrann\""));
    assert!(requests[1].starts_with("POST /api/accounts/deviceauth/token HTTP/1.1"));
    assert!(requests[1].contains("device-auth-id"));
    assert!(requests[1].contains("ABCD-EFGH"));
    assert!(requests[2].starts_with("POST /oauth/token HTTP/1.1"));
    assert!(requests[2].contains("grant_type=authorization_code"));
    assert!(requests[2].contains("code_verifier=verifier"));

    let output = String::from_utf8(stdout).expect("login output should be UTF-8");
    assert!(output.contains("http://127.0.0.1:"));
    assert!(output.contains("/codex/device"));
    assert!(output.contains("Code: ABCD-EFGH"));
    assert!(output.contains("logged in account: device primary"));
    assert!(output.contains("account_id: acct_device_primary"));
    assert!(!output.contains("device-access-canary"));
    assert!(!output.contains("device-refresh-canary"));
    assert_eq!(reader.position(), 0);

    let write_events = write_trace.events();
    let mut temporary_writes = 0;
    let mut activated_bundle_renamed = false;
    for event in &write_events {
        match event {
            FileWriteTraceEvent::TemporaryFileWritten { path, contents } => {
                temporary_writes += 1;
                assert!(path.starts_with(&secret_root));
                assert!(!contains_bytes(contents, b"device-access-canary"));
                assert!(!contains_bytes(contents, b"device-refresh-canary"));
            }
            FileWriteTraceEvent::FileRenamed { from, to } => {
                assert!(from.starts_with(&secret_root));
                assert!(to.starts_with(&secret_root));
                activated_bundle_renamed |= to.file_name().is_some_and(|name| {
                    name.to_string_lossy() == "openai_credential_bundle.acct_device_primary.1.v2"
                });
            }
            FileWriteTraceEvent::FileRemoved { path } => {
                assert!(path.starts_with(&secret_root));
                assert!(!path.ends_with("auth.json"));
            }
        }
    }
    assert!(
        temporary_writes > 0,
        "login must trace encrypted temp writes"
    );
    assert!(
        activated_bundle_renamed,
        "login must atomically publish the encrypted credential generation"
    );

    let state = must_ok(SqliteStateStore::open(&router_root.join("state.sqlite")));
    let account = must_ok(AccountStateRepository::load_account(
        &state,
        &account_id("acct_device_primary"),
    ))
    .expect("device login should activate an account");
    assert_eq!(account.provider(), Provider::Openai);
    assert_eq!(account.status(), AccountStatus::Enabled);
    assert_eq!(account.active_credential_generation(), Some(1));

    let secrets = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store(
            router_root.join("secrets"),
        ),
    );
    let bundle_key = must_ok(openai_account_credential_bundle_key(
        account.account_id(),
        1,
    ));
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

    let encrypted_path = secret_root.join("openai_credential_bundle.acct_device_primary.1.v2");
    let encrypted_bytes = must_ok(fs::read(encrypted_path));
    assert!(!contains_bytes(&encrypted_bytes, b"device-access-canary"));
    assert!(!contains_bytes(&encrypted_bytes, b"device-refresh-canary"));
    assert!(!router_root.join("auth.json").exists());
}

#[test]
fn openai_device_login_reauthenticates_the_same_account() {
    let test_root = TestRoot::new("openai-device-login-same-account");
    must_ok(fs::create_dir(test_root.path()));
    let router_root = test_root.path().join("router");
    must_ok(fs::create_dir_all(&router_root));
    let state_path = router_root.join("state.sqlite");
    let secret_root = router_root.join("secrets");
    let account = AccountRecord::new(
        Provider::Openai,
        account_id("acct_device_primary"),
        "device primary",
        AccountStatus::Disabled,
    )
    .with_active_credential_generation(1);
    let runtime = test_async_runtime();
    let state = must_ok(runtime.block_on(AsyncSqliteStateStore::open(&state_path)));
    must_ok(runtime.block_on(state.upsert_account(&account)));
    let secrets = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store(&secret_root),
    );
    let active_key = must_ok(openai_account_credential_bundle_key(
        account.account_id(),
        1,
    ));
    let active_bundle = must_ok(
        AccountCredentialBundle::imported_codex_auth(
            "previous-access-canary",
            Some("previous-refresh-canary".to_owned()),
        )
        .to_secret_string(),
    );
    must_ok(secrets.write_secret(&active_key, &active_bundle));
    must_ok(runtime.block_on(state.close()));

    let issuer = FakeDeviceIssuer::spawn("replacement-access-canary", "replacement-refresh-canary");
    let client = OpenAiOAuthDeviceLoginClient::with_test_issuer(
        issuer.base_url.clone(),
        std::time::Duration::from_secs(60),
    );
    let command = parse_openai_login(&router_root, "device primary");
    let mut stdout = Vec::new();
    must_ok(
        crate::account::run_account_command_with_input_and_openai_client(
            &mut stdout,
            &mut Cursor::new(Vec::<u8>::new()),
            command,
            &client,
        ),
    );
    issuer.finish();

    let reopened = must_ok(runtime.block_on(AsyncSqliteStateStore::open(&state_path)));
    let account =
        must_ok(runtime.block_on(reopened.load_account(&account_id("acct_device_primary"))))
            .expect("same-account login should keep the account");
    assert_eq!(account.status(), AccountStatus::Enabled);
    assert_eq!(account.active_credential_generation(), Some(2));
    let secrets = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store(&secret_root),
    );
    let active_key = must_ok(openai_account_credential_bundle_key(
        account.account_id(),
        2,
    ));
    let bundle = must_ok(AccountCredentialBundle::from_secret_string(must_ok(
        secrets.read_secret(&active_key),
    )));
    assert_eq!(
        bundle.access_token().expose_secret(),
        "replacement-access-canary"
    );
    assert_eq!(
        bundle.refresh_token().map(SecretString::expose_secret),
        Some("replacement-refresh-canary")
    );
}

#[cfg(unix)]
#[test]
fn openai_device_login_does_not_spawn_a_codex_child() {
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    let test_root = TestRoot::new("openai-device-login-no-codex-child");
    must_ok(fs::create_dir_all(test_root.path().join("bin")));
    let bin_directory = test_root.path().join("bin");
    let child_marker = test_root.path().join("codex-child-invoked");
    let codex_stub = bin_directory.join("codex");
    must_ok(fs::write(
        &codex_stub,
        "#!/bin/sh\nprintf invoked > \"$CODEX_CHILD_MARKER\"\n",
    ));
    must_ok(fs::set_permissions(
        &codex_stub,
        fs::Permissions::from_mode(0o700),
    ));

    let mut path_entries = vec![bin_directory];
    path_entries.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let isolated_path = must_ok(std::env::join_paths(path_entries));
    let test_binary = must_ok(std::env::current_exe());
    let output = must_ok(
        Command::new(test_binary)
            .arg("--exact")
            .arg("tests::account_login_tests::openai_device_login_activates_fake_issuer_tokens_without_plaintext_files")
            .arg("--nocapture")
            .env("PATH", isolated_path)
            .env("CODEX_CHILD_MARKER", &child_marker)
            .output(),
    );

    assert!(
        output.status.success(),
        "nested OpenAI device login failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !child_marker.exists(),
        "Router-native OpenAI device login must not invoke a Codex child"
    );
}

#[test]
fn openai_login_label_guard_runs_before_device_authorization() {
    let test_root = TestRoot::new("openai-login-label-guard");
    must_ok(fs::create_dir(test_root.path()));
    let router_root = test_root.path().join("router");
    must_ok(fs::create_dir_all(&router_root));
    let state_path = router_root.join("state.sqlite");
    let state = must_ok(test_async_runtime().block_on(AsyncSqliteStateStore::open(&state_path)));
    must_ok(
        test_async_runtime().block_on(state.upsert_account(&AccountRecord::new(
            Provider::Claude,
            account_id("openai-login-label-owner"),
            "shared-label",
            AccountStatus::Enabled,
        ))),
    );
    must_ok(test_async_runtime().block_on(state.close()));

    let issuer = FakeDeviceIssuer::spawn("unused-access-canary", "unused-refresh-canary");
    let client = OpenAiOAuthDeviceLoginClient::with_test_issuer(
        issuer.base_url.clone(),
        std::time::Duration::from_secs(60),
    );
    let mut stdout = Vec::new();
    let error = crate::account::run_account_command_with_input_and_openai_client(
        &mut stdout,
        &mut Cursor::new(Vec::<u8>::new()),
        parse_openai_login(&router_root, "shared-label"),
        &client,
    )
    .expect_err("OpenAI login must reject a label owned by Claude before authorization");

    assert!(matches!(
        error,
        crate::account::AccountCommandError::DuplicateAccountLabel { .. }
    ));
    assert!(stdout.is_empty());
    assert!(
        issuer.finish_without_requests().is_empty(),
        "a duplicate OpenAI label must not contact the issuer"
    );
}

#[test]
fn claude_login_label_guard_runs_before_prompt_or_oauth() {
    let test_root = TestRoot::new("claude-login-label-guard");
    must_ok(fs::create_dir(test_root.path()));
    let router_root = test_root.path().join("router");
    must_ok(fs::create_dir_all(&router_root));
    let state_path = router_root.join("state.sqlite");
    let state = must_ok(test_async_runtime().block_on(AsyncSqliteStateStore::open(&state_path)));
    must_ok(
        test_async_runtime().block_on(state.upsert_account(&AccountRecord::new(
            Provider::Openai,
            account_id("claude-login-label-owner"),
            "shared-label",
            AccountStatus::Enabled,
        ))),
    );
    must_ok(test_async_runtime().block_on(state.close()));

    let command = CliCommand::parse([
        OsString::from("account"),
        OsString::from("login"),
        OsString::from("--provider"),
        OsString::from("claude"),
        OsString::from("--label"),
        OsString::from("shared-label"),
        OsString::from("--router-root"),
        OsString::from(path_to_str(&router_root)),
    ])
    .expect("Claude login command should parse");
    let CliCommand::Account(command) = command else {
        panic!("Claude login should parse as an account command");
    };
    let mut stdout = Vec::new();
    let mut reader = Cursor::new(b"callback-must-not-be-read".to_vec());

    let error = crate::account::run_account_command_with_input(&mut stdout, &mut reader, command)
        .expect_err("globally duplicate labels must be rejected before OAuth starts");

    assert!(matches!(
        error,
        crate::account::AccountCommandError::DuplicateAccountLabel { .. }
    ));
    assert!(stdout.is_empty());
    assert_eq!(reader.position(), 0);
}

fn parse_openai_login(router_root: &Path, label: &str) -> AccountCommand {
    let command = CliCommand::parse([
        OsString::from("account"),
        OsString::from("login"),
        OsString::from("--provider"),
        OsString::from("openai"),
        OsString::from("--label"),
        OsString::from(label),
        OsString::from("--router-root"),
        OsString::from(path_to_str(router_root)),
    ])
    .expect("OpenAI login command should parse");
    let CliCommand::Account(command) = command else {
        panic!("OpenAI login should parse as an account command");
    };
    command
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

#[cfg(target_os = "macos")]
pub(super) fn request_body_json(request: &str) -> serde_json::Value {
    let (_, body) = request
        .split_once("\r\n\r\n")
        .expect("fake issuer request should include an HTTP body");
    serde_json::from_str(body).expect("fake issuer request body should be JSON")
}

fn fake_id_token() -> String {
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;

    let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256","kid":"key-1","typ":"JWT"}"#);
    let payload = URL_SAFE_NO_PAD
        .encode(r#"{"https://api.openai.com/auth":{"chatgpt_account_id":"chatgpt-fixture-id"}}"#);
    format!("{header}.{payload}.signature")
}

struct FakeDeviceIssuer {
    base_url: String,
    address: SocketAddr,
    server: Option<JoinHandle<Vec<String>>>,
}

impl FakeDeviceIssuer {
    fn spawn(access_token: &str, refresh_token: &str) -> Self {
        let listener = must_ok(TcpListener::bind("127.0.0.1:0"));
        let address = must_ok(listener.local_addr());
        let id_token = fake_id_token();
        let responses = [
            r#"{"device_auth_id":"device-auth-id","user_code":"ABCD-EFGH","interval":"0"}"#.to_owned(),
            r#"{"authorization_code":"authorization-code","code_challenge":"challenge","code_verifier":"verifier"}"#.to_owned(),
            format!(
                r#"{{"id_token":"{id_token}","access_token":"{access_token}","refresh_token":"{refresh_token}"}}"#
            ),
        ];
        let server = thread::spawn(move || {
            let mut requests = Vec::new();
            for body in responses {
                let (mut stream, _) = must_ok(listener.accept());
                let request = read_http_request_with_body(&mut stream);
                if request.is_empty() {
                    break;
                }
                requests.push(request);
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                must_ok(stream.write_all(response.as_bytes()));
            }
            requests
        });

        Self {
            base_url: format!("http://{address}"),
            address,
            server: Some(server),
        }
    }

    fn finish(mut self) -> Vec<String> {
        self.server
            .take()
            .expect("fake issuer should still be running")
            .join()
            .expect("fake issuer should finish")
    }

    fn finish_without_requests(mut self) -> Vec<String> {
        must_ok(TcpStream::connect(self.address));
        self.server
            .take()
            .expect("fake issuer should still be running")
            .join()
            .expect("fake issuer should finish")
    }
}

impl Drop for FakeDeviceIssuer {
    fn drop(&mut self) {
        if let Some(server) = self.server.take() {
            let _ = TcpStream::connect(self.address);
            let _ = server.join();
        }
    }
}

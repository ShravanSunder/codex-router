use super::account_login_tests::openai_device_login_activates_fake_issuer_tokens_without_plaintext_files;
use super::account_login_tests::request_body_json;
use super::cli_test_support::TestRoot;
use super::cli_test_support::must_ok;
use codex_router_auth::claude_oauth::AccountLoginFlow;
use codex_router_auth::claude_oauth::ClaudeOAuthLoginFlow;
use codex_router_auth::claude_oauth::ClaudeOAuthRefreshClient;
use codex_router_auth::claude_oauth::LoginFlowError;
use codex_router_auth::claude_oauth::PendingClaudeOAuthLogin;
use codex_router_auth::resolver::AsyncRouterCredentialResolver;
use codex_router_core::ids::AccountId;
use codex_router_core::provider::Provider;
use codex_router_secret_store::SecretStore;
use codex_router_secret_store::account_tokens::provider_credential_bundle_key;
use codex_router_secret_store::credential_bundle::CredentialBundle;
use codex_router_secret_store::test_support::FileReadTraceEvent;
use codex_router_secret_store::test_support::open_encrypted_credential_store_with_read_write_traces;
use codex_router_state::account::AccountStatus;
use codex_router_state::sqlite::AsyncSqliteStateStore;
use std::fs;
use std::io;
use std::io::BufRead;
use std::io::Read;
use std::io::Write;
use std::net::SocketAddr;
use std::net::TcpListener;
use std::net::TcpStream;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::sync::Mutex;
use std::thread::JoinHandle;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

const OPENAI_SANDBOX_LOGIN_CHILD_ENV: &str = "CODEX_ROUTER_OPENAI_LOGIN_SANDBOX_CHILD";
const OPENAI_SANDBOX_PROBE_PATH_ENV: &str = "CODEX_ROUTER_OPENAI_LOGIN_SANDBOX_PROBE_PATH";
const CLAUDE_SANDBOX_LOGIN_CHILD_ENV: &str = "CODEX_ROUTER_CLAUDE_LOGIN_SANDBOX_CHILD";
const CLAUDE_SANDBOX_PROBE_PATH_ENV: &str = "CODEX_ROUTER_CLAUDE_LOGIN_SANDBOX_PROBE_PATH";
const SANDBOX_CHILD_PROCESS_MARKER_ENV: &str = "CODEX_ROUTER_CLAUDE_LOGIN_CHILD_PROCESS_MARKER";

#[test]
fn openai_device_login_succeeds_when_native_auth_reads_are_denied() {
    if run_sandbox_file_read_canary_if_requested(OPENAI_SANDBOX_PROBE_PATH_ENV) {
        return;
    }

    if std::env::var_os(OPENAI_SANDBOX_LOGIN_CHILD_ENV).is_some() {
        openai_device_login_activates_fake_issuer_tokens_without_plaintext_files();
        return;
    }

    run_sandboxed_native_auth_flow(
        "openai-login-sandbox-probe",
        "tests::account_login_sandbox_tests::openai_device_login_succeeds_when_native_auth_reads_are_denied",
        "sandboxed OpenAI device login",
        OPENAI_SANDBOX_LOGIN_CHILD_ENV,
        OPENAI_SANDBOX_PROBE_PATH_ENV,
        None,
    );
}

#[test]
fn claude_login_and_refresh_succeed_when_native_auth_reads_are_denied() {
    if run_sandbox_file_read_canary_if_requested(CLAUDE_SANDBOX_PROBE_PATH_ENV) {
        return;
    }

    if std::env::var_os(CLAUDE_SANDBOX_LOGIN_CHILD_ENV).is_some() {
        claude_login_and_refresh_use_only_router_owned_credentials();
        return;
    }

    run_sandboxed_native_auth_flow(
        "claude-login-sandbox-probe",
        "tests::account_login_sandbox_tests::claude_login_and_refresh_succeed_when_native_auth_reads_are_denied",
        "sandboxed Claude login and refresh",
        CLAUDE_SANDBOX_LOGIN_CHILD_ENV,
        CLAUDE_SANDBOX_PROBE_PATH_ENV,
        Some(SANDBOX_CHILD_PROCESS_MARKER_ENV),
    );
}

fn run_sandbox_file_read_canary_if_requested(probe_environment: &str) -> bool {
    let Some(probe_path) = std::env::var_os(probe_environment) else {
        return false;
    };
    let read_error = fs::read(probe_path).expect_err("sandbox should deny the probe read");
    assert_eq!(read_error.kind(), std::io::ErrorKind::PermissionDenied);
    true
}

fn run_sandboxed_native_auth_flow(
    test_root_label: &str,
    test_name: &str,
    operation_description: &str,
    login_child_environment: &str,
    probe_environment: &str,
    child_process_marker_environment: Option<&str>,
) {
    let test_root = TestRoot::new(test_root_label);
    must_ok(fs::create_dir_all(test_root.path()));
    let home = PathBuf::from(std::env::var_os("HOME").expect("HOME should be available"));
    let blocked_native_auth_paths = [
        home.join(".codex").join("auth.json"),
        home.join(".claude").join(".credentials.json"),
    ];
    let probe_file_path = test_root.path().join("probe-auth-file");
    must_ok(fs::write(&probe_file_path, b"synthetic sandbox probe"));
    let denied_probe_path = must_ok(fs::canonicalize(probe_file_path));
    let mut canary_denied_paths = blocked_native_auth_paths.to_vec();
    canary_denied_paths.push(denied_probe_path.clone());
    let test_binary = std::env::current_exe().expect("test binary path should be available");
    let probe_output = Command::new("/usr/bin/sandbox-exec")
        .arg("-p")
        .arg(sandbox_profile_denying_file_reads(&canary_denied_paths))
        .arg(&test_binary)
        .arg("--exact")
        .arg(test_name)
        .arg("--nocapture")
        .env(probe_environment, &denied_probe_path)
        .output()
        .expect("sandbox-exec should start the file-read probe");
    assert!(
        probe_output.status.success(),
        "sandbox should deny the synthetic file read; stderr: {}",
        String::from_utf8_lossy(&probe_output.stderr)
    );

    let sandbox_profile = sandbox_profile_denying_file_reads(&blocked_native_auth_paths);
    let login_start_unix_seconds = unix_timestamp_seconds();
    let child_process_stubs = child_process_marker_environment
        .map(|_| provider_cli_child_process_stubs(test_root.path()));
    let mut login_command = Command::new("/usr/bin/sandbox-exec");
    login_command
        .arg("-p")
        .arg(sandbox_profile)
        .arg(&test_binary)
        .arg("--exact")
        .arg(test_name)
        .arg("--nocapture")
        .env(login_child_environment, "1");
    if let (Some(marker_environment), Some((isolated_path, child_process_marker))) = (
        child_process_marker_environment,
        child_process_stubs.as_ref(),
    ) {
        login_command
            .env(marker_environment, child_process_marker)
            .env("PATH", isolated_path);
    }
    let login_output = login_command
        .output()
        .expect("sandbox-exec should start the sandboxed authentication test");
    assert!(
        login_output.status.success(),
        "{operation_description} should succeed with native auth reads denied; stderr: {}",
        String::from_utf8_lossy(&login_output.stderr)
    );
    if let Some((_, child_process_marker)) = child_process_stubs {
        assert!(
            !child_process_marker.exists(),
            "{operation_description} must not invoke a provider CLI child"
        );
    }

    for blocked_path in &blocked_native_auth_paths {
        let blocked_credential_read_records =
            sandbox_violation_records(blocked_path, login_start_unix_seconds);
        assert!(
            blocked_credential_read_records.is_empty(),
            "supplementary sandbox violation log should be empty for a protected native auth path"
        );
    }
}

fn sandbox_violation_records(path: &Path, start_unix_seconds: u64) -> Vec<serde_json::Value> {
    let escaped_path = escape_predicate_string(path);
    let predicate = format!(
        "subsystem == \"com.apple.sandbox.reporting\" AND category == \"violation\" AND (eventMessage CONTAINS[c] \"{escaped_path}\" OR composedMessage CONTAINS[c] \"{escaped_path}\")"
    );
    let log_output = Command::new("/usr/bin/log")
        .arg("show")
        .arg("--start")
        .arg(format!("@{start_unix_seconds}"))
        .arg("--style")
        .arg("json")
        .arg("--info")
        .arg("--debug")
        .arg("--predicate")
        .arg(predicate)
        .output()
        .expect("unified logging should be available for sandbox proof");
    assert!(
        log_output.status.success(),
        "sandbox violation log query should succeed; stderr: {}",
        String::from_utf8_lossy(&log_output.stderr)
    );
    serde_json::from_slice(&log_output.stdout).expect("sandbox violation log should be JSON")
}

fn unix_timestamp_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn sandbox_profile_denying_file_reads(paths: &[PathBuf]) -> String {
    let denied_paths = paths
        .iter()
        .map(|path| {
            format!(
                "(deny file-read* (literal \"{}\"))",
                escape_predicate_string(path)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("(version 1)\n(allow default)\n{denied_paths}\n")
}

fn escape_predicate_string(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
}

fn claude_login_and_refresh_use_only_router_owned_credentials() {
    let test_root = TestRoot::new("claude-login-refresh-native-auth-denial");
    must_ok(fs::create_dir_all(test_root.path()));
    let router_root = test_root.path().join("router");
    let secret_root = router_root.join("secrets");
    let (secret_store, read_trace, _) = must_ok(
        open_encrypted_credential_store_with_read_write_traces(&secret_root),
    );
    let issuer = FakeClaudeOAuthIssuer::spawn(vec![
        r#"{"access_token":"claude-login-access-canary","refresh_token":"claude-login-refresh-canary","expires_in":3600}"#.to_owned(),
        r#"{"access_token":"claude-refreshed-access-canary","refresh_token":"claude-refreshed-refresh-canary","expires_in":7200}"#.to_owned(),
    ]);
    let observed_state = Arc::new(Mutex::new(None));
    let login_flow = StateRecordingClaudeOAuthLoginFlow {
        inner: ClaudeOAuthLoginFlow::with_test_issuer(issuer.endpoint(), "test-claude-client"),
        observed_state: observed_state.clone(),
    };
    let refresh_client =
        ClaudeOAuthRefreshClient::with_test_issuer(issuer.endpoint(), "test-claude-client");
    let command = parse_claude_login(&router_root, "Claude R18");
    let mut output = Vec::new();
    let mut reader = StateDrivenClaudeCallbackReader::new(observed_state);

    must_ok(
        crate::account::run_account_command_with_input_and_claude_flow_and_secret_store(
            &mut output,
            &mut reader,
            command,
            &login_flow,
            secret_store.clone(),
        ),
    );

    let login_output = String::from_utf8(output).expect("login output should be UTF-8");
    assert!(login_output.contains("https://claude.ai/oauth/authorize"));
    assert!(login_output.contains("logged in Claude account: Claude R18"));
    assert!(login_output.contains("account_id: acct_claude_r18"));
    for token_canary in ["claude-login-access-canary", "claude-login-refresh-canary"] {
        assert!(!login_output.contains(token_canary));
    }

    let login_read_events = read_trace.events();
    assert!(
        login_read_events.iter().any(|event| matches!(
            event,
            FileReadTraceEvent::FileOpenAttempted { path }
                if path.file_name().is_some_and(|name| name.to_string_lossy().ends_with(".v2"))
        )),
        "Claude login must trace encrypted credential verification reads"
    );
    assert_trace_contains_only_router_secret_reads(&login_read_events, &secret_root, "login");

    let runtime = super::cli_test_support::test_async_runtime();
    let state = must_ok(runtime.block_on(AsyncSqliteStateStore::open(
        &router_root.join("state.sqlite"),
    )));
    let account_id = AccountId::new("acct_claude_r18").expect("account id should be valid");
    let account = must_ok(runtime.block_on(state.load_account(&account_id)))
        .expect("Claude login should activate the account");
    assert_eq!(account.provider(), Provider::Claude);
    assert_eq!(account.status(), AccountStatus::Enabled);
    assert_eq!(account.active_credential_generation(), Some(1));

    let initial_key = must_ok(provider_credential_bundle_key(
        Provider::Claude,
        &account_id,
        1,
    ));
    let initial_bundle = must_ok(CredentialBundle::from_secret_string(
        Provider::Claude,
        must_ok(secret_store.read_secret(&initial_key)),
    ));
    let resolver_fixed_now = initial_bundle
        .expires_unix_seconds()
        .expect("Claude login credentials should have an expiry")
        + 1;
    let login_event_count = login_read_events.len();
    let resolver = AsyncRouterCredentialResolver::new(
        state.clone(),
        secret_store.clone(),
        refresh_client,
        Some(resolver_fixed_now),
    );
    must_ok(runtime.block_on(resolver.maintain_account_credentials(&account_id)));
    let refresh_read_events = read_trace.events();
    assert!(refresh_read_events.len() > login_event_count);
    let refresh_only_events = &refresh_read_events[login_event_count..];
    assert!(
        refresh_only_events.iter().any(|event| matches!(
            event,
            FileReadTraceEvent::FileOpenAttempted { path }
                if path.file_name().is_some_and(|name| name.to_string_lossy().ends_with(".v2"))
        )),
        "Claude refresh must trace encrypted credential reads"
    );
    assert_trace_contains_only_router_secret_reads(refresh_only_events, &secret_root, "refresh");

    let refreshed_account = must_ok(runtime.block_on(state.load_account(&account_id)))
        .expect("refreshed Claude account should remain active");
    assert_eq!(refreshed_account.active_credential_generation(), Some(2));
    let refreshed_key = must_ok(provider_credential_bundle_key(
        Provider::Claude,
        &account_id,
        2,
    ));
    let refreshed_bundle = must_ok(CredentialBundle::from_secret_string(
        Provider::Claude,
        must_ok(secret_store.read_secret(&refreshed_key)),
    ));
    assert_eq!(
        refreshed_bundle.access_token().expose_secret(),
        "claude-refreshed-access-canary"
    );
    assert_eq!(
        refreshed_bundle
            .refresh_token()
            .map(|token| token.expose_secret()),
        Some("claude-refreshed-refresh-canary")
    );

    let requests = issuer.finish();
    assert_eq!(requests.len(), 2);
    let login_request = request_body_json(&requests[0]);
    assert_eq!(login_request["grant_type"], "authorization_code");
    assert_eq!(login_request["code"], "claude-r18-authorization-code");
    assert_eq!(login_request["client_id"], "test-claude-client");
    assert_eq!(
        login_request["redirect_uri"],
        "https://platform.claude.com/oauth/code/callback"
    );
    assert_eq!(
        login_request["state"].as_str(),
        login_flow.observed_state().as_deref()
    );
    assert!(
        login_request["code_verifier"]
            .as_str()
            .is_some_and(|value| !value.is_empty())
    );

    let refresh_request = request_body_json(&requests[1]);
    assert_eq!(refresh_request["grant_type"], "refresh_token");
    assert_eq!(refresh_request["client_id"], "test-claude-client");
    assert_eq!(
        refresh_request["refresh_token"],
        "claude-login-refresh-canary"
    );

    drop(resolver);
    must_ok(runtime.block_on(state.close()));
}

fn parse_claude_login(router_root: &Path, label: &str) -> crate::account::AccountCommand {
    let command = super::CliCommand::parse([
        "account".into(),
        "login".into(),
        "--provider".into(),
        "claude".into(),
        "--label".into(),
        label.into(),
        "--router-root".into(),
        router_root.as_os_str().to_owned(),
    ])
    .expect("Claude login command should parse");
    let super::CliCommand::Account(command) = command else {
        panic!("Claude login should parse as an account command");
    };
    command
}

fn assert_trace_contains_only_router_secret_reads(
    events: &[FileReadTraceEvent],
    secret_root: &Path,
    operation_name: &str,
) {
    let home_root = PathBuf::from(std::env::var_os("HOME").expect("HOME should be available"));
    let native_auth_roots = [home_root.join(".codex"), home_root.join(".claude")];
    assert!(
        !events.is_empty(),
        "{operation_name} must produce read trace events"
    );
    for event in events {
        let path = match event {
            FileReadTraceEvent::FileOpenAttempted { path }
            | FileReadTraceEvent::DirectoryOpenAttempted { path } => path,
        };
        assert!(
            path.starts_with(secret_root),
            "{operation_name} opened a path outside the Router secret store: {path:?}"
        );
        assert!(
            native_auth_roots
                .iter()
                .all(|native_root| !path.starts_with(native_root)),
            "{operation_name} opened a path under a native auth store: {path:?}"
        );
    }
}

fn provider_cli_child_process_stubs(test_root: &Path) -> (std::ffi::OsString, PathBuf) {
    use std::os::unix::fs::PermissionsExt;

    let bin_directory = test_root.join("provider-cli-stubs");
    must_ok(fs::create_dir_all(&bin_directory));
    let process_marker = test_root.join("provider-cli-child-launched");
    let script = format!("#!/bin/sh\nprintf invoked >> \"${SANDBOX_CHILD_PROCESS_MARKER_ENV}\"\n");
    for executable_name in ["claude", "codex", "curl", "open", "security", "sh"] {
        let stub_path = bin_directory.join(executable_name);
        must_ok(fs::write(&stub_path, &script));
        must_ok(fs::set_permissions(
            &stub_path,
            fs::Permissions::from_mode(0o700),
        ));
    }

    let mut path_entries = vec![bin_directory];
    path_entries.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let isolated_path = std::env::join_paths(path_entries).expect("test PATH should be joinable");
    (isolated_path, process_marker)
}

struct StateRecordingClaudeOAuthLoginFlow {
    inner: ClaudeOAuthLoginFlow,
    observed_state: Arc<Mutex<Option<String>>>,
}

impl StateRecordingClaudeOAuthLoginFlow {
    fn observed_state(&self) -> Option<String> {
        self.observed_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl AccountLoginFlow for StateRecordingClaudeOAuthLoginFlow {
    type PendingLogin = PendingClaudeOAuthLogin;

    fn begin_login(&self) -> Result<Self::PendingLogin, LoginFlowError> {
        self.inner.begin_login()
    }

    fn authorization_url(&self, pending: &Self::PendingLogin) -> Result<String, LoginFlowError> {
        let authorization_url = self.inner.authorization_url(pending)?;
        let state = authorization_url
            .split_once("?")
            .and_then(|(_, query)| {
                query
                    .split('&')
                    .find_map(|parameter| parameter.strip_prefix("state="))
            })
            .ok_or(LoginFlowError::AuthorizationRequestFailed)?;
        *self
            .observed_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(state.to_owned());
        Ok(authorization_url)
    }

    fn finish_login(
        &self,
        pending: Self::PendingLogin,
        pasted_callback: &str,
    ) -> Result<CredentialBundle, LoginFlowError> {
        self.inner.finish_login(pending, pasted_callback)
    }
}

struct StateDrivenClaudeCallbackReader {
    observed_state: Arc<Mutex<Option<String>>>,
    buffer: Vec<u8>,
    position: usize,
}

impl StateDrivenClaudeCallbackReader {
    fn new(observed_state: Arc<Mutex<Option<String>>>) -> Self {
        Self {
            observed_state,
            buffer: Vec::new(),
            position: 0,
        }
    }
}

impl Read for StateDrivenClaudeCallbackReader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let available = BufRead::fill_buf(self)?;
        let amount = available.len().min(output.len());
        output[..amount].copy_from_slice(&available[..amount]);
        BufRead::consume(self, amount);
        Ok(amount)
    }
}

impl BufRead for StateDrivenClaudeCallbackReader {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        if self.buffer.is_empty() {
            let state = self
                .observed_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
                .expect("authorization URL should be written before callback input is read");
            self.buffer = format!("claude-r18-authorization-code#{state}\n").into_bytes();
        }
        Ok(&self.buffer[self.position..])
    }

    fn consume(&mut self, amount: usize) {
        self.position = self.position.saturating_add(amount).min(self.buffer.len());
    }
}

struct FakeClaudeOAuthIssuer {
    endpoint: String,
    address: SocketAddr,
    server: Option<JoinHandle<Vec<String>>>,
}

impl FakeClaudeOAuthIssuer {
    fn spawn(responses: Vec<String>) -> Self {
        let listener = must_ok(TcpListener::bind("127.0.0.1:0"));
        let address = must_ok(listener.local_addr());
        let server = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for body in responses {
                let (mut stream, _) = must_ok(listener.accept());
                let request = super::read_http_request_with_body(&mut stream);
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
            endpoint: format!("http://{address}/oauth/token"),
            address,
            server: Some(server),
        }
    }

    fn endpoint(&self) -> &str {
        &self.endpoint
    }

    fn finish(mut self) -> Vec<String> {
        let _ = TcpStream::connect(self.address);
        self.server
            .take()
            .expect("fake Claude issuer should still be running")
            .join()
            .expect("fake Claude issuer should finish")
    }
}

impl Drop for FakeClaudeOAuthIssuer {
    fn drop(&mut self) {
        if let Some(server) = self.server.take() {
            let _ = TcpStream::connect(self.address);
            let _ = server.join();
        }
    }
}

use codex_router_secret_store::SecretStore;
use codex_router_secret_store::account_tokens::AccountCredentialBundle;
use codex_router_secret_store::account_tokens::openai_account_credential_bundle_key;
use codex_router_state::sqlite::SqliteStateStore;
use std::borrow::Cow;
use std::fs;
use std::os::unix::fs as unix_fs;
use std::os::unix::process::ExitStatusExt;
use std::process::ExitStatus;
use std::process::Output;

use super::InstalledCodexSmokeMode;
use super::MockHttpSseTranscript;
use super::MockWebSocketTranscript;
use super::QUOTA_RECONNECT_PRIMARY;
use super::RETAIN_SMOKE_ROOT_ENV;
use super::RedactedTranscriptInput;
use super::RouterAuditObservation;
use super::RouterProcessObservation;
use super::SMOKE_EXPECTED_TEXT;
use super::SmokeContractAssertion;
use super::SmokeQuotaStatus;
use super::SmokeSeed;
use super::SmokeTempRoot;
use super::seed_smoke_account;

use super::assert_codex_visible_output;
use super::assert_redacted_three_websocket_payload;
use super::assert_smoke_contract;
use super::first_frame_shape_summary;
use super::run_hostile_no_token_smoke;
use super::run_installed_codex_http_sse_mock_smoke;
use super::run_installed_codex_mock_smoke;
use super::run_installed_codex_quota_reconnect_websocket_mock_smoke;
use super::run_installed_codex_s8_overlap_quota_websocket_mock_smoke;
use super::run_installed_codex_three_websocket_mock_e2e;
use super::run_installed_codex_three_websocket_mock_soak;
use super::run_installed_codex_websocket_mock_smoke;
use super::run_with_timeout;
use super::upstream_account_token;
use super::validate_copied_dev_state_roots;
use super::write_redacted_transcript;

#[test]
fn access_forwarding_smoke_seed_cannot_refresh_with_production_oauth() {
    let root = SmokeTempRoot::new("access-forwarding-no-refresh").expect("fixture root");
    let state = SqliteStateStore::open(&root.path().join("state.sqlite")).expect("fixture state");
    let secrets = codex_router_secret_store::test_support::open_encrypted_credential_store(
        root.path().join("secrets"),
    )
    .expect("fixture secrets");
    seed_smoke_account(&state, &secrets, QUOTA_RECONNECT_PRIMARY)
        .expect("smoke account should seed");
    let account_id = codex_router_core::ids::AccountId::new(QUOTA_RECONNECT_PRIMARY.account_id)
        .expect("fixture account id");
    let key = openai_account_credential_bundle_key(&account_id, 1).expect("bundle key");
    let bundle = AccountCredentialBundle::from_secret_string(
        secrets.read_secret(&key).expect("seeded bundle"),
    )
    .expect("bundle should decode");
    assert!(
        bundle.refresh_token().is_none(),
        "serve smoke must have no OAuth refresh capability"
    );
}

fn expect_string_error(result: Result<(), String>, context: &'static str) -> String {
    match result {
        Ok(()) => panic!("{context}"),
        Err(error) => error,
    }
}

fn success_status() -> ExitStatus {
    ExitStatus::from_raw(0)
}

fn valid_transcript(
    local_token_in_http_body: bool,
    local_token_in_first_frame: bool,
) -> MockWebSocketTranscript {
    let local_token = "local-token-canary";
    let first_frame = if local_token_in_first_frame {
        format!(
            r#"{{"model":"gpt-5.5","input":[{{"role":"user","content":[{{"type":"input_text","text":"hello"}}]}}],"stream":true,"token":"{local_token}"}}"#
        )
    } else {
        r#"{"model":"gpt-5.5","input":[{"role":"user","content":[{"type":"input_text","text":"hello"}]}],"stream":true}"#.to_owned()
    };
    MockWebSocketTranscript {
        headers: vec![(
            "authorization".to_owned(),
            format!("Bearer {}", upstream_account_token()),
        )],
        first_frame: first_frame.clone(),
        request_frames: vec![first_frame],
        websocket_request_frame_count: 1,
        http_probe_count: 0,
        http_sse: Some(MockHttpSseTranscript {
            request_line: "POST /v1/responses HTTP/1.1".to_owned(),
            headers: vec![(
                "authorization".to_owned(),
                format!("Bearer {}", upstream_account_token()),
            )],
            body: if local_token_in_http_body {
                format!(r#"{{"stream":true,"token":"{local_token}"}}"#)
            } else {
                r#"{"stream":true}"#.to_owned()
            },
        }),
    }
}

fn valid_quota_status() -> SmokeQuotaStatus {
    SmokeQuotaStatus {
        table: "matches 5h weekly resets available routing next use\n".to_owned(),
        plain: "account\tstatus\t5h\tweekly\tresets available\trouting\tnext use\nmatches\tenabled\t██████████ 91% resets in 4h\t██████░░░░ 54% resets in 6d\t-\t✓ preferred 5h 91%\tnext\nresponses route\tnext: matches\twhy: ✓ preferred 5h 91%\n".to_owned(),
        json: r#"{"preferred_next_account_hash":"hash_matches","accounts":[{"account_hash":"hash_matches","safe_account_label":"matches","preferred_next":true}]}"#.to_owned(),
    }
}

fn valid_router_audit_observation() -> RouterAuditObservation {
    RouterAuditObservation {
        http_sse_local_auth_validated: true,
        websocket_local_auth_validated: true,
    }
}

#[test]
fn websocket_scenario_all_runs_serial_concurrent_and_soak_filters() -> Result<(), String> {
    let script_path = super::workspace_root()?
        .join("tests")
        .join("smoke")
        .join("installed_codex_mock.sh");
    let script = fs::read_to_string(script_path)
        .map_err(|error| format!("failed to read smoke script: {error}"))?;

    assert!(
        script.contains(
            r#"elif [[ "${scenario}" == "all" && "${transport}" == "websocket" ]]; then"#
        )
    );
    assert!(script.contains(r#"run_test_filter "installed_codex_websocket_""#));
    assert!(script.contains(r#"run_test_filter "three_codex_websocket_concurrent_e2e_""#));
    assert!(script.contains(r#"run_three_websocket_soak_filter "three_codex_websocket_soak_""#));
    assert!(
        script.contains(
            r#"run_s8_overlap_quota_filter "installed_codex_websocket_s8_overlap_quota_""#
        )
    );
    assert!(
        script
            .contains(r#"run_quota_reconnect_filter "installed_codex_websocket_quota_reconnect_""#)
    );
    assert!(script.contains(r#"smoke_target_model="gpt-5.4-mini""#));
    assert!(script.contains(r#"smoke_client_summary="3 concurrent clients""#));
    assert!(script.contains(r#"smoke_client_summary="1 client with quota reconnect""#));
    assert!(script.contains(r#"smoke_client_summary="3 concurrent clients with quota reconnect""#));
    assert!(script.contains(r#"clients=%s"#));
    assert!(script.contains("bounded explicit exact-reply prompt"));
    assert!(script.contains("uses the existing codex CLI from PATH; it does not install Codex"));
    Ok(())
}

#[test]
fn proxy_db_runtime_isolation_uses_copied_roots_and_blocks_false_receipts() -> Result<(), String> {
    let workspace_root = super::workspace_root()?;
    let proxy_script = fs::read_to_string(
        workspace_root
            .join("tests")
            .join("smoke")
            .join("proxy_db_runtime_isolation.sh"),
    )
    .map_err(|error| format!("failed to read proxy DB smoke script: {error}"))?;
    let validator_script = fs::read_to_string(
        workspace_root
            .join("scripts")
            .join("validate-proxy-db-runtime-isolation-artifact.py"),
    )
    .map_err(|error| format!("failed to read proxy DB artifact validator: {error}"))?;
    let installed_script = fs::read_to_string(
        workspace_root
            .join("tests")
            .join("smoke")
            .join("installed_codex_mock.sh"),
    )
    .map_err(|error| format!("failed to read installed Codex smoke script: {error}"))?;

    assert!(proxy_script.contains(r#"--runtime-root-mode copied-dev-state"#));
    assert!(proxy_script.contains(r#"--router-root "${router_root_resolved}""#));
    assert!(proxy_script.contains(r#"--codex-home "${codex_home_resolved}""#));
    assert!(proxy_script.contains(r#"--process-home "${home_resolved}""#));
    assert!(proxy_script.contains("validate-proxy-db-runtime-isolation-artifact.py"));
    assert!(proxy_script.contains(r#"--scenario s8-overlap-quota"#));
    assert!(!proxy_script.contains(r#"--scenario quota-reconnect"#));
    assert!(proxy_script.contains("installed-codex-s8-overlap-quota-artifact.txt"));
    assert!(!proxy_script.contains("${quota_artifact_path}"));
    assert!(validator_script.contains(r#"runtime_roots.get("mode") == "copied-dev-state""#));
    assert!(validator_script.contains(r#"pressure.get("copied_db_pressure_proven") is True"#));
    assert!(
        validator_script.contains(r#"pressure.get("sqlite_lock_or_maintenance_pressure") is True"#)
    );
    assert!(
        validator_script.contains(r#"signal_ordering.get("signal_before_persistence") is True"#)
    );
    assert!(validator_script.contains(r#"account_selection.get("non_reselection") is True"#));
    assert!(validator_script.contains(r#"router_signal_count"#));
    assert!(validator_script.contains(r#"source_artifacts_same_s8_run_id"#));
    assert!(proxy_script.contains(r#"status=BLOCKED"#));
    assert!(proxy_script.contains(r#"scrubbed_signal_log_path"#));
    assert!(proxy_script.contains(r#"CODEX_ROUTER_S8_RUN_ID"#));
    assert!(proxy_script.contains(r#"receipt_path_value()"#));
    assert!(!proxy_script.contains(r#"printf 'router_root=%s\n' "${router_root_resolved}""#));
    assert!(!proxy_script.contains(r#"printf 'router_db=%s\n' "${router_db}""#));
    assert!(!proxy_script.contains(r#"printf 'codex_home=%s\n' "${codex_home_resolved}""#));
    assert!(!proxy_script.contains(r#"printf 'codex_db=%s\n' "${codex_db}""#));
    assert!(!proxy_script.contains(r#"printf 'sentinel_home=%s\n' "${home_resolved}""#));
    assert!(validator_script.contains(r#"pass_count="#));
    assert!(validator_script.contains(r#"fail_count="#));
    assert!(installed_script.contains(r#"--runtime-root-mode"#));
    assert!(installed_script.contains(r#"CODEX_ROUTER_INSTALLED_SMOKE_RUNTIME_ROOT_MODE"#));
    assert!(installed_script.contains(r#"CODEX_ROUTER_INSTALLED_SMOKE_ROUTER_ROOT"#));
    assert!(installed_script.contains(r#"CODEX_ROUTER_INSTALLED_SMOKE_CODEX_HOME"#));
    assert!(installed_script.contains(r#"CODEX_ROUTER_INSTALLED_SMOKE_PROCESS_HOME"#));
    assert!(installed_script.contains(r#"CODEX_ROUTER_S8_RUN_ID"#));
    assert!(installed_script.contains("os.path.realpath"));
    assert!(!installed_script.contains("os.path.abspath(candidate)"));

    Ok(())
}

fn valid_router_process_observation(test_root: &SmokeTempRoot) -> RouterProcessObservation {
    RouterProcessObservation {
        binary_path: test_root.path().join("target/debug/codex-router"),
        pid: 42,
        argv: vec!["serve".to_owned(), "--port".to_owned(), "8787".to_owned()],
        listener: "127.0.0.1:8787".to_owned(),
        readiness_line: "listening: 127.0.0.1:8787".to_owned(),
        cleanup_result: "terminated:signal: 9 (SIGKILL)".to_owned(),
    }
}

#[test]
fn smoke_contract_rejects_local_token_in_upstream_http_body() {
    let routable_upstream_tokens = [upstream_account_token().to_owned()];
    let quota_status = valid_quota_status();
    let upstream = valid_transcript(true, false);
    let error = match assert_smoke_contract(SmokeContractAssertion {
        mode: InstalledCodexSmokeMode::Combined,
        http_sse_codex_status: Some(&success_status()),
        websocket_codex_status: Some(&success_status()),
        upstream: &upstream,
        local_token: "local-token-canary",
        expected_account_label: "matches",
        expected_upstream_token: upstream_account_token(),
        routable_upstream_tokens: &routable_upstream_tokens,
        quota_status: &quota_status,
    }) {
        Ok(()) => panic!("HTTP/SSE body local-token leak must fail smoke contract"),
        Err(error) => error,
    };

    assert!(error.contains("HTTP/SSE request body leaked local router token"));
}

#[test]
fn smoke_contract_rejects_local_token_in_upstream_websocket_frame() {
    let routable_upstream_tokens = [upstream_account_token().to_owned()];
    let quota_status = valid_quota_status();
    let upstream = valid_transcript(false, true);
    let error = match assert_smoke_contract(SmokeContractAssertion {
        mode: InstalledCodexSmokeMode::Combined,
        http_sse_codex_status: Some(&success_status()),
        websocket_codex_status: Some(&success_status()),
        upstream: &upstream,
        local_token: "local-token-canary",
        expected_account_label: "matches",
        expected_upstream_token: upstream_account_token(),
        routable_upstream_tokens: &routable_upstream_tokens,
        quota_status: &quota_status,
    }) {
        Ok(()) => panic!("WebSocket frame local-token leak must fail smoke contract"),
        Err(error) => error,
    };

    assert!(error.contains("websocket frame leaked local router token"));
}

#[test]
fn smoke_visible_output_requires_last_message_text() {
    let test_root = match SmokeTempRoot::new("visible-output") {
        Ok(test_root) => test_root,
        Err(error) => panic!("failed to create temp root: {error}"),
    };
    let last_message_path = test_root.path().join("last-message.txt");
    if let Err(error) = fs::write(&last_message_path, "wrong text") {
        panic!("failed to write last-message fixture: {error}");
    }
    let output = Output {
        status: success_status(),
        stdout: SMOKE_EXPECTED_TEXT.as_bytes().to_vec(),
        stderr: Vec::new(),
    };

    let error = match assert_codex_visible_output("HTTP/SSE", &output, &last_message_path) {
        Ok(()) => panic!("wrong last-message text must fail visible output check"),
        Err(error) => error,
    };

    assert!(error.contains("last-message file did not contain expected response text"));
}

#[test]
fn mock_http_reader_consumes_chunked_request_body_before_responding() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap_or_else(|error| panic!("failed to bind chunked request fixture: {error}"));
    let address = listener
        .local_addr()
        .unwrap_or_else(|error| panic!("failed to read chunked fixture address: {error}"));
    let client = std::thread::spawn(move || {
        let mut stream = std::net::TcpStream::connect(address)
            .unwrap_or_else(|error| panic!("failed to connect chunked fixture: {error}"));
        std::io::Write::write_all(
            &mut stream,
            b"POST /v1/responses HTTP/1.1\r\nHost: 127.0.0.1\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n",
        )
        .unwrap_or_else(|error| panic!("failed to write chunked fixture: {error}"));
    });
    let (mut stream, _peer) = listener
        .accept()
        .unwrap_or_else(|error| panic!("failed to accept chunked fixture: {error}"));

    let request = super::read_http_request(&mut stream)
        .unwrap_or_else(|error| panic!("failed to read chunked request: {error}"));

    assert_eq!(request.request_line, "POST /v1/responses HTTP/1.1");
    assert_eq!(request.body, "hello world");
    client
        .join()
        .unwrap_or_else(|error| panic!("chunked request client panicked: {error:?}"));
}

#[test]
fn timed_out_codex_output_suppresses_captured_stdout_stderr() {
    let mut command = std::process::Command::new("sh");
    command
        .arg("-c")
        .arg(
            "printf 'local-secret-canary shell_command'; printf 'workdir: /tmp/raw-path session id: raw-session-id user prompt-canary diagnostic: waiting on loopback closed connection function_call_output' >&2; sleep 2",
        )
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let error = match run_with_timeout(command, std::time::Duration::from_millis(250)) {
        Ok(_) => panic!("sleeping command must time out"),
        Err(error) => error,
    };

    assert!(!error.contains("local-secret-canary"));
    assert!(!error.contains("raw-path"));
    assert!(!error.contains("raw-session-id"));
    assert!(!error.contains("prompt-canary"));
    assert!(!error.contains("waiting on loopback"));
    assert!(error.contains("captured stdout/stderr suppressed"));
    assert!(error.contains("stdout_preview=<suppressed>"));
    assert!(error.contains("stderr_preview=<suppressed>"));
    assert!(error.contains("stdout_markers=shell_command"));
    assert!(
        error.contains("stderr_markers=closed_connection,waiting_on_loopback,function_call_output")
    );
}

#[test]
fn run_with_timeout_drains_child_stderr_while_waiting() {
    let mut command = std::process::Command::new("python3");
    command
        .arg("-c")
        .arg("import sys; sys.stderr.write('x' * 200000); sys.stderr.flush()")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let output = match run_with_timeout(command, std::time::Duration::from_secs(2)) {
        Ok(output) => output,
        Err(error) => panic!("child output pipes should be drained while waiting: {error}"),
    };

    assert!(
        output.status.success(),
        "stderr writer should exit cleanly: {}",
        output.status
    );
    assert_eq!(output.stderr.len(), 200000);
}

#[test]
fn copied_dev_state_roots_reject_live_style_paths_before_mutation() {
    let test_root = SmokeTempRoot::new("copied-dev-state-live-style-rejection")
        .unwrap_or_else(|error| panic!("failed to create temp root: {error}"));
    let live_style_home = test_root.path().join("live-style-home");
    let router_root = live_style_home.join(".codex-router");
    let codex_home = live_style_home.join(".codex");

    let error = expect_string_error(
        validate_copied_dev_state_roots(&router_root, &codex_home, &live_style_home),
        "live-style copied-dev-state roots must be rejected",
    );

    assert!(
        error.contains("tmp/dev-state"),
        "error should tell operators to use repo-local tmp/dev-state roots: {error}"
    );
    assert!(
        !router_root.exists(),
        "unsafe router root must be rejected before directory creation"
    );
    assert!(
        !codex_home.exists(),
        "unsafe Codex home must be rejected before directory creation"
    );
}

#[test]
fn copied_dev_state_roots_reject_symlinked_roots_before_mutation() {
    let workspace_root =
        super::workspace_root().unwrap_or_else(|error| panic!("workspace root: {error}"));
    let dev_state_root = workspace_root.join("tmp/dev-state");
    fs::create_dir_all(&dev_state_root)
        .unwrap_or_else(|error| panic!("failed to create dev-state fixture root: {error}"));
    let test_root = SmokeTempRoot::new("copied-dev-state-symlink-root-rejection")
        .unwrap_or_else(|error| panic!("failed to create temp root: {error}"));
    let external_router_target = test_root.path().join("external-router");
    fs::create_dir_all(&external_router_target)
        .unwrap_or_else(|error| panic!("failed to create external router target: {error}"));
    let router_root = dev_state_root.join(format!("symlink-router-{}", std::process::id()));
    let _ = fs::remove_file(&router_root);
    unix_fs::symlink(&external_router_target, &router_root)
        .unwrap_or_else(|error| panic!("failed to create router symlink fixture: {error}"));
    let codex_home = dev_state_root.join(format!("codex-home-{}", std::process::id()));
    let process_home = dev_state_root.join(format!("process-home-{}", std::process::id()));

    let error = expect_string_error(
        validate_copied_dev_state_roots(&router_root, &codex_home, &process_home),
        "symlinked router root must be rejected",
    );

    assert!(
        error.contains("symlink") || error.contains("tmp/dev-state"),
        "error should explain copied-dev-state symlink/root rejection: {error}"
    );
    fs::remove_file(&router_root)
        .unwrap_or_else(|error| panic!("failed to clean router symlink fixture: {error}"));
}

#[test]
fn copied_dev_state_roots_reject_symlinked_db_and_secret_targets_before_mutation() {
    let workspace_root =
        super::workspace_root().unwrap_or_else(|error| panic!("workspace root: {error}"));
    let dev_state_root = workspace_root.join("tmp/dev-state");
    fs::create_dir_all(&dev_state_root)
        .unwrap_or_else(|error| panic!("failed to create dev-state fixture root: {error}"));
    let test_root = SmokeTempRoot::new("copied-dev-state-symlink-target-rejection")
        .unwrap_or_else(|error| panic!("failed to create temp root: {error}"));
    let fixture_suffix = format!("targets-{}", std::process::id());
    let router_root = dev_state_root.join(format!("router-{fixture_suffix}"));
    let codex_home = dev_state_root.join(format!("codex-{fixture_suffix}"));
    let process_home = dev_state_root.join(format!("home-{fixture_suffix}"));
    fs::create_dir_all(&router_root)
        .unwrap_or_else(|error| panic!("failed to create router root fixture: {error}"));
    fs::create_dir_all(&codex_home)
        .unwrap_or_else(|error| panic!("failed to create codex home fixture: {error}"));
    fs::create_dir_all(&process_home)
        .unwrap_or_else(|error| panic!("failed to create process home fixture: {error}"));
    let external_router_db = test_root.path().join("external-state.sqlite");
    let external_codex_db = test_root.path().join("external-state_5.sqlite");
    let external_secrets = test_root.path().join("external-secrets");
    fs::write(&external_router_db, b"outside router db")
        .unwrap_or_else(|error| panic!("failed to write external router DB: {error}"));
    fs::write(&external_codex_db, b"outside codex db")
        .unwrap_or_else(|error| panic!("failed to write external Codex DB: {error}"));
    fs::create_dir_all(&external_secrets)
        .unwrap_or_else(|error| panic!("failed to create external secrets dir: {error}"));
    let _ = fs::remove_file(router_root.join("state.sqlite"));
    let _ = fs::remove_file(codex_home.join("state_5.sqlite"));
    let _ = fs::remove_file(router_root.join("secrets"));
    unix_fs::symlink(&external_router_db, router_root.join("state.sqlite"))
        .unwrap_or_else(|error| panic!("failed to create router DB symlink: {error}"));
    unix_fs::symlink(&external_codex_db, codex_home.join("state_5.sqlite"))
        .unwrap_or_else(|error| panic!("failed to create Codex DB symlink: {error}"));
    unix_fs::symlink(&external_secrets, router_root.join("secrets"))
        .unwrap_or_else(|error| panic!("failed to create secrets symlink: {error}"));

    let error = expect_string_error(
        validate_copied_dev_state_roots(&router_root, &codex_home, &process_home),
        "symlinked copied-dev-state DB/secret targets must be rejected",
    );

    assert!(
        error.contains("symlink") || error.contains("tmp/dev-state"),
        "error should explain copied-dev-state symlink target rejection: {error}"
    );
    fs::remove_file(router_root.join("state.sqlite"))
        .unwrap_or_else(|error| panic!("failed to clean router DB symlink: {error}"));
    fs::remove_file(codex_home.join("state_5.sqlite"))
        .unwrap_or_else(|error| panic!("failed to clean Codex DB symlink: {error}"));
    fs::remove_file(router_root.join("secrets"))
        .unwrap_or_else(|error| panic!("failed to clean secrets symlink: {error}"));
}

#[test]
fn child_timeout_diagnostics_report_last_message_shape_without_content_or_path() {
    let test_root = SmokeTempRoot::new("child-timeout-diagnostics")
        .unwrap_or_else(|error| panic!("failed to create temp root: {error}"));
    let last_message_path = test_root.path().join("last-message.txt");
    fs::write(&last_message_path, SMOKE_EXPECTED_TEXT)
        .unwrap_or_else(|error| panic!("failed to write last message: {error}"));

    let diagnostics = super::codex_child_timeout_diagnostics(Some(2), &last_message_path);

    assert!(diagnostics.contains("client_index:2"));
    assert!(diagnostics.contains("last_message_exists:true"));
    assert!(diagnostics.contains("last_message_bytes:21"));
    assert!(diagnostics.contains("last_message_contains_expected:true"));
    assert!(!diagnostics.contains(SMOKE_EXPECTED_TEXT));
    assert!(!diagnostics.contains(&last_message_path.display().to_string()));
}

#[test]
fn smoke_temp_root_is_retained_when_explicitly_requested() {
    if std::env::var_os(RETAIN_SMOKE_ROOT_ENV).is_none() {
        eprintln!("skipping retained-root assertion; set {RETAIN_SMOKE_ROOT_ENV}=1 to run it");
        return;
    }
    let test_root = match SmokeTempRoot::new("retain-fixture") {
        Ok(test_root) => test_root,
        Err(error) => panic!("failed to create temp root: {error}"),
    };
    let retained_path = test_root.path().to_path_buf();

    drop(test_root);

    assert!(
        retained_path.exists(),
        "smoke temp root should remain when CODEX_ROUTER_RETAIN_SMOKE_ROOT=1"
    );
    fs::remove_dir_all(&retained_path).unwrap_or_else(|error| {
        panic!(
            "failed to clean retained fixture {}: {error}",
            retained_path.display()
        )
    });
}

#[test]
fn redacted_transcript_omits_forbidden_request_canaries() {
    let test_root = match SmokeTempRoot::new("redacted-transcript") {
        Ok(test_root) => test_root,
        Err(error) => panic!("failed to create temp root: {error}"),
    };
    let http_sse_last_message_path = test_root.path().join("http-sse-last-message.txt");
    let websocket_last_message_path = test_root.path().join("websocket-last-message.txt");
    let upstream = MockWebSocketTranscript {
        headers: vec![(
            "authorization".to_owned(),
            "Bearer installed-smoke-matches-token".to_owned(),
        )],
        first_frame: r#"{"type":"response.create","model":"gpt-5.5","input":[{"role":"user","content":[{"type":"input_text","text":"prompt-canary"}]}],"stream":true,"previous_response_id":"raw-previous-response-id-canary"}"#.to_owned(),
        request_frames: Vec::new(),
        websocket_request_frame_count: 1,
        http_probe_count: 0,
        http_sse: Some(MockHttpSseTranscript {
            request_line: "POST /v1/responses HTTP/1.1".to_owned(),
            headers: vec![(
                "authorization".to_owned(),
                "Bearer installed-smoke-matches-token".to_owned(),
            )],
            body: r#"{"stream":true,"input":"prompt-canary"}"#.to_owned(),
        }),
    };
    let transcript_path = match write_redacted_transcript(RedactedTranscriptInput {
        mode: InstalledCodexSmokeMode::Combined,
        codex_version: "OpenAI Codex v0.test",
        profile_path: test_root.path(),
        http_sse_codex_status: Some(&success_status()),
        http_sse_codex_stdout: Some(Cow::Borrowed("codex-router smoke ok")),
        http_sse_codex_stderr: Some(Cow::Borrowed("")),
        http_sse_last_message_path: Some(&http_sse_last_message_path),
        websocket_codex_status: Some(&success_status()),
        websocket_codex_stdout: Some(Cow::Borrowed("codex-router smoke ok")),
        websocket_codex_stderr: Some(Cow::Borrowed("")),
        websocket_last_message_path: Some(&websocket_last_message_path),
        upstream: &upstream,
        quota_status: &valid_quota_status(),
        expected_account_label: "matches",
        expected_upstream_token: upstream_account_token(),
        router_process: &valid_router_process_observation(&test_root),
        router_audit: &valid_router_audit_observation(),
    }) {
        Ok(path) => path,
        Err(error) => panic!("redacted transcript fixture failed: {error}"),
    };
    let payload = match fs::read_to_string(&transcript_path) {
        Ok(payload) => payload,
        Err(error) => panic!("failed to read transcript fixture: {error}"),
    };

    for forbidden in [
        "first_frame_model",
        "first_frame_has_input",
        "first_frame_stream",
        "gpt-5.5",
        "prompt-canary",
        "raw-previous-response-id-canary",
        "installed-smoke-matches-token",
    ] {
        assert!(
            !payload.contains(forbidden),
            "redacted transcript leaked {forbidden}"
        );
    }
    assert!(payload.contains("first_frame_shape"));
}

#[test]
fn three_websocket_artifact_rejects_raw_session_and_local_port_fields() {
    let seed = SmokeSeed {
        local_token_assignment: "CODEX_ROUTER_TOKEN=local-secret-canary".to_owned(),
        local_token: "local-secret-canary".to_owned(),
        expected_upstream_token: "upstream-secret-canary".to_owned(),
        expected_account_tag: "safe-tag".to_owned(),
        expected_account_label: "unsafe:raw-account-label".to_owned(),
        routable_upstream_tokens: vec!["upstream-secret-canary".to_owned()],
        quota_status: valid_quota_status(),
    };
    let payload = serde_json::json!({
        "router_process": {
            "listener": "127.0.0.1:43210",
            "readiness_line": "listening: 127.0.0.1:43210",
            "argv": ["serve", "--port", "43210"],
        },
        "router_websocket_registry": {
            "registered_session_ids": [1, 2, 3],
            "session_peer_addrs": [{"session_id": 1, "local_port": 60001}],
        },
        "runtime_correlations": [{
            "router_session_id": 1,
            "upstream_session_id": 10,
        }],
        "session_continuity": {
            "per_client_join_keys": [{
                "router_session_id": 1,
                "upstream_session_id": 10,
            }],
            "router_registered_session_ids": [1, 2, 3],
            "upstream_session_ids": [10, 11, 12],
        },
        "upstream": {
            "upstream_session_ids": [10, 11, 12],
            "upstream_client_sessions": [{"client_index": 0, "upstream_session_id": 10}],
        },
    });

    let error = expect_string_error(
        assert_redacted_three_websocket_payload(&payload.to_string(), &[], &seed),
        "raw session identifiers and local ports must be rejected",
    );

    assert!(
        error.contains("forbidden structural key")
            || error.contains("forbidden fragment")
            || error.contains("loopback endpoint with numeric port"),
        "unexpected redaction error: {error}"
    );
}

#[test]
fn persisted_websocket_registry_report_accepts_sanitized_schema_without_raw_identifiers()
-> Result<(), String> {
    let test_root = SmokeTempRoot::new("sanitized-registry-report")?;
    let report_path = test_root.path().join("websocket-registry-report.json");
    let report_json = serde_json::json!({
        "schema_version": 2,
        "handled_connections": 3,
        "websocket_registry": {
            "active_sessions": 0,
            "high_water_sessions": 3,
            "registered_sessions": 3,
            "closed_sessions": 3,
            "completed_response_sessions": 3,
            "forwarded_upstream_messages": 9,
            "registered_session_id_count": 3,
            "completed_session_id_count": 3,
            "closed_session_id_count": 3,
            "session_peer_addr_count": 3,
            "session_peer_join_observable": true,
            "completed_session_forwarded_upstream_message_counts": [3, 3, 3],
            "final_session_forwarded_upstream_message_counts": [3, 3, 3],
            "quota_reconnect_signal_count": 1,
            "quota_reconnect_signal_unix_ms": 1_720_000_000_000u64,
        }
    });
    let rendered = serde_json::to_string_pretty(&report_json)
        .map_err(|error| format!("failed to render sanitized registry report: {error}"))?;
    fs::write(&report_path, &rendered)
        .map_err(|error| format!("failed to write sanitized registry report: {error}"))?;

    for forbidden_key in [
        "registered_session_ids",
        "completed_session_ids",
        "closed_session_ids",
        "session_peer_addrs",
        "session_id",
        "peer_addr",
        "local_port",
    ] {
        assert!(
            !rendered.contains(&format!("\"{forbidden_key}\"")),
            "sanitized persisted registry report leaked raw key {forbidden_key}"
        );
    }
    assert!(
        !rendered.contains("127.0.0.1:") && !rendered.contains("[::1]:"),
        "sanitized persisted registry report leaked a raw loopback peer port"
    );

    let _report = super::RouterWebSocketRegistryReport::from_file(&report_path)?;

    Ok(())
}

#[test]
#[ignore = "T8a inventory preflight; route-native proof belongs to the next route-native slice"]
fn route_native_harness_inventory_preflight() {
    let first_frame = serde_json::json!({
        "type": "response.create",
        "model": "gpt-5.5",
        "input": [{"role": "user", "content": [{"type": "input_text", "text": "prompt-canary"}]}],
        "stream": true
    });
    let summary = first_frame_shape_summary(&first_frame);

    assert_eq!(
        summary.get("json_object").and_then(|value| value.as_bool()),
        Some(true)
    );
    assert_eq!(
        summary
            .get("non_prewarm_response_create")
            .and_then(|value| value.as_bool()),
        Some(true)
    );
    assert!(!summary.to_string().contains("prompt-canary"));
    assert!(!summary.to_string().contains("gpt-5.5"));
}

#[test]
#[ignore = "T8a inventory preflight; run full HTTP/SSE smoke through tests/smoke/installed_codex_mock.sh --transport http-sse"]
fn installed_codex_http_sse_harness_inventory_preflight() {
    let routable_upstream_tokens = [upstream_account_token().to_owned()];
    let quota_status = valid_quota_status();
    let upstream = valid_transcript(false, false);

    if let Err(error) = assert_smoke_contract(SmokeContractAssertion {
        mode: InstalledCodexSmokeMode::Combined,
        http_sse_codex_status: Some(&success_status()),
        websocket_codex_status: Some(&success_status()),
        upstream: &upstream,
        local_token: "local-token-canary",
        expected_account_label: "matches",
        expected_upstream_token: upstream_account_token(),
        routable_upstream_tokens: &routable_upstream_tokens,
        quota_status: &quota_status,
    }) {
        panic!("HTTP/SSE harness preflight failed: {error}");
    }
}

#[test]
#[ignore = "T9 installed-Codex HTTP/SSE e2e; run through tests/smoke/installed_codex_mock.sh --transport http-sse"]
fn installed_codex_http_sse_e2e_exercises_generated_profile_token() {
    let report = match run_installed_codex_http_sse_mock_smoke() {
        Ok(report) => report,
        Err(error) => panic!("installed Codex HTTP/SSE e2e failed: {error}"),
    };

    assert!(report.transcript_path().exists());
    println!(
        "codex_router_installed_codex_artifact={}",
        report.transcript_path().display()
    );
}

#[test]
#[ignore = "T8a inventory preflight; run full WebSocket smoke through tests/smoke/installed_codex_mock.sh --transport websocket"]
fn installed_codex_websocket_harness_inventory_preflight() {
    let routable_upstream_tokens = [upstream_account_token().to_owned()];
    let quota_status = valid_quota_status();
    let upstream = valid_transcript(false, false);

    if let Err(error) = assert_smoke_contract(SmokeContractAssertion {
        mode: InstalledCodexSmokeMode::Combined,
        http_sse_codex_status: Some(&success_status()),
        websocket_codex_status: Some(&success_status()),
        upstream: &upstream,
        local_token: "local-token-canary",
        expected_account_label: "matches",
        expected_upstream_token: upstream_account_token(),
        routable_upstream_tokens: &routable_upstream_tokens,
        quota_status: &quota_status,
    }) {
        panic!("WebSocket harness preflight failed: {error}");
    }
}

#[test]
#[ignore = "T10 installed-Codex WebSocket e2e; run through tests/smoke/installed_codex_mock.sh --transport websocket"]
fn installed_codex_websocket_e2e_exercises_generated_profile_token() {
    let report = match run_installed_codex_websocket_mock_smoke() {
        Ok(report) => report,
        Err(error) => panic!("installed Codex WebSocket e2e failed: {error}"),
    };

    assert!(report.transcript_path().exists());
    println!(
        "codex_router_installed_codex_artifact={}",
        report.transcript_path().display()
    );
}

#[test]
#[ignore = "T8 installed-Codex concurrent WebSocket e2e; run through tests/smoke/installed_codex_mock.sh --transport websocket --scenario concurrent"]
fn three_codex_websocket_concurrent_e2e_shares_router_pid_and_overlaps() {
    let report = match run_installed_codex_three_websocket_mock_e2e() {
        Ok(report) => report,
        Err(error) => panic!("installed Codex concurrent WebSocket e2e failed: {error}"),
    };

    assert!(report.transcript_path().exists());
    println!(
        "codex_router_three_websocket_artifact={}",
        report.transcript_path().display()
    );
}

#[test]
#[ignore = "T8 installed-Codex five-minute WebSocket soak; run through tests/smoke/installed_codex_mock.sh --transport websocket --scenario soak"]
fn three_codex_websocket_soak_holds_overlap_and_records_activity() {
    let report = match run_installed_codex_three_websocket_mock_soak() {
        Ok(report) => report,
        Err(error) => panic!("installed Codex concurrent WebSocket soak failed: {error}"),
    };

    assert!(report.transcript_path().exists());
    println!(
        "codex_router_three_websocket_artifact={}",
        report.transcript_path().display()
    );
}

#[test]
#[ignore = "S8 installed-Codex WebSocket overlap quota proof; run through tests/smoke/installed_codex_mock.sh --transport websocket --scenario s8-overlap-quota"]
fn installed_codex_websocket_s8_overlap_quota_reconnects_during_three_client_overlap() {
    let report = match run_installed_codex_s8_overlap_quota_websocket_mock_smoke() {
        Ok(report) => report,
        Err(error) => panic!("installed Codex S8 overlap quota smoke failed: {error}"),
    };

    assert!(report.transcript_path().exists());
    println!(
        "codex_router_s8_overlap_quota_artifact={}",
        report.transcript_path().display()
    );
}

#[test]
#[ignore = "T8 installed-Codex quota reconnect WebSocket e2e; run through tests/smoke/installed_codex_mock.sh --transport websocket --scenario quota-reconnect"]
fn installed_codex_websocket_quota_reconnect_e2e_switches_account_and_completes() {
    let report = match run_installed_codex_quota_reconnect_websocket_mock_smoke() {
        Ok(report) => report,
        Err(error) => panic!("installed Codex quota reconnect WebSocket e2e failed: {error}"),
    };

    assert!(report.transcript_path().exists());
    println!(
        "codex_router_quota_reconnect_artifact={}",
        report.transcript_path().display()
    );
}

#[test]
#[ignore = "run through tests/smoke/installed_codex_mock.sh"]
fn installed_codex_mock_smoke_exercises_generated_profile_token_and_websocket() {
    let report = match run_installed_codex_mock_smoke() {
        Ok(report) => report,
        Err(error) => panic!("installed Codex smoke failed: {error}"),
    };

    assert!(report.transcript_path().exists());
    println!(
        "codex_router_installed_codex_artifact={}",
        report.transcript_path().display()
    );
}

#[test]
#[ignore = "run through tests/smoke/installed_codex_mock.sh"]
fn installed_codex_hostile_no_token_smoke_keeps_upstream_empty() {
    if let Err(error) = run_hostile_no_token_smoke() {
        panic!("hostile no-token smoke failed: {error}");
    }
}

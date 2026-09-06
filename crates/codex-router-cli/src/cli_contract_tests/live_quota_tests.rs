use super::*;

#[test]
fn live_quota_command_rejects_api_key_auth_without_printing_key() {
    let test_root = TestRoot::new("live-quota-api-key");
    must_ok(fs::create_dir(test_root.path()));
    let auth_json = test_root.path().join("auth.json");
    must_ok(fs::write(
        &auth_json,
        r#"{"auth_mode":"api-key","OPENAI_API_KEY":"sk-local-secret-canary"}"#,
    ));

    let output = run_cli(
        [
            "codex-router",
            "live",
            "quota",
            "--auth-json",
            path_to_str(&auth_json),
            "--profile-label",
            "api-key-profile",
            "--approve-network-account-use",
        ],
        CliContext::new(Vec::new()),
    );

    assert!(output.stdout.contains("profile: api-key-profile\n"));
    assert!(output.stdout.contains("status: error\n"));
    assert!(
        output
            .stdout
            .contains("error: api_key_auth_not_quota_compatible\n")
    );
    assert!(!output.stdout.contains("sk-local-secret-canary"));
    assert!(output.stderr.is_empty());
}

#[test]
fn live_quota_refuses_network_account_use_without_explicit_approval() {
    let test_root = TestRoot::new("live-quota-approval-required");
    must_ok(fs::create_dir(test_root.path()));
    let auth_json = test_root.path().join("auth.json");
    must_ok(fs::write(
        &auth_json,
        r#"{"auth_mode":"chatgpt","tokens":{"access_token":"oauth-secret-canary"}}"#,
    ));

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let error = match run_with_io(
        vec![
            "codex-router".into(),
            "live".into(),
            "quota".into(),
            "--auth-json".into(),
            auth_json.as_os_str().to_owned(),
            "--profile-label".into(),
            "main".into(),
        ],
        &CliContext::new(Vec::new()),
        &mut stdout,
        &mut stderr,
    ) {
        Ok(()) => panic!("live quota without approval must fail"),
        Err(error) => error,
    };

    assert_eq!(
        error.to_string(),
        "live quota requires --approve-network-account-use or --dry-run"
    );
    assert!(stdout.is_empty());
    assert!(stderr.is_empty());
}

#[test]
fn live_quota_rejects_non_provider_base_url_before_token_egress() {
    let test_root = TestRoot::new("live-quota-disallowed-base-url");
    must_ok(fs::create_dir(test_root.path()));
    let auth_json = test_root.path().join("auth.json");
    must_ok(fs::write(
        &auth_json,
        r#"{"auth_mode":"chatgpt","tokens":{"access_token":"oauth-egress-canary"}}"#,
    ));

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let error = match run_with_io(
        vec![
            "codex-router".into(),
            "live".into(),
            "quota".into(),
            "--auth-json".into(),
            auth_json.as_os_str().to_owned(),
            "--profile-label".into(),
            "main".into(),
            "--base-url".into(),
            "http://attacker.example".into(),
            "--approve-network-account-use".into(),
        ],
        &CliContext::new(Vec::new()),
        &mut stdout,
        &mut stderr,
    ) {
        Ok(()) => panic!("disallowed live quota base URL must fail before token egress"),
        Err(error) => error,
    };
    let rendered_error = error.to_string();

    assert!(rendered_error.contains("live quota base URL is not allowed"));
    assert!(!rendered_error.contains("oauth-egress-canary"));
    assert!(stdout.is_empty());
    assert!(stderr.is_empty());
}

#[test]
fn live_quota_dry_run_discovers_profiles_without_provider_io_or_token_output() {
    let test_root = TestRoot::new("live-quota-dry-run");
    must_ok(fs::create_dir(test_root.path()));
    let profiles_root = test_root.path().join("profiles");
    let profile_root = profiles_root.join("main");
    must_ok(fs::create_dir_all(&profile_root));
    must_ok(fs::write(
        profile_root.join("auth.json"),
        r#"{"auth_mode":"chatgpt","tokens":{"access_token":"dry-run-secret-canary"}}"#,
    ));

    let output = run_cli(
        [
            "codex-router",
            "live",
            "quota",
            "--profiles-root",
            path_to_str(&profiles_root),
            "--dry-run",
        ],
        CliContext::new(Vec::new()),
    );

    assert!(output.stdout.contains("profile: main\n"));
    assert!(output.stdout.contains("status: dry-run\n"));
    assert!(output.stdout.contains("network: not-run\n"));
    assert!(output.stdout.contains("generation: not-run\n"));
    assert!(!output.stdout.contains("dry-run-secret-canary"));
    assert!(output.stderr.is_empty());
}

#[test]
fn live_quota_profiles_root_fetches_oauth_usage_without_printing_token() {
    let test_root = TestRoot::new("live-quota-profiles");
    must_ok(fs::create_dir(test_root.path()));
    let profiles_root = test_root.path().join("profiles");
    let profile_root = profiles_root.join("main");
    must_ok(fs::create_dir_all(&profile_root));
    let auth_json = profile_root.join("auth.json");
    must_ok(fs::write(
        &auth_json,
        r#"{"auth_mode":"chatgpt","tokens":{"access_token":"oauth-secret-canary"}}"#,
    ));
    let login_root = profiles_root.join(".login-ephemeral");
    must_ok(fs::create_dir_all(&login_root));
    must_ok(fs::write(
        login_root.join("auth.json"),
        r#"{"auth_mode":"chatgpt","tokens":{"access_token":"ignored-token-canary"}}"#,
    ));

    let listener = must_ok(TcpListener::bind("127.0.0.1:0"));
    let address = must_ok(listener.local_addr());
    let server_thread = thread::spawn(move || {
        let (mut stream, _peer_address) = match listener.accept() {
            Ok(connection) => connection,
            Err(error) => panic!("quota mock should accept: {error}"),
        };
        if let Err(error) = stream.set_read_timeout(Some(Duration::from_secs(2))) {
            panic!("quota mock should set read timeout: {error}");
        }
        let mut buffer = [0_u8; 4096];
        let bytes_read = match stream.read(&mut buffer) {
            Ok(bytes_read) => bytes_read,
            Err(error) => panic!("quota mock should read request: {error}"),
        };
        let request = String::from_utf8_lossy(&buffer[..bytes_read]);
        assert!(request.starts_with("GET /api/codex/usage HTTP/1.1\r\n"));
        assert!(request.contains("authorization: Bearer oauth-secret-canary\r\n"));
        let body = r#"{"rate_limit":{"primary_window":{"used_percent":25,"reset_at":2000,"limit_window_seconds":18000},"secondary_window":{"used_percent":80,"reset_at":9000,"limit_window_seconds":604800}},"additional_rate_limits":[{}]}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        if let Err(error) = stream.write_all(response.as_bytes()) {
            panic!("quota mock should write response: {error}");
        }
    });
    let base_url = format!("http://{address}");

    let output = run_cli(
        [
            "codex-router",
            "live",
            "quota",
            "--profiles-root",
            path_to_str(&profiles_root),
            "--base-url",
            base_url.as_str(),
            "--approve-network-account-use",
        ],
        CliContext::new(Vec::new()),
    );

    assert!(output.stdout.contains("profile: main\n"));
    assert!(output.stdout.contains("status: ok\n"));
    assert!(
        output
            .stdout
            .contains("rate_limit.primary: remaining_percent=75")
    );
    assert!(
        output
            .stdout
            .contains("rate_limit.secondary: remaining_percent=20")
    );
    assert!(output.stdout.contains("additional_rate_limit_count: 1\n"));
    assert!(!output.stdout.contains("oauth-secret-canary"));
    assert!(!output.stdout.contains("ignored-token-canary"));
    assert!(!output.stdout.contains(".login-ephemeral"));
    assert!(output.stderr.is_empty());

    match server_thread.join() {
        Ok(()) => {}
        Err(error) => panic!("quota mock thread panicked: {error:?}"),
    }
}

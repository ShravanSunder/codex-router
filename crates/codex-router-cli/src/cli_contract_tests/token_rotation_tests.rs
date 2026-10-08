use super::*;

#[tokio::test]
async fn local_token_reload_watcher_reports_generation_changes() {
    let test_root = TestRoot::new("local-token-reload-watcher");
    must_ok(fs::create_dir(test_root.path()));
    let secret_root = test_root.path().join("secrets");
    let secret_store = must_ok(FileSecretStore::open(&secret_root));
    let token_service = LocalRouterTokenService::new(secret_store.clone());
    let initial_token = must_ok(token_service.rotate_with_token("watch-token-a"));
    let (reload_sender, mut reload_receiver) = tokio::sync::mpsc::unbounded_channel();
    let mut watcher =
        LocalTokenReloadWatcher::start(secret_store, initial_token.generation(), move |auth| {
            let _send_result = reload_sender.send(auth.current_generation());
        });

    let rotated_token = must_ok(token_service.rotate_with_token("watch-token-b"));
    let observed_generation = tokio::time::timeout(Duration::from_secs(2), reload_receiver.recv())
        .await
        .unwrap_or_else(|error| panic!("watcher should report the new token generation: {error}"))
        .expect("token watcher should send its changed generation");

    assert_eq!(observed_generation, rotated_token.generation());
    watcher.shutdown().await;
    tokio::time::timeout(
        Duration::from_secs(1),
        tokio::time::sleep(Duration::from_millis(1)),
    )
    .await
    .expect("caller runtime should still schedule after token-watcher shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn serve_scopes_claude_token_rotation_and_keeps_codex_optional() {
    let test_root = TestRoot::new("serve-claude-token-scope-rotation");
    must_ok(fs::create_dir(test_root.path()));
    let state_path = test_root.path().join("state.sqlite");
    let secret_root = test_root.path().join("secrets");
    let state = must_ok(SqliteStateStore::open(&state_path));
    let secrets = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store(&secret_root),
    );
    let account_id = account_id("acct_cli_claude_token_scope");
    let account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id.clone(),
        "cli-claude-token-scope",
        AccountStatus::Enabled,
    )
    .with_active_credential_generation(1);
    must_ok(AccountStateRepository::upsert_account(&state, &account));
    let snapshot =
        PersistedQuotaSnapshot::new(account_id.clone(), QuotaSnapshotSource::MockEndpoint)
            .with_observed_unix_seconds(1_000)
            .with_route_band("responses", 100);
    must_ok(QuotaSnapshotRepository::upsert_snapshot(&state, &snapshot));
    persist_effective_selector_window(&state, &account_id, "responses", 100);
    let upstream_token_key = must_ok(openai_account_credential_bundle_key(&account_id, 1));
    let upstream_credential_bundle = must_ok(
        AccountCredentialBundle::imported_codex_auth(
            "cli-claude-scope-upstream-token",
            Some("cli-claude-scope-upstream-refresh-token".to_owned()),
        )
        .to_secret_string(),
    );
    must_ok(secrets.write_secret(&upstream_token_key, &upstream_credential_bundle));

    let upstream_listener = must_ok(TcpListener::bind("127.0.0.1:0"));
    let upstream_address = must_ok(upstream_listener.local_addr());
    let (upstream_sender, upstream_receiver) = mpsc::channel();
    let upstream_thread = thread::spawn(move || {
        let (mut stream, _peer_address) = match upstream_listener.accept() {
            Ok(connection) => connection,
            Err(error) => panic!("mock upstream should accept: {error}"),
        };
        let request = read_http_request_with_body(&mut stream);
        if request.is_empty() {
            return;
        }
        if let Err(error) = upstream_sender.send(request) {
            panic!("mock upstream request should record: {error}");
        }
        if let Err(error) =
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\ndata: ok\n\n")
        {
            panic!("mock upstream should write response: {error}");
        }
    });

    let router_port = reserve_loopback_port();
    let command = must_ok(CliCommand::parse([
        OsString::from("serve"),
        OsString::from("--listen-host"),
        OsString::from("127.0.0.1"),
        OsString::from("--port"),
        OsString::from(router_port.to_string()),
        OsString::from("--state-db"),
        state_path.as_os_str().to_os_string(),
        OsString::from("--secret-root"),
        secret_root.as_os_str().to_os_string(),
        OsString::from("--upstream-base-url"),
        OsString::from(format!("http://{upstream_address}/v1")),
        OsString::from("--now-unix-seconds"),
        OsString::from("1030"),
        OsString::from("--max-snapshot-age-seconds"),
        OsString::from("60"),
        OsString::from("--disable-background-quota-refresh"),
        OsString::from("--max-connections"),
        OsString::from("5"),
    ]));
    let CliCommand::Serve(command) = command else {
        panic!("serve arguments should parse as a serve command");
    };
    assert!(!command.require_local_token);

    let (serve_ready_sender, mut serve_ready_receiver) = tokio::sync::mpsc::unbounded_channel();
    let (token_reload_sender, mut token_reload_receiver) = tokio::sync::mpsc::unbounded_channel();
    let serve_task = tokio::spawn(async move {
        let mut stdout = Vec::new();
        let result = run_serve_command_with_upkeep_start_and_token_reload_observer(
            &mut stdout,
            command,
            secrets,
            move |state_database_path, credential_store, refresh_tasks| async move {
                let _send_result = serve_ready_sender.send(());
                credential_upkeep_worker::start_background_credential_upkeep_worker(
                    state_database_path,
                    credential_store,
                    refresh_tasks,
                )
                .await
            },
            move |generation| {
                let _send_result = token_reload_sender.send(generation);
            },
        )
        .await;
        (result, stdout)
    });
    tokio::time::timeout(Duration::from_secs(2), serve_ready_receiver.recv())
        .await
        .unwrap_or_else(|error| panic!("serve should signal readiness before requests: {error}"))
        .expect("serve should send readiness before requests");

    let token_store = must_ok(FileSecretStore::open(&secret_root));
    let token_service = LocalRouterTokenService::new(token_store);
    let token_a = must_ok(token_service.load_current());
    let missing_claude_token_response =
        send_claude_messages_request(router_port, None, "missing-token request");
    let valid_initial_claude_token_response = send_claude_messages_request(
        router_port,
        Some(token_a.token().expose_secret()),
        "initial-token request",
    );

    let mut rotate_stdout = Vec::new();
    let mut rotate_stderr = Vec::new();
    must_ok(run_with_io(
        [
            "codex-router",
            "token",
            "rotate",
            "--router-root",
            path_to_str(&secret_root),
        ]
        .into_iter()
        .map(OsString::from),
        &CliContext::new(Vec::new()),
        &mut rotate_stdout,
        &mut rotate_stderr,
    ));
    let token_b = must_ok(token_service.load_current());
    let reload_observation =
        tokio::time::timeout(Duration::from_secs(2), token_reload_receiver.recv())
            .await
            .unwrap_or_else(|error| panic!("token reload observer should run: {error}"));
    assert_eq!(
        reload_observation,
        Some(token_b.generation()),
        "wait for the bounded token-reload event before checking either token"
    );
    let stale_claude_token_response = send_claude_messages_request(
        router_port,
        Some(token_a.token().expose_secret()),
        "stale-token request after reload",
    );
    let rotated_claude_token_response = send_claude_messages_request(
        router_port,
        Some(token_b.token().expose_secret()),
        "rotated-token request after reload",
    );
    let codex_response = send_tokenless_loopback_request_with_retry(
        router_port,
        br#"{"model":"gpt-5","serve":true}"#,
    );
    let upstream_request = match upstream_receiver.recv_timeout(Duration::from_secs(2)) {
        Ok(request) => Some(request),
        Err(_error) => {
            if let Ok(wakeup_connection) = TcpStream::connect(upstream_address) {
                drop(wakeup_connection);
            }
            None
        }
    };
    let (serve_result, stdout) = serve_task
        .await
        .unwrap_or_else(|error| panic!("serve task should join: {error}"));
    must_ok(serve_result);
    match upstream_thread.join() {
        Ok(()) => {}
        Err(error) => panic!("mock upstream thread panicked: {error:?}"),
    }

    assert_eq!(token_a.generation().as_u64(), 1);
    assert_eq!(String::from_utf8_lossy(&rotate_stdout), "generation: 2\n");
    assert!(rotate_stderr.is_empty());
    assert_eq!(token_b.generation().as_u64(), 2);
    assert!(
        missing_claude_token_response.starts_with("HTTP/1.1 401 Unauthorized\r\n"),
        "Claude admission should require a token: {missing_claude_token_response}"
    );
    assert!(
        !valid_initial_claude_token_response.starts_with("HTTP/1.1 401 Unauthorized\r\n"),
        "the initial Claude token should pass admission: {valid_initial_claude_token_response}"
    );
    assert!(
        stale_claude_token_response.starts_with("HTTP/1.1 401 Unauthorized\r\n"),
        "the replaced Claude token should reject: {stale_claude_token_response}"
    );
    assert!(
        !rotated_claude_token_response.starts_with("HTTP/1.1 401 Unauthorized\r\n"),
        "the rotated Claude token should pass admission: {rotated_claude_token_response}"
    );
    assert!(codex_response.starts_with("HTTP/1.1 200 OK\r\n"));
    let upstream_request = match upstream_request {
        Some(request) => request,
        None => panic!("tokenless Codex request should reach the fake upstream"),
    };
    assert!(upstream_request.starts_with("POST /v1/responses HTTP/1.1\r\n"));
    assert!(upstream_request.contains("authorization: Bearer cli-claude-scope-upstream-token\r\n"));
    assert!(!upstream_request.contains("X-Codex-Router-Token"));
    assert!(String::from_utf8_lossy(&stdout).contains("listening: 127.0.0.1:"));
}

#[test]
#[allow(clippy::result_large_err)]
fn serve_command_reloads_token_rotation_without_restart() {
    let test_root = TestRoot::new("serve-command-token-rotation");
    must_ok(fs::create_dir(test_root.path()));
    let state_path = test_root.path().join("state.sqlite");
    let secret_root = test_root.path().join("secrets");
    let state = must_ok(SqliteStateStore::open(&state_path));
    let secrets = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store(&secret_root),
    );
    let token_service = LocalRouterTokenService::new(secrets.clone());
    must_ok(token_service.rotate_with_token("token-a"));
    let account_id = account_id("acct_cli_rotate");
    let account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id.clone(),
        "cli-rotate",
        AccountStatus::Enabled,
    )
    .with_active_credential_generation(1);
    must_ok(AccountStateRepository::upsert_account(&state, &account));
    let snapshot =
        PersistedQuotaSnapshot::new(account_id.clone(), QuotaSnapshotSource::MockEndpoint)
            .with_observed_unix_seconds(1_000)
            .with_route_band("responses", 100);
    must_ok(QuotaSnapshotRepository::upsert_snapshot(&state, &snapshot));
    persist_effective_selector_window(&state, &account_id, "responses", 100);
    let upstream_token_key = must_ok(openai_account_credential_bundle_key(&account_id, 1));
    let upstream_credential_bundle = must_ok(
        AccountCredentialBundle::imported_codex_auth(
            "cli-rotation-upstream-token",
            Some("cli-rotation-upstream-refresh-token".to_owned()),
        )
        .to_secret_string(),
    );
    must_ok(secrets.write_secret(&upstream_token_key, &upstream_credential_bundle));

    let upstream_listener = must_ok(TcpListener::bind("127.0.0.1:0"));
    let upstream_address = must_ok(upstream_listener.local_addr());
    let (upstream_sender, upstream_receiver) = mpsc::channel();
    let (release_sender, release_receiver) = mpsc::channel();
    let upstream_thread = thread::spawn(move || {
        let (stream, _peer_address) = match upstream_listener.accept() {
            Ok(connection) => connection,
            Err(error) => panic!("mock websocket upstream should accept: {error}"),
        };
        let mut websocket = match accept_hdr(stream, |_request: &Request, response: Response| {
            Ok(response)
        }) {
            Ok(websocket) => websocket,
            Err(error) => {
                panic!("mock websocket upstream handshake should accept: {error}")
            }
        };
        let first_frame = match websocket.read() {
            Ok(message) => message,
            Err(error) => panic!("mock websocket upstream should read first frame: {error}"),
        };
        if let Err(error) = upstream_sender.send(first_frame.to_string()) {
            panic!("mock websocket upstream first frame should record: {error}");
        }
        let _released = release_receiver.recv_timeout(std::time::Duration::from_secs(2));
        drop(websocket);

        let (mut stream, _peer_address) = match upstream_listener.accept() {
            Ok(connection) => connection,
            Err(error) => panic!("mock HTTP upstream should accept after rotation: {error}"),
        };
        let request = read_http_request_with_body(&mut stream);
        if let Err(error) = upstream_sender.send(request) {
            panic!("mock HTTP upstream request should record: {error}");
        }
        if let Err(error) =
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\ndata: ok\n\n")
        {
            panic!("mock HTTP upstream should write response: {error}");
        }
    });

    let router_port = reserve_loopback_port();
    let router_port_text = router_port.to_string();
    let upstream_base_url = format!("http://{upstream_address}/v1");
    let state_arg = state_path;
    let secret_arg = secret_root.clone();
    let serve_thread = thread::spawn(move || {
        run_cli(
            [
                "codex-router",
                "serve",
                "--listen-host",
                "127.0.0.1",
                "--port",
                router_port_text.as_str(),
                "--state-db",
                path_to_str(&state_arg),
                "--secret-root",
                path_to_str(&secret_arg),
                "--upstream-base-url",
                upstream_base_url.as_str(),
                "--now-unix-seconds",
                "1030",
                "--max-snapshot-age-seconds",
                "60",
                "--disable-background-quota-refresh",
                "--require-local-token",
                "--max-connections",
                "3",
            ],
            CliContext::new(Vec::new()),
        )
    });

    let mut client = connect_websocket_with_retry(router_port, "token-a");
    if let Err(error) = client.send(Message::text(r#"{"type":"response.create"}"#)) {
        panic!("local websocket client should send first frame: {error}");
    }
    let recorded_first_frame = match upstream_receiver.recv_timeout(Duration::from_secs(2)) {
        Ok(frame) => frame,
        Err(error) => panic!("upstream should receive first frame before rotation: {error}"),
    };
    assert_eq!(recorded_first_frame, r#"{"type":"response.create"}"#);

    let rotate_output = run_cli(
        [
            "codex-router",
            "token",
            "rotate",
            "--router-root",
            path_to_str(&secret_root),
        ],
        CliContext::new(Vec::new()),
    );
    assert_eq!(rotate_output.stdout, "generation: 2\n");
    assert!(rotate_output.stderr.is_empty());
    let token_b = must_ok(token_service.load_current());

    let (old_close_sender, old_close_receiver) = mpsc::channel();
    let old_client_thread = thread::spawn(move || {
        let read_result = client
            .read()
            .map(|message| matches!(message, Message::Close(_)));
        if let Err(error) = old_close_sender.send(read_result) {
            panic!("old websocket close result should send: {error}");
        }
    });
    let old_close_result = match old_close_receiver.recv_timeout(Duration::from_secs(2)) {
        Ok(result) => result,
        Err(error) => {
            let _ = release_sender.send(());
            panic!("old-token websocket should close after token rotation: {error}");
        }
    };
    if let Ok(false) = old_close_result {
        panic!("old-token websocket should close, got data message");
    }
    if let Err(error) = release_sender.send(()) {
        panic!("upstream release should send: {error}");
    }
    match old_client_thread.join() {
        Ok(()) => {}
        Err(error) => panic!("old websocket client thread panicked: {error:?}"),
    }

    let old_token_response =
        send_loopback_request_with_retry(router_port, "token-a", br#"{"old":true}"#);
    assert!(old_token_response.starts_with("HTTP/1.1 401 Unauthorized\r\n"));

    let new_token_response = send_loopback_request_with_retry(
        router_port,
        token_b.token().expose_secret(),
        br#"{"new":true}"#,
    );
    assert!(new_token_response.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(new_token_response.ends_with("\r\ndata: ok\n\n"));

    let upstream_http_request = match upstream_receiver.recv_timeout(Duration::from_secs(2)) {
        Ok(request) => request,
        Err(error) => panic!("mock HTTP upstream request should be recorded: {error}"),
    };
    assert!(
        upstream_http_request.contains("authorization: Bearer cli-rotation-upstream-token\r\n")
    );
    assert!(!upstream_http_request.contains("X-Codex-Router-Token"));
    assert!(!upstream_http_request.contains("token-a"));
    assert!(!upstream_http_request.contains(token_b.token().expose_secret()));

    let output = match serve_thread.join() {
        Ok(output) => output,
        Err(error) => panic!("serve thread panicked: {error:?}"),
    };
    assert!(
        output
            .stdout
            .contains(format!("listening: 127.0.0.1:{router_port}\n").as_str())
    );
    assert!(output.stderr.is_empty());

    match upstream_thread.join() {
        Ok(()) => {}
        Err(error) => panic!("mock upstream thread panicked: {error:?}"),
    }
}

fn send_claude_messages_request(port: u16, token: Option<&str>, request_case: &str) -> String {
    let body = br#"{"model":"claude-3-5-sonnet","max_tokens":16,"messages":[{"role":"user","content":"hello"}]}"#;
    let mut stream =
        must_ok(TcpStream::connect(("127.0.0.1", port)).map_err(|error| error.to_string()));
    let local_token_header = token.map_or_else(String::new, |token| {
        format!("X-Codex-Router-Token: {token}\r\n")
    });
    let request = format!(
        "POST /anthropic/v1/messages HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{local_token_header}\r\n",
        body.len()
    );
    must_ok(
        stream
            .write_all(request.as_bytes())
            .map_err(|error| error.to_string()),
    );
    must_ok(stream.write_all(body).map_err(|error| error.to_string()));
    match read_http_response_with_progress(&mut stream) {
        Ok(response) => response,
        Err(error) => panic!("Claude {request_case} failed: {error}"),
    }
}

fn read_http_response_with_progress(
    response_reader: &mut impl std::io::Read,
) -> Result<String, String> {
    let mut response_bytes = Vec::new();
    let mut buffer = [0_u8; 1024];
    loop {
        match response_reader.read(&mut buffer) {
            Ok(0) => {
                if let Some((received_body_bytes, declared_body_bytes)) =
                    declared_http_response_body_progress(&response_bytes)
                    && received_body_bytes < declared_body_bytes
                {
                    return Err(format!(
                        "HTTP response ended before its declared body completed; expected {declared_body_bytes} bytes, received {received_body_bytes}; {}",
                        describe_http_response_progress(&response_bytes)
                    ));
                }
                return response_bytes_to_string(response_bytes);
            }
            Ok(read_bytes) => {
                response_bytes.extend_from_slice(&buffer[..read_bytes]);
                if declared_http_response_body_is_complete(&response_bytes) {
                    return response_bytes_to_string(response_bytes);
                }
            }
            Err(error) => {
                return Err(format!(
                    "HTTP response read failed: {error}; {}",
                    describe_http_response_progress(&response_bytes)
                ));
            }
        }
    }
}

fn response_bytes_to_string(response_bytes: Vec<u8>) -> Result<String, String> {
    String::from_utf8(response_bytes)
        .map_err(|error| format!("HTTP response should be UTF-8: {error}"))
}

fn declared_http_response_body_is_complete(response_bytes: &[u8]) -> bool {
    declared_http_response_body_progress(response_bytes).is_some_and(
        |(received_body_bytes, declared_body_bytes)| received_body_bytes >= declared_body_bytes,
    )
}

fn declared_http_response_body_progress(response_bytes: &[u8]) -> Option<(usize, usize)> {
    let header_separator = response_bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")?;
    let headers = String::from_utf8_lossy(&response_bytes[..header_separator]);
    let declared_body_bytes = headers.lines().skip(1).find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("Content-Length")
            .then(|| value.trim().parse::<usize>().ok())
            .flatten()
    })?;
    let body_start = header_separator + 4;
    Some((
        response_bytes.len().saturating_sub(body_start),
        declared_body_bytes,
    ))
}

fn describe_http_response_progress(response_bytes: &[u8]) -> String {
    let header_separator = response_bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n");
    let (status_line, headers_complete, declared_body_bytes, received_body_bytes, body_complete) =
        match header_separator {
            Some(header_end) => {
                let headers = String::from_utf8_lossy(&response_bytes[..header_end]);
                let status_line = headers.lines().next().unwrap_or_default().to_owned();
                let declared_body_bytes = headers.lines().skip(1).find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("Content-Length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                });
                let body = &response_bytes[header_end + 4..];
                (
                    status_line,
                    true,
                    declared_body_bytes,
                    body.len(),
                    declared_body_bytes.map(|expected_bytes| body.len() >= expected_bytes),
                )
            }
            None => (
                String::from_utf8_lossy(response_bytes)
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .to_owned(),
                false,
                None,
                0,
                None,
            ),
        };
    let response_text = String::from_utf8_lossy(response_bytes);
    let response_bytes_hex = response_bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "status_line={status_line:?}; headers_complete={headers_complete}; declared_body_bytes={declared_body_bytes:?}; received_body_bytes={received_body_bytes}; body_complete={body_complete:?}; response_text={response_text:?}; response_bytes_hex={response_bytes_hex}"
    )
}

#[cfg(test)]
#[path = "token_rotation_tests/response_reader_tests.rs"]
mod response_reader_tests;

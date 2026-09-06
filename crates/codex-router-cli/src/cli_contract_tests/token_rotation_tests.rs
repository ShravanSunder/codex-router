use super::*;

#[test]
#[allow(clippy::result_large_err)]
fn serve_command_reloads_token_rotation_without_restart() {
    let test_root = TestRoot::new("serve-command-token-rotation");
    must_ok(fs::create_dir(test_root.path()));
    let state_path = test_root.path().join("state.sqlite");
    let secret_root = test_root.path().join("secrets");
    let state = must_ok(SqliteStateStore::open(&state_path));
    let secrets = must_ok(FileSecretStore::open(&secret_root));
    let token_service = LocalRouterTokenService::new(secrets.clone());
    must_ok(token_service.rotate_with_token("token-a"));
    let account_id = account_id("acct_cli_rotate");
    let account = AccountRecord::new(account_id.clone(), "cli-rotate", AccountStatus::Enabled)
        .with_active_credential_generation(1);
    must_ok(AccountStateRepository::upsert_account(&state, &account));
    let snapshot =
        PersistedQuotaSnapshot::new(account_id.clone(), QuotaSnapshotSource::MockEndpoint)
            .with_observed_unix_seconds(1_000)
            .with_route_band("responses", 100);
    must_ok(QuotaSnapshotRepository::upsert_snapshot(&state, &snapshot));
    persist_effective_selector_window(&state, &account_id, "responses", 100);
    let upstream_token_key = must_ok(account_credential_bundle_key(&account_id, 1));
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

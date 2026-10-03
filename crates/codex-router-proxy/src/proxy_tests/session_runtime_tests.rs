use super::*;

#[test]
fn assembled_http_runtime_reuses_live_session_affinity_and_persists_timestamp() {
    const RETENTION_NOW: u64 = 10 * 86_400;
    const EVENT_RETENTION: u64 = 7 * 86_400;
    let temp_dir = ProxyTestTempDir::new("runtime_http_session_affinity");
    let database_path = temp_dir.path().join("state.sqlite");
    let secret_path = temp_dir.path().join("secrets");
    let state = SqliteStateStore::open(&database_path).expect("state store should open");
    let secrets =
        codex_router_secret_store::test_support::open_encrypted_credential_store(&secret_path)
            .expect("secret store should open");
    let alpha = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_http_alpha"),
        "alpha",
        AccountStatus::Enabled,
    );
    let beta = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_http_beta"),
        "beta",
        AccountStatus::Enabled,
    );
    persist_account_with_snapshot_and_token(&state, &secrets, &alpha, 50, "http-alpha-token");
    persist_account_with_snapshot_and_token(&state, &secrets, &beta, 50, "http-beta-token");
    drop(state);
    seed_completed_active_session(
        &database_path,
        alpha.account_id(),
        "startup-expired",
        1,
        RETENTION_NOW - EVENT_RETENTION - 1,
    );
    seed_completed_active_session(
        &database_path,
        alpha.account_id(),
        "startup-exact-cutoff",
        2,
        RETENTION_NOW - EVENT_RETENTION,
    );
    let observation_runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("observation runtime should build");
    let observation_state = observation_runtime.block_on(async {
        AsyncSqliteStateStore::open_read_only(&database_path)
            .await
            .expect("observation state should open")
    });

    let upstream_listener = TcpListener::bind("127.0.0.1:0").expect("mock upstream should bind");
    let upstream_address = upstream_listener
        .local_addr()
        .expect("mock upstream address should read");
    let (upstream_sender, upstream_receiver) = mpsc::channel();
    let (release_upstream_sender, release_upstream_receiver) = mpsc::channel();
    let upstream_thread = thread::spawn(move || {
        let (mut first_stream, _) = upstream_listener
            .accept()
            .expect("mock upstream should accept first connection");
        let first_request = read_test_http_request(&mut first_stream);
        upstream_sender
            .send(first_request)
            .expect("first request should record");

        let (mut second_stream, _) = upstream_listener
            .accept()
            .expect("mock upstream should accept second connection");
        let second_request = read_test_http_request(&mut second_stream);
        upstream_sender
            .send(second_request)
            .expect("second request should record");
        release_upstream_receiver
            .recv()
            .expect("test should release upstream responses");

        for stream in [&mut second_stream, &mut first_stream] {
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                .expect("mock upstream should write response");
        }
    });
    let endpoint = UpstreamEndpoint::new(format!("http://{upstream_address}/v1"))
        .expect("mock endpoint should validate");
    let bind_address =
        LoopbackBindAddress::new("127.0.0.1", 0).expect("router bind address should validate");
    let config = LoopbackRouterRuntimeConfig::new(
        bind_address,
        endpoint,
        database_path.clone(),
        secret_path,
        LocalRouterTokenRecord::new(SecretString::new("current-token"), TokenGeneration::new(1)),
    )
    .with_quota_clock(RETENTION_NOW, 60);
    let (maintenance_completion_sender, maintenance_completion_receiver) = mpsc::channel();
    let runtime = LoopbackRouterRuntime::start_for_test_with_maintenance_completion_sender(
        config,
        maintenance_completion_sender,
    )
    .expect("router runtime should start");
    wait_for_responses_history_compaction(&maintenance_completion_receiver);
    assert_active_session_absent(&observation_runtime, &observation_state, "startup-expired");
    assert!(
        observed_active_session_reservation_ids(&observation_runtime, &observation_state)
            .iter()
            .any(|reservation_id| reservation_id == "startup-exact-cutoff"),
        "startup maintenance must preserve the exact-cutoff completed session"
    );
    seed_completed_active_session(
        &database_path,
        alpha.account_id(),
        "first-accepted-expired",
        3,
        RETENTION_NOW - EVENT_RETENTION - 1,
    );
    let router_address = runtime.local_addr();
    let runtime_thread = thread::spawn(move || runtime.serve_protocol_connections(2));
    let first_client = thread::spawn(move || {
        send_loopback_request_with_session(
            router_address,
            "assembled-http-session",
            br#"{"model":"gpt-5","turn":1}"#,
        )
    });
    let first_request = upstream_receiver
        .recv()
        .expect("first upstream request should record");
    wait_for_responses_history_compaction(&maintenance_completion_receiver);
    assert_active_session_absent(
        &observation_runtime,
        &observation_state,
        "first-accepted-expired",
    );
    seed_completed_active_session(
        &database_path,
        alpha.account_id(),
        "accepted-expired",
        4,
        RETENTION_NOW - EVENT_RETENTION - 1,
    );
    let second_client = thread::spawn(move || {
        send_loopback_request_with_session(
            router_address,
            "assembled-http-session",
            br#"{"model":"gpt-5","turn":2}"#,
        )
    });
    let second_request = upstream_receiver
        .recv()
        .expect("second upstream request should record");
    wait_for_responses_history_compaction(&maintenance_completion_receiver);
    release_upstream_sender
        .send(())
        .expect("upstream responses should release");

    assert_eq!(
        runtime_thread
            .join()
            .expect("runtime thread should not panic")
            .expect("runtime should serve two connections"),
        2
    );
    for client in [first_client, second_client] {
        assert!(
            client
                .join()
                .expect("client thread should not panic")
                .starts_with("HTTP/1.1 200 OK\r\n")
        );
    }
    for (request, expected_body) in [
        (&first_request, r#"{"model":"gpt-5","turn":1}"#),
        (&second_request, r#"{"model":"gpt-5","turn":2}"#),
    ] {
        assert!(request.contains("authorization: Bearer http-alpha-token\r\n"));
        assert!(request.ends_with(expected_body));
    }
    assert!(!second_request.contains("http-beta-token"));
    assert!(
        !observed_active_session_reservation_ids(&observation_runtime, &observation_state)
            .iter()
            .any(|reservation_id| reservation_id == "accepted-expired"),
        "accepted-connection maintenance must delete newly eligible completed sessions"
    );
    assert!(
        observed_active_session_reservation_ids(&observation_runtime, &observation_state)
            .iter()
            .any(|reservation_id| reservation_id == "startup-exact-cutoff"),
        "accepted-connection maintenance must preserve exact-cutoff sessions"
    );

    upstream_thread
        .join()
        .expect("upstream thread should not panic");
    let persisted = observation_runtime.block_on(async {
        observation_state
            .load_session_account_affinity(Provider::Openai, "assembled-http-session")
            .await
            .expect("session affinity should load")
    });
    assert_eq!(
        persisted,
        Some(SessionAccountAffinity::new(
            codex_router_core::provider::Provider::Openai,
            "assembled-http-session",
            alpha.account_id().clone(),
            RETENTION_NOW,
        ))
    );
}

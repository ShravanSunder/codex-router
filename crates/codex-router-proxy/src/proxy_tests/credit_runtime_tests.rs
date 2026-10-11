use super::*;

#[test]
fn assembled_loopback_http_routes_known_exhaustion_to_opted_in_credit_account() {
    let temp_dir = ProxyTestTempDir::new("assembled_runtime_http_credit_backing");
    let database_path = temp_dir.path().join("state.sqlite");
    let secret_path = temp_dir.path().join("secrets");
    let state = match SqliteStateStore::open(&database_path) {
        Ok(state) => state,
        Err(error) => panic!("state store should open: {error}"),
    };
    let secrets = match codex_router_secret_store::test_support::open_encrypted_credential_store(
        &secret_path,
    ) {
        Ok(secrets) => secrets,
        Err(error) => panic!("secret store should open: {error}"),
    };
    let included_exhausted = AccountRecord::new(
        Provider::Openai,
        account_id("acct_included_exhausted"),
        "included-exhausted",
        AccountStatus::Enabled,
    );
    let credit_backed = AccountRecord::new(
        Provider::Openai,
        account_id("acct_credit_backed_http"),
        "credit-backed-http",
        AccountStatus::Enabled,
    );
    persist_account_with_snapshot_and_token(
        &state,
        &secrets,
        &included_exhausted,
        0,
        "included-exhausted-token",
    );
    persist_credit_backed_account_with_token(
        &database_path,
        &secrets,
        &credit_backed,
        "credit-backed-token",
        1_030,
        true,
    );

    let upstream_listener = TcpListener::bind("127.0.0.1:0")
        .unwrap_or_else(|error| panic!("credit mock upstream should bind: {error}"));
    let upstream_address = upstream_listener
        .local_addr()
        .unwrap_or_else(|error| panic!("credit mock upstream address should read: {error}"));
    let (authorization_sender, authorization_receiver) = mpsc::channel();
    let upstream_thread = thread::spawn(move || {
        let (mut stream, _peer_address) = upstream_listener
            .accept()
            .unwrap_or_else(|error| panic!("credit upstream should receive one request: {error}"));
        let request = read_test_http_request(&mut stream);
        let authorization = request
            .lines()
            .find(|line| line.starts_with("authorization: "))
            .unwrap_or("<missing>")
            .to_owned();
        authorization_sender
            .send(authorization)
            .unwrap_or_else(|error| panic!("credit authorization should record: {error}"));
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
            .unwrap_or_else(|error| panic!("credit upstream response should write: {error}"));
    });
    let endpoint = UpstreamEndpoint::new(format!("http://{upstream_address}/v1"))
        .unwrap_or_else(|error| panic!("credit upstream endpoint should validate: {error}"));
    let bind_address = LoopbackBindAddress::new("127.0.0.1", 0)
        .unwrap_or_else(|error| panic!("router bind address should validate: {error}"));
    let config = LoopbackRouterRuntimeConfig::new(
        bind_address,
        endpoint,
        database_path,
        secret_path,
        LocalRouterTokenRecord::new(SecretString::new("current-token"), TokenGeneration::new(1)),
    )
    .with_quota_clock(1_030, 60);
    let runtime = LoopbackRouterRuntime::start(config, secrets.into())
        .unwrap_or_else(|error| panic!("credit proxy runtime should start: {error}"));
    let router_address = runtime.local_addr();
    let client_thread = thread::spawn(move || {
        send_loopback_request(
            router_address,
            "POST /v1/responses HTTP/1.1\r\n",
            br#"{"model":"gpt-5","credit_backed":true}"#,
        )
    });
    assert_eq!(
        runtime
            .serve_http_connections(1)
            .unwrap_or_else(|error| panic!("credit runtime should serve request: {error}")),
        1
    );
    let response = client_thread
        .join()
        .unwrap_or_else(|error| panic!("credit client should finish: {error:?}"));
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(response.ends_with("\r\n\r\nok"), "{response}");
    assert_eq!(
        authorization_receiver
            .recv()
            .unwrap_or_else(|error| panic!("selected credit authorization should arrive: {error}")),
        "authorization: Bearer credit-backed-token"
    );
    upstream_thread
        .join()
        .unwrap_or_else(|error| panic!("credit upstream should finish: {error:?}"));
}

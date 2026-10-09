use super::*;

#[test]
fn loopback_server_binds_ephemeral_tcp_listener_on_loopback() {
    let address = match LoopbackBindAddress::new("127.0.0.1", 0) {
        Ok(address) => address,
        Err(error) => panic!("loopback address should validate: {error}"),
    };
    let runtime = match LoopbackServerRuntime::bind(address) {
        Ok(runtime) => runtime,
        Err(error) => panic!("loopback bind should succeed: {error}"),
    };

    assert!(runtime.local_addr().ip().is_loopback());
    assert_ne!(runtime.local_addr().port(), 0);
}

#[tokio::test]
async fn async_loopback_server_adopts_granted_listener_and_stops_on_cancellation() {
    let address = match LoopbackBindAddress::new("127.0.0.1", 0) {
        Ok(address) => address,
        Err(error) => panic!("loopback address should validate: {error}"),
    };
    let gate = codex_router_descriptor_boundary::DescriptorGate::global();
    let listener =
        codex_router_descriptor_boundary::OwnedListener::bind_tcp(address.socket_addr(), gate)
            .await
            .expect("real owned listening socket");
    let granted_address = listener.tcp_address().expect("actual kernel address");
    let runtime = match AsyncLoopbackServerRuntime::from_granted(listener, address, gate).await {
        Ok(runtime) => runtime,
        Err(error) => panic!("async loopback bind should succeed: {error}"),
    };
    let local_addr = runtime.local_addr();
    assert_eq!(local_addr, granted_address);
    let shutdown = tokio_util::sync::CancellationToken::new();
    let shutdown_for_task = shutdown.clone();
    let serve_task =
        tokio::spawn(async move { runtime.serve_until_cancelled(shutdown_for_task).await });

    shutdown.cancel();
    let handled = match serve_task.await {
        Ok(result) => match result {
            Ok(handled) => handled,
            Err(error) => panic!("async runtime should shut down cleanly: {error}"),
        },
        Err(error) => panic!("async runtime task should join: {error}"),
    };

    assert!(local_addr.ip().is_loopback());
    assert_ne!(local_addr.port(), 0);
    assert_eq!(handled, 0);
}

#[test]
fn hyper_protocol_switchpoint_routes_websocket_upgrade_without_body_buffering() {
    let mut headers = http::HeaderMap::new();
    headers.insert(
        http::header::UPGRADE,
        http::HeaderValue::from_static("websocket"),
    );
    headers.insert(
        http::header::CONNECTION,
        http::HeaderValue::from_static("Upgrade"),
    );

    let dispatch = HyperProtocolSwitchpoint::classify(
        &http::Method::POST,
        &must_ok("/v1/responses".parse::<http::Uri>()),
        &headers,
    );

    assert_eq!(dispatch, HyperProtocolDispatch::WebSocketUpgrade);
}

#[test]
fn hyper_protocol_switchpoint_routes_http_without_upgrade() {
    let dispatch = HyperProtocolSwitchpoint::classify(
        &http::Method::POST,
        &must_ok("/v1/responses".parse::<http::Uri>()),
        &http::HeaderMap::new(),
    );

    assert_eq!(dispatch, HyperProtocolDispatch::Http);
}

#[test]
fn hyper_websocket_upgrade_uses_hyper_tungstenite_response_builder() {
    let mut request = match http::Request::builder()
        .method(http::Method::GET)
        .uri("/v1/responses")
        .header(http::header::CONNECTION, "Upgrade")
        .header(http::header::UPGRADE, "websocket")
        .header(http::header::SEC_WEBSOCKET_VERSION, "13")
        .header(http::header::SEC_WEBSOCKET_KEY, "dGhlIHNhbXBsZSBub25jZQ==")
        .body(http_body_util::Full::new(bytes::Bytes::new()))
    {
        Ok(request) => request,
        Err(error) => panic!("test request should build: {error}"),
    };

    let (response, _websocket) = match hyper_tungstenite::upgrade(&mut request, None) {
        Ok(upgrade) => upgrade,
        Err(error) => panic!("hyper upgrade response should build: {error}"),
    };

    assert_eq!(response.status(), http::StatusCode::SWITCHING_PROTOCOLS);
    assert_eq!(
        response.headers().get(http::header::SEC_WEBSOCKET_ACCEPT),
        Some(&http::HeaderValue::from_static(
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        )),
    );
    assert_eq!(
        response.headers().get(http::header::CONNECTION),
        Some(&http::HeaderValue::from_static("upgrade")),
    );
    assert_eq!(
        response.headers().get(http::header::UPGRADE),
        Some(&http::HeaderValue::from_static("websocket")),
    );
}

#[test]
fn hyper_websocket_upgrade_rejects_missing_key() {
    let mut request = match http::Request::builder()
        .method(http::Method::GET)
        .uri("/v1/responses")
        .header(http::header::CONNECTION, "Upgrade")
        .header(http::header::UPGRADE, "websocket")
        .header(http::header::SEC_WEBSOCKET_VERSION, "13")
        .body(http_body_util::Full::new(bytes::Bytes::new()))
    {
        Ok(request) => request,
        Err(error) => panic!("test request should build: {error}"),
    };

    match hyper_tungstenite::upgrade(&mut request, None) {
        Ok(response) => panic!("missing key should fail, got response {response:?}"),
        Err(error) => assert!(matches!(
            error,
            hyper_tungstenite::tungstenite::error::ProtocolError::MissingSecWebSocketKey
        )),
    }
}

#[test]
fn loopback_server_accepts_localhost_and_ipv6_loopback() {
    let localhost = match LoopbackBindAddress::new("localhost", 0) {
        Ok(address) => address,
        Err(error) => panic!("localhost should validate: {error}"),
    };
    let ipv6_loopback = match LoopbackBindAddress::new("::1", 0) {
        Ok(address) => address,
        Err(error) => panic!("IPv6 loopback should validate: {error}"),
    };

    assert!(localhost.socket_addr().ip().is_loopback());
    assert!(ipv6_loopback.socket_addr().ip().is_loopback());
}

#[test]
fn loopback_server_rejects_non_loopback_before_binding() {
    assert_eq!(
        LoopbackBindAddress::new("0.0.0.0", 8787),
        Err(ServerBindError::NonLoopback {
            host: "0.0.0.0".to_owned()
        })
    );
    assert_eq!(
        LoopbackBindAddress::new("::", 8787),
        Err(ServerBindError::NonLoopback {
            host: "::".to_owned()
        })
    );
    assert_eq!(
        LoopbackBindAddress::new("192.168.1.10", 8787),
        Err(ServerBindError::NonLoopback {
            host: "192.168.1.10".to_owned()
        })
    );
}

#[test]
fn loopback_http_adapter_forwards_real_tcp_request_and_serializes_response() {
    let address = match LoopbackBindAddress::new("127.0.0.1", 0) {
        Ok(address) => address,
        Err(error) => panic!("loopback address should validate: {error}"),
    };
    let runtime = match LoopbackServerRuntime::bind(address) {
        Ok(runtime) => runtime,
        Err(error) => panic!("loopback bind should succeed: {error}"),
    };
    let listener = match runtime.listener().try_clone() {
        Ok(listener) => listener,
        Err(error) => panic!("listener clone should succeed: {error}"),
    };
    let (request_sender, request_receiver) = mpsc::channel();
    let server_thread = thread::spawn(move || {
        let (stream, _peer_address) = match listener.accept() {
            Ok(accepted) => accepted,
            Err(error) => panic!("server should accept one client: {error}"),
        };
        let upstream = ChannelUpstream::new(
            request_sender,
            HttpProxyResponse::new(
                200,
                HeaderCollection::new(vec![Header::new("Content-Type", "text/event-stream")]),
                b"data: ok\n\n".to_vec(),
            ),
        );
        let selector = RecordingSelector::new();
        let resolver = RecordingProviderCredentialResolver::new("selected-upstream-token");
        let auth_gate = local_auth_gate();
        let service =
            AuthenticatedHttpProxyService::new(&auth_gate, &selector, &resolver, &upstream)
                .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER);

        match LoopbackHttpAdapter::handle_connection(stream, &service) {
            Ok(()) => {}
            Err(error) => panic!("connection should be handled: {error}"),
        }
    });

    let mut client = match TcpStream::connect(runtime.local_addr()) {
        Ok(client) => client,
        Err(error) => panic!("client should connect to loopback listener: {error}"),
    };
    let request = concat!(
        "POST /v1/responses?stream=true HTTP/1.1\r\n",
        "Host: 127.0.0.1\r\n",
        "X-Codex-Router-Token: current-token\r\n",
        "Authorization: Bearer current-token\r\n",
        "Accept: text/event-stream\r\n",
        "Content-Length: 17\r\n",
        "\r\n",
        "{\"model\":\"gpt-5\"}"
    );
    if let Err(error) = client.write_all(request.as_bytes()) {
        panic!("client request write should succeed: {error}");
    }
    if let Err(error) = client.shutdown(Shutdown::Write) {
        panic!("client write shutdown should succeed: {error}");
    }
    let mut response = String::new();
    if let Err(error) = client.read_to_string(&mut response) {
        panic!("client response read should succeed: {error}");
    }

    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(response.contains("content-type: text/event-stream\r\n"));
    assert!(response.ends_with("\r\ndata: ok\n\n"));

    let recorded = match request_receiver.recv() {
        Ok(request) => request,
        Err(error) => panic!("upstream request should be recorded: {error}"),
    };
    assert_eq!(recorded.method(), Method::Post);
    assert_eq!(recorded.path(), "/v1/responses?stream=true");
    assert_eq!(recorded.body(), br#"{"model":"gpt-5"}"#);
    assert_eq!(
        recorded.headers().values("authorization"),
        vec!["Bearer selected-upstream-token"]
    );
    assert_eq!(recorded.headers().value("x-codex-router-token"), None);
    assert_eq!(
        recorded.headers().value("accept"),
        Some("text/event-stream")
    );

    match server_thread.join() {
        Ok(()) => {}
        Err(error) => panic!("server thread panicked: {error:?}"),
    }
}

#[test]
fn loopback_http_adapter_returns_status_for_post_auth_proxy_rejections() {
    let selection_response = http_response_from_one_connection(|stream| {
        let upstream = RecordingUpstream::new(HttpProxyResponse::new(
            200,
            HeaderCollection::default(),
            b"should-not-send".to_vec(),
        ));
        let selector = RejectingSelector::new(QuotaAwareAccountSelectorError::NoEligibleAccounts);
        let resolver = RecordingProviderCredentialResolver::new("selected-upstream-token");
        let auth_gate = local_auth_gate();
        let service =
            AuthenticatedHttpProxyService::new(&auth_gate, &selector, &resolver, &upstream)
                .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER);
        must_ok(LoopbackHttpAdapter::handle_connection(stream, &service));
    });
    assert!(selection_response.starts_with("HTTP/1.1 503 Service Unavailable\r\n"));

    let credential_response = http_response_from_one_connection(|stream| {
        let upstream = RecordingUpstream::new(HttpProxyResponse::new(
            200,
            HeaderCollection::default(),
            b"should-not-send".to_vec(),
        ));
        let selector = RecordingSelector::new();
        let resolver =
            RejectingProviderCredentialResolver::new(CredentialResolverError::RefreshUnavailable);
        let auth_gate = local_auth_gate();
        let service =
            AuthenticatedHttpProxyService::new(&auth_gate, &selector, &resolver, &upstream)
                .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER);
        must_ok(LoopbackHttpAdapter::handle_connection(stream, &service));
    });
    assert!(credential_response.starts_with("HTTP/1.1 502 Bad Gateway\r\n"));
}

#[test]
fn loopback_http_streaming_adapter_returns_status_for_post_auth_proxy_rejections() {
    let selection_response = http_response_from_one_connection(|stream| {
        let upstream = RecordingUpstream::new(HttpProxyResponse::new(
            200,
            HeaderCollection::default(),
            b"should-not-send".to_vec(),
        ));
        let selector = RejectingSelector::new(QuotaAwareAccountSelectorError::NoEligibleAccounts);
        let resolver = RecordingProviderCredentialResolver::new("selected-upstream-token");
        let auth_gate = local_auth_gate();
        let service =
            AuthenticatedHttpProxyService::new(&auth_gate, &selector, &resolver, &upstream)
                .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER);
        must_ok(LoopbackHttpAdapter::handle_streaming_connection(
            stream, &service,
        ));
    });
    assert!(selection_response.starts_with("HTTP/1.1 503 Service Unavailable\r\n"));

    let credential_response = http_response_from_one_connection(|stream| {
        let upstream = RecordingUpstream::new(HttpProxyResponse::new(
            200,
            HeaderCollection::default(),
            b"should-not-send".to_vec(),
        ));
        let selector = RecordingSelector::new();
        let resolver =
            RejectingProviderCredentialResolver::new(CredentialResolverError::RefreshUnavailable);
        let auth_gate = local_auth_gate();
        let service =
            AuthenticatedHttpProxyService::new(&auth_gate, &selector, &resolver, &upstream)
                .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER);
        must_ok(LoopbackHttpAdapter::handle_streaming_connection(
            stream, &service,
        ));
    });
    assert!(credential_response.starts_with("HTTP/1.1 502 Bad Gateway\r\n"));
}

#[test]
fn loopback_http_adapter_responds_without_client_write_shutdown() {
    let address = match LoopbackBindAddress::new("127.0.0.1", 0) {
        Ok(address) => address,
        Err(error) => panic!("loopback address should validate: {error}"),
    };
    let runtime = match LoopbackServerRuntime::bind(address) {
        Ok(runtime) => runtime,
        Err(error) => panic!("loopback bind should succeed: {error}"),
    };
    let listener = match runtime.listener().try_clone() {
        Ok(listener) => listener,
        Err(error) => panic!("listener clone should succeed: {error}"),
    };
    let server_thread = thread::spawn(move || {
        let (stream, _peer_address) = match listener.accept() {
            Ok(accepted) => accepted,
            Err(error) => panic!("server should accept one client: {error}"),
        };
        let (request_sender, _request_receiver) = mpsc::channel();
        let upstream = ChannelUpstream::new(
            request_sender,
            HttpProxyResponse::new(200, HeaderCollection::default(), b"ok".to_vec()),
        );
        let selector = RecordingSelector::new();
        let resolver = RecordingProviderCredentialResolver::new("selected-upstream-token");
        let auth_gate = local_auth_gate();
        let service =
            AuthenticatedHttpProxyService::new(&auth_gate, &selector, &resolver, &upstream)
                .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER);

        match LoopbackHttpAdapter::handle_connection(stream, &service) {
            Ok(()) => {}
            Err(error) => panic!("connection should be handled: {error}"),
        }
    });

    let mut client = match TcpStream::connect(runtime.local_addr()) {
        Ok(client) => client,
        Err(error) => panic!("client should connect to loopback listener: {error}"),
    };
    if let Err(error) = client.set_read_timeout(Some(Duration::from_millis(250))) {
        panic!("client read timeout should be set: {error}");
    }
    let request = concat!(
        "POST /v1/responses HTTP/1.1\r\n",
        "Host: 127.0.0.1\r\n",
        "X-Codex-Router-Token: current-token\r\n",
        "Content-Length: 17\r\n",
        "\r\n",
        "{\"model\":\"gpt-5\"}"
    );
    if let Err(error) = client.write_all(request.as_bytes()) {
        panic!("client request write should succeed: {error}");
    }
    let mut response = String::new();
    let read_result = client.read_to_string(&mut response);
    drop(client);

    match server_thread.join() {
        Ok(()) => {}
        Err(error) => panic!("server thread panicked: {error:?}"),
    }
    match read_result {
        Ok(_) => {}
        Err(error) => panic!("client should receive response without write shutdown: {error}"),
    }
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(response.ends_with("\r\nok"));
}

#[test]
fn loopback_http_server_accepts_multiple_connections_until_bound_is_reached() {
    let address = match LoopbackBindAddress::new("127.0.0.1", 0) {
        Ok(address) => address,
        Err(error) => panic!("loopback address should validate: {error}"),
    };
    let runtime = match LoopbackServerRuntime::bind(address) {
        Ok(runtime) => runtime,
        Err(error) => panic!("loopback bind should succeed: {error}"),
    };
    let listener = match runtime.listener().try_clone() {
        Ok(listener) => listener,
        Err(error) => panic!("listener clone should succeed: {error}"),
    };
    let server_address = runtime.local_addr();
    let (request_sender, request_receiver) = mpsc::channel();
    let server_thread = thread::spawn(move || {
        let upstream = ChannelUpstream::new(
            request_sender,
            HttpProxyResponse::new(200, HeaderCollection::default(), b"ok".to_vec()),
        );
        let selector = RecordingSelector::new();
        let resolver = RecordingProviderCredentialResolver::new("selected-upstream-token");
        let auth_gate = local_auth_gate();
        let service =
            AuthenticatedHttpProxyService::new(&auth_gate, &selector, &resolver, &upstream)
                .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER);

        match LoopbackHttpServer::serve_connections(listener, &service, 2) {
            Ok(handled) => handled,
            Err(error) => panic!("server should handle bounded connections: {error}"),
        }
    });

    let first_response = send_loopback_request(
        server_address,
        "POST /v1/responses?turn=1 HTTP/1.1\r\n",
        br#"{"model":"gpt-5","turn":1}"#,
    );
    let second_response = send_loopback_request(
        server_address,
        "POST /v1/responses?turn=2 HTTP/1.1\r\n",
        br#"{"model":"gpt-5","turn":2}"#,
    );

    assert!(first_response.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(second_response.starts_with("HTTP/1.1 200 OK\r\n"));

    let first_recorded = match request_receiver.recv() {
        Ok(request) => request,
        Err(error) => panic!("first upstream request should be recorded: {error}"),
    };
    let second_recorded = match request_receiver.recv() {
        Ok(request) => request,
        Err(error) => panic!("second upstream request should be recorded: {error}"),
    };
    assert_eq!(first_recorded.path(), "/v1/responses?turn=1");
    assert_eq!(second_recorded.path(), "/v1/responses?turn=2");

    match server_thread.join() {
        Ok(handled) => assert_eq!(handled, 2),
        Err(error) => panic!("server thread panicked: {error:?}"),
    }
}

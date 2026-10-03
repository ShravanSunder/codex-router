use super::*;

#[test]
fn http_upstream_transport_forwards_real_request_to_mock_server() {
    let listener = match TcpListener::bind("127.0.0.1:0") {
        Ok(listener) => listener,
        Err(error) => panic!("mock upstream should bind: {error}"),
    };
    let server_address = match listener.local_addr() {
        Ok(address) => address,
        Err(error) => panic!("mock upstream address should be readable: {error}"),
    };
    let (request_sender, request_receiver) = mpsc::channel();
    let server_thread = thread::spawn(move || {
        let (mut stream, _peer_address) = match listener.accept() {
            Ok(connection) => connection,
            Err(error) => panic!("mock upstream should accept one connection: {error}"),
        };
        let request = read_test_http_request(&mut stream);
        if let Err(error) = request_sender.send(request) {
            panic!("mock upstream request should record: {error}");
        }
        if let Err(error) = stream.write_all(
                b"HTTP/1.1 201 Created\r\nETag: upstream-etag\r\nContent-Length: 16\r\n\r\n{\"ok\":true}\nrest",
            ) {
                panic!("mock upstream should write response: {error}");
            }
    });
    let endpoint = match UpstreamEndpoint::new(format!("http://{server_address}/v1")) {
        Ok(endpoint) => endpoint,
        Err(error) => panic!("mock endpoint should validate: {error}"),
    };
    let upstream = HttpUpstreamTransport::new(endpoint);
    let service = HttpProxyService::new(&upstream);

    let response = match service.handle(
        HttpProxyRequest::new(Method::Post, "/v1/responses?stream=true")
            .with_header(Header::new("X-Codex-Router-Token", "local-token"))
            .with_header(Header::new("Authorization", "Bearer wrong"))
            .with_header(Header::new("OpenAI-Beta", "responses=v1"))
            .with_body(br#"{"model":"gpt-5"}"#.to_vec()),
        SecretString::new("selected-upstream-token"),
        None,
    ) {
        Ok(response) => response,
        Err(error) => panic!("HTTP upstream transport should forward request: {error}"),
    };

    assert_eq!(response.status(), 201);
    assert_eq!(response.headers().value("etag"), Some("upstream-etag"));
    assert_eq!(response.body(), b"{\"ok\":true}\nrest");
    let recorded_request = match request_receiver.recv() {
        Ok(request) => request,
        Err(error) => panic!("mock upstream request should be recorded: {error}"),
    };
    assert!(recorded_request.starts_with("POST /v1/responses?stream=true HTTP/1.1\r\n"));
    assert!(recorded_request.contains("authorization: Bearer selected-upstream-token\r\n"));
    assert!(recorded_request.contains("openai-beta: responses=v1\r\n"));
    assert!(!recorded_request.contains("X-Codex-Router-Token"));
    assert!(!recorded_request.contains("Bearer wrong"));

    match server_thread.join() {
        Ok(()) => {}
        Err(error) => panic!("mock upstream thread panicked: {error:?}"),
    }
}

#[test]
fn http_upstream_transport_accepts_https_endpoints_at_send_time() {
    let endpoint = match UpstreamEndpoint::new("https://127.0.0.1:1/v1") {
        Ok(endpoint) => endpoint,
        Err(error) => panic!("https endpoint should validate: {error}"),
    };
    let upstream = HttpUpstreamTransport::new(endpoint);
    let service = HttpProxyService::new(&upstream);

    let error = match service.handle(
        HttpProxyRequest::new(Method::Get, "/v1/models"),
        SecretString::new("selected-upstream-token"),
        None,
    ) {
        Ok(_response) => panic!("closed local port should not produce a response"),
        Err(error) => error,
    };

    match error {
        HttpProxyError::Upstream { message } => {
            assert_ne!(message, "http upstream transport requires http endpoint");
        }
        other => panic!("expected upstream error, got {other:?}"),
    }
}

#[test]
fn http_proxy_forwards_supported_routes_and_preserves_models_etag() {
    let upstream = RecordingUpstream::new(HttpProxyResponse::new(
        200,
        HeaderCollection::new(vec![Header::new("ETag", "models-etag")]),
        br#"{"object":"list"}"#.to_vec(),
    ));
    let service = HttpProxyService::new(&upstream);
    let response = match service.handle(
        HttpProxyRequest::new(Method::Get, "/v1/models")
            .with_header(Header::new("X-Codex-Router-Token", "local-token"))
            .with_header(Header::new("Authorization", "Bearer wrong"))
            .with_body(Vec::new()),
        SecretString::new("selected-upstream-token"),
        None,
    ) {
        Ok(response) => response,
        Err(error) => panic!("models request should forward: {error}"),
    };

    assert_eq!(response.status(), 200);
    assert_eq!(response.headers().value("etag"), Some("models-etag"));
    assert_eq!(response.body(), br#"{"object":"list"}"#);

    let recorded = upstream.take_recorded();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].method(), Method::Get);
    assert_eq!(recorded[0].path(), "/v1/models");
    assert_eq!(
        recorded[0].headers().values("authorization"),
        vec!["Bearer selected-upstream-token"]
    );
    assert_eq!(recorded[0].headers().value("x-codex-router-token"), None);
}

#[test]
fn http_proxy_preserves_responses_body_bytes_without_interpreting_unknown_fields() {
    let upstream = RecordingUpstream::new(HttpProxyResponse::new(
        200,
        HeaderCollection::default(),
        b"data: kept\n\n".to_vec(),
    ));
    let service = HttpProxyService::new(&upstream);
    let body = br#"{"unknown_codex_field":{"kept":true}}"#.to_vec();
    let response = match service.handle(
        HttpProxyRequest::new(Method::Post, "/v1/responses")
            .with_header(Header::new("Accept", "text/event-stream"))
            .with_body(body.clone()),
        SecretString::new("selected-upstream-token"),
        None,
    ) {
        Ok(response) => response,
        Err(error) => panic!("responses request should forward: {error}"),
    };

    assert_eq!(response.body(), b"data: kept\n\n");
    let recorded = upstream.take_recorded();
    assert_eq!(recorded[0].body(), body.as_slice());
    assert_eq!(
        recorded[0].headers().value("accept"),
        Some("text/event-stream")
    );
}

#[test]
fn http_proxy_resolver_refreshes_expired_access_token_before_upstream_egress() {
    let upstream = RecordingUpstream::new(HttpProxyResponse::new(
        200,
        HeaderCollection::default(),
        b"ok".to_vec(),
    ));
    let selector = RecordingSelector::new();
    let resolver = RecordingProviderCredentialResolver::new("resolved-upstream-token");
    let auth_gate = local_auth_gate();
    let service = AuthenticatedHttpProxyService::new(&auth_gate, &selector, &resolver, &upstream)
        .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER);

    let response = must_ok(
        service.handle_request(
            HttpProxyRequest::new(Method::Post, "/v1/responses")
                .with_header(Header::new("X-Codex-Router-Token", "current-token"))
                .with_header(Header::new("Authorization", "Bearer current-token"))
                .with_body(br#"{"input":"hi"}"#.to_vec()),
        ),
    );

    assert_eq!(response.status(), 200);
    assert_eq!(
        selector.take_recorded(),
        vec![("/v1/responses".to_owned(), TokenGeneration::new(1))]
    );
    assert_eq!(resolver.take_recorded(), vec!["acct_selected".to_owned()]);
    let recorded = upstream.take_recorded();
    assert_eq!(recorded.len(), 1);
    assert_eq!(
        recorded[0].headers().values("authorization"),
        vec!["Bearer resolved-upstream-token"]
    );
    assert_ne!(
        recorded[0].headers().values("authorization"),
        vec!["Bearer stale-token-canary"]
    );
}

#[test]
fn http_proxy_preserves_query_string_after_route_classification() {
    let upstream = RecordingUpstream::new(HttpProxyResponse::new(
        200,
        HeaderCollection::default(),
        b"data: kept\n\n".to_vec(),
    ));
    let service = HttpProxyService::new(&upstream);
    let response = match service.handle(
        HttpProxyRequest::new(Method::Post, "/v1/responses?stream=true&cursor=abc")
            .with_body(br#"{"model":"gpt-5"}"#.to_vec()),
        SecretString::new("selected-upstream-token"),
        None,
    ) {
        Ok(response) => response,
        Err(error) => panic!("responses request with query should forward: {error}"),
    };

    assert_eq!(response.body(), b"data: kept\n\n");
    let recorded = upstream.take_recorded();
    assert_eq!(recorded[0].path(), "/v1/responses?stream=true&cursor=abc");
    assert_eq!(recorded[0].route_kind(), RouteKind::Responses);
}

#[test]
fn http_proxy_rejects_unsupported_paths_before_upstream() {
    let upstream = RecordingUpstream::new(HttpProxyResponse::new(
        200,
        HeaderCollection::default(),
        Vec::new(),
    ));
    let service = HttpProxyService::new(&upstream);
    let error = match service.handle(
        HttpProxyRequest::new(Method::Post, "/v1/realtime").with_body(Vec::new()),
        SecretString::new("selected-upstream-token"),
        None,
    ) {
        Ok(response) => panic!("unsupported path should fail closed: {response:?}"),
        Err(error) => error,
    };

    assert_eq!(
        error,
        HttpProxyError::Rejected {
            reason: "unsupported_path"
        }
    );
    assert!(upstream.take_recorded().is_empty());
}

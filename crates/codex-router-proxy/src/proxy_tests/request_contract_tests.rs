use super::*;

#[test]
fn reports_package_name() {
    assert_eq!(package_name(), "codex-router-proxy");
}

#[test]
fn proxy_auth_gate_rejects_before_selection() {
    let current =
        LocalRouterTokenRecord::new(SecretString::new("current-token"), TokenGeneration::new(1));
    let auth = LocalRouterAuth::new(current, Vec::new());
    let gate = ProxyLocalAuthGate::new(auth);

    assert_eq!(gate.authorize(None), Err(LocalAuthError::Missing));
    assert_eq!(
        gate.authorize(Some("current-token")),
        Ok(TokenGeneration::new(1))
    );
}

#[test]
fn route_classifier_supports_required_codex_routes_and_rejects_realtime() {
    assert_eq!(
        classify_route(Method::Post, "/v1/responses", false),
        RouteClass::Supported(RouteKind::Responses)
    );
    assert_eq!(
        classify_route(Method::Post, "/v1/responses", true),
        RouteClass::Supported(RouteKind::ResponsesWebSocket)
    );
    assert_eq!(
        classify_route(Method::Get, "/v1/models", false),
        RouteClass::Supported(RouteKind::Models)
    );
    assert_eq!(
        classify_route(Method::Post, "/v1/memories/trace_summarize", false),
        RouteClass::Supported(RouteKind::MemoriesTraceSummarize)
    );
    assert_eq!(
        classify_route(Method::Post, "/v1/responses/compact", false),
        RouteClass::Supported(RouteKind::ResponsesCompact)
    );
    for (path, expected_route_kind) in [
        ("/v1/images/generations", RouteKind::ImageGenerations),
        ("/v1/images/edits", RouteKind::ImageEdits),
    ] {
        assert_eq!(
            classify_route(Method::Post, path, false),
            RouteClass::Supported(expected_route_kind)
        );
        assert_eq!(expected_route_kind.route_band(), RouteBand::Responses);
        assert!(!expected_route_kind.previous_response_affinity_capable());
    }
    assert_eq!(
        classify_route(Method::Get, "/v1/realtime", true),
        RouteClass::Rejected {
            reason: "unsupported_path"
        }
    );
}

#[test]
fn upstream_request_strips_local_and_hop_headers_and_injects_auth_once() {
    let request = UpstreamRequestBuilder::new(RouteKind::Responses)
        .with_header(Header::new("X-Codex-Router-Token", "local-token-canary"))
        .with_header(Header::new("Host", "127.0.0.1:8787"))
        .with_header(Header::new("Content-Length", "42"))
        .with_header(Header::new("Connection", "upgrade"))
        .with_header(Header::new("Upgrade", "websocket"))
        .with_header(Header::new("Authorization", "Bearer user-supplied"))
        .with_header(Header::new("ChatGPT-Account-Id", "hostile-account-id"))
        .with_header(Header::new("Cookie", "session=user-cookie"))
        .with_header(Header::new("OpenAI-Beta", "responses=v1"))
        .with_body(br#"{"model":"gpt-5","unknown_codex_field":{"kept":true}}"#.to_vec())
        .build_with_chatgpt_account_id(
            SecretString::new("upstream-account-token"),
            Some("chatgpt-account-id-canary"),
        );

    assert_eq!(request.route_kind(), RouteKind::Responses);
    assert_eq!(
        request.body(),
        br#"{"model":"gpt-5","unknown_codex_field":{"kept":true}}"#
    );
    assert_eq!(request.headers().value("openai-beta"), Some("responses=v1"));
    assert_eq!(
        request.headers().values("authorization"),
        vec!["Bearer upstream-account-token"]
    );
    assert_eq!(
        request.headers().values("chatgpt-account-id"),
        vec!["chatgpt-account-id-canary"]
    );
    assert_eq!(request.headers().value("x-codex-router-token"), None);
    assert_eq!(request.headers().value("host"), None);
    assert_eq!(request.headers().value("content-length"), None);
    assert_eq!(request.headers().value("connection"), None);
    assert_eq!(request.headers().value("upgrade"), None);
    assert_eq!(request.headers().value("cookie"), None);
}

#[test]
fn upstream_endpoint_joins_base_url_with_codex_path_without_losing_query() {
    let endpoint = match UpstreamEndpoint::new("https://api.openai.com/v1") {
        Ok(endpoint) => endpoint,
        Err(error) => panic!("upstream endpoint should validate: {error}"),
    };

    assert_eq!(
        endpoint.url_for_path("/v1/responses?stream=true&cursor=abc"),
        "https://api.openai.com/v1/responses?stream=true&cursor=abc"
    );
    assert_eq!(
        endpoint.url_for_path("v1/models"),
        "https://api.openai.com/v1/models"
    );
}

#[test]
fn upstream_endpoint_maps_chatgpt_backend_api_to_codex_runtime_paths() {
    let endpoint = match UpstreamEndpoint::new("https://chatgpt.com/backend-api") {
        Ok(endpoint) => endpoint,
        Err(error) => panic!("upstream endpoint should validate: {error}"),
    };

    assert_eq!(
        endpoint.url_for_path("/v1/responses?stream=true&cursor=abc"),
        "https://chatgpt.com/backend-api/codex/responses?stream=true&cursor=abc"
    );
    assert_eq!(
        endpoint.url_for_path("/v1/responses/compact"),
        "https://chatgpt.com/backend-api/codex/responses/compact"
    );
    assert_eq!(
        endpoint.url_for_path("/v1/models"),
        "https://chatgpt.com/backend-api/codex/models"
    );
    assert_eq!(
        endpoint.url_for_path("/v1/images/generations?output_format=png"),
        "https://chatgpt.com/backend-api/codex/images/generations?output_format=png"
    );
    assert_eq!(
        endpoint.url_for_path("/v1/images/edits"),
        "https://chatgpt.com/backend-api/codex/images/edits"
    );
    assert_eq!(
        endpoint.websocket_url_for_path("/v1/responses"),
        "wss://chatgpt.com/backend-api/codex/responses"
    );
}

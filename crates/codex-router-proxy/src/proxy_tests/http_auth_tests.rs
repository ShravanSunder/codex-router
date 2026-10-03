use super::*;

#[test]
fn authenticated_http_proxy_rejects_missing_token_before_selection_or_upstream() {
    let upstream = RecordingUpstream::new(HttpProxyResponse::new(
        200,
        HeaderCollection::default(),
        Vec::new(),
    ));
    let selector = RecordingSelector::new();
    let resolver = RecordingProviderCredentialResolver::new("selected-upstream-token");
    let auth_gate = local_auth_gate();
    let service = AuthenticatedHttpProxyService::new(&auth_gate, &selector, &resolver, &upstream)
        .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER);

    let error = match service.handle_request(
        HttpProxyRequest::new(Method::Post, "/v1/responses")
            .with_body(br#"{"model":"gpt-5"}"#.to_vec()),
    ) {
        Ok(response) => panic!("missing token should reject locally: {response:?}"),
        Err(error) => error,
    };

    assert_eq!(
        error,
        HttpProxyError::LocalAuth {
            reason: LocalAuthError::Missing
        }
    );
    assert!(selector.take_recorded().is_empty());
    assert!(upstream.take_recorded().is_empty());
}

#[test]
fn authenticated_http_proxy_requires_affinity_secret_before_response_selection() {
    let upstream = RecordingUpstream::new(HttpProxyResponse::new(
        200,
        HeaderCollection::default(),
        b"should-not-send".to_vec(),
    ));
    let selector = RecordingSelector::new();
    let resolver = RecordingProviderCredentialResolver::new("selected-upstream-token");
    let auth_gate = local_auth_gate();
    let service = AuthenticatedHttpProxyService::new(&auth_gate, &selector, &resolver, &upstream);

    let error = match service.handle_request(
        HttpProxyRequest::new(Method::Post, "/v1/responses")
            .with_header(Header::new("X-Codex-Router-Token", "current-token"))
            .with_body(br#"{"model":"gpt-5"}"#.to_vec()),
    ) {
        Ok(response) => panic!("missing affinity secret provider should reject: {response:?}"),
        Err(error) => error,
    };

    assert_eq!(
        error,
        HttpProxyError::Selection {
            reason: QuotaAwareAccountSelectorError::SecretUnavailable
        }
    );
    assert!(selector.take_recorded().is_empty());
    assert!(resolver.take_recorded().is_empty());
    assert!(upstream.take_recorded().is_empty());
}

#[test]
fn authenticated_http_proxy_selects_after_auth_and_forwards_selected_token() {
    let upstream = RecordingUpstream::new(HttpProxyResponse::new(
        200,
        HeaderCollection::default(),
        b"data: kept\n\n".to_vec(),
    ));
    let selector = RecordingSelector::new();
    let resolver = RecordingProviderCredentialResolver::new("selected-upstream-token");
    let auth_gate = local_auth_gate();
    let service = AuthenticatedHttpProxyService::new(&auth_gate, &selector, &resolver, &upstream)
        .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER);

    let response = match service.handle_request(
        HttpProxyRequest::new(Method::Post, "/v1/responses?stream=true")
            .with_header(Header::new("X-Codex-Router-Token", "current-token"))
            .with_header(Header::new("Authorization", "Bearer current-token"))
            .with_body(br#"{"model":"gpt-5"}"#.to_vec()),
    ) {
        Ok(response) => response,
        Err(error) => panic!("authorized request should forward: {error}"),
    };

    assert_eq!(response.body(), b"data: kept\n\n");
    let selected = selector.take_recorded();
    assert_eq!(
        selected,
        vec![(
            "/v1/responses?stream=true".to_owned(),
            TokenGeneration::new(1)
        )]
    );
    let recorded = upstream.take_recorded();
    assert_eq!(recorded[0].path(), "/v1/responses?stream=true");
    assert_eq!(
        recorded[0].headers().values("authorization"),
        vec!["Bearer selected-upstream-token"]
    );
    assert_eq!(recorded[0].headers().value("x-codex-router-token"), None);
}

#[test]
fn authenticated_http_proxy_records_top_level_response_id_owner() {
    let upstream = RecordingUpstream::new(HttpProxyResponse::new(
        200,
        HeaderCollection::default(),
        br#"{"id":"resp_top_level","output":[{"id":"resp_nested"}]}"#.to_vec(),
    ));
    let selector = RecordingSelector::new();
    let resolver = RecordingProviderCredentialResolver::new("selected-upstream-token");
    let auth_gate = local_auth_gate();
    let recorder = RecordingAffinityOwnerRecorder::default();
    let service = AuthenticatedHttpProxyService::new(&auth_gate, &selector, &resolver, &upstream)
        .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER)
        .with_affinity_owner_recorder(Arc::new(recorder.clone()));

    let response = must_ok(
        service.handle_request(
            HttpProxyRequest::new(Method::Post, "/v1/responses")
                .with_header(Header::new("X-Codex-Router-Token", "current-token"))
                .with_body(br#"{"model":"gpt-5"}"#.to_vec()),
        ),
    );

    assert_eq!(response.status(), 200);
    let records = recorder.take_records();
    assert_eq!(records.len(), 1);
    let owner = &records[0];
    assert_eq!(owner.account_id().as_str(), "acct_selected");
    assert_eq!(owner.credential_generation(), 1);
    assert_eq!(
        owner.route_band(),
        codex_router_core::routes::RouteBand::Responses
    );
    assert_eq!(owner.source_transport(), AffinitySourceTransport::HttpSse);
    let expected_hash = must_ok(hash_previous_response_id(
        &test_affinity_secret(),
        &must_ok(PreviousResponseId::new("resp_top_level")),
    ));
    assert_eq!(owner.affinity_key_hash(), &expected_hash);
    assert_ne!(owner.affinity_key_hash().as_str(), "resp_top_level");
}

#[test]
fn authenticated_http_proxy_ignores_nested_response_ids() {
    let upstream = RecordingUpstream::new(HttpProxyResponse::new(
        200,
        HeaderCollection::default(),
        br#"{"output":[{"id":"resp_nested"}]}"#.to_vec(),
    ));
    let selector = RecordingSelector::new();
    let resolver = RecordingProviderCredentialResolver::new("selected-upstream-token");
    let auth_gate = local_auth_gate();
    let recorder = RecordingAffinityOwnerRecorder::default();
    let service = AuthenticatedHttpProxyService::new(&auth_gate, &selector, &resolver, &upstream)
        .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER)
        .with_affinity_owner_recorder(Arc::new(recorder.clone()));

    let response = must_ok(
        service.handle_request(
            HttpProxyRequest::new(Method::Post, "/v1/responses")
                .with_header(Header::new("X-Codex-Router-Token", "current-token"))
                .with_body(br#"{"model":"gpt-5"}"#.to_vec()),
        ),
    );

    assert_eq!(response.status(), 200);
    assert!(recorder.take_records().is_empty());
}

#[test]
fn authenticated_http_proxy_records_streaming_sse_response_id_after_body_read() {
    let upstream = RecordingUpstream::new(HttpProxyResponse::new(
        200,
        HeaderCollection::new(vec![Header::new("Content-Type", "text/event-stream")]),
        b"data: {\"id\":\"resp_stream\"}\n\ndata: [DONE]\n\n".to_vec(),
    ));
    let selector = RecordingSelector::new();
    let resolver = RecordingProviderCredentialResolver::new("selected-upstream-token");
    let auth_gate = local_auth_gate();
    let recorder = RecordingAffinityOwnerRecorder::default();
    let service = AuthenticatedHttpProxyService::new(&auth_gate, &selector, &resolver, &upstream)
        .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER)
        .with_affinity_owner_recorder(Arc::new(recorder.clone()));

    let mut response = must_ok(
        service.handle_streaming_request(
            HttpProxyRequest::new(Method::Post, "/v1/responses")
                .with_header(Header::new("X-Codex-Router-Token", "current-token"))
                .with_body(br#"{"model":"gpt-5","stream":true}"#.to_vec()),
        ),
    );
    assert!(recorder.take_records().is_empty());
    let mut body = Vec::new();
    must_ok(response.body_mut().read_to_end(&mut body));

    assert_eq!(body, b"data: {\"id\":\"resp_stream\"}\n\ndata: [DONE]\n\n");
    let records = recorder.take_records();
    assert_eq!(records.len(), 1);
    let expected_hash = must_ok(hash_previous_response_id(
        &test_affinity_secret(),
        &must_ok(PreviousResponseId::new("resp_stream")),
    ));
    assert_eq!(records[0].affinity_key_hash(), &expected_hash);
}

#[test]
fn authenticated_http_proxy_accepts_codex_env_key_authorization_bearer() {
    let upstream = RecordingUpstream::new(HttpProxyResponse::new(
        200,
        HeaderCollection::default(),
        b"data: kept\n\n".to_vec(),
    ));
    let selector = RecordingSelector::new();
    let resolver = RecordingProviderCredentialResolver::new("selected-upstream-token");
    let auth_gate = local_auth_gate();
    let service = AuthenticatedHttpProxyService::new(&auth_gate, &selector, &resolver, &upstream)
        .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER);

    let response = match service.handle_request(
        HttpProxyRequest::new(Method::Post, "/v1/responses")
            .with_header(Header::new("Authorization", "Bearer current-token"))
            .with_body(br#"{"model":"gpt-5"}"#.to_vec()),
    ) {
        Ok(response) => response,
        Err(error) => panic!("authorization bearer should satisfy local auth: {error}"),
    };

    assert_eq!(response.body(), b"data: kept\n\n");
    assert_eq!(
        selector.take_recorded(),
        vec![("/v1/responses".to_owned(), TokenGeneration::new(1))]
    );
    let recorded = upstream.take_recorded();
    assert_eq!(
        recorded[0].headers().values("authorization"),
        vec!["Bearer selected-upstream-token"]
    );
    assert!(
        !recorded[0]
            .headers()
            .values("authorization")
            .contains(&"Bearer current-token")
    );
}

#[test]
fn authenticated_http_proxy_accepts_equal_mixed_local_auth_carriers() {
    let upstream = RecordingUpstream::new(HttpProxyResponse::new(
        200,
        HeaderCollection::default(),
        b"data: kept\n\n".to_vec(),
    ));
    let selector = RecordingSelector::new();
    let resolver = RecordingProviderCredentialResolver::new("selected-upstream-token");
    let auth_gate = local_auth_gate();
    let service = AuthenticatedHttpProxyService::new(&auth_gate, &selector, &resolver, &upstream)
        .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER);

    let response = match service.handle_request(
        HttpProxyRequest::new(Method::Post, "/v1/responses")
            .with_header(Header::new("X-Codex-Router-Token", "current-token"))
            .with_header(Header::new("Authorization", "Bearer current-token"))
            .with_body(br#"{"model":"gpt-5"}"#.to_vec()),
    ) {
        Ok(response) => response,
        Err(error) => panic!("equal local auth carriers should satisfy auth: {error}"),
    };

    assert_eq!(response.body(), b"data: kept\n\n");
    assert_eq!(
        selector.take_recorded(),
        vec![("/v1/responses".to_owned(), TokenGeneration::new(1))]
    );
}

#[test]
fn authenticated_http_proxy_rejects_forbidden_local_auth_carriers_before_selection() {
    let cases = [
        HttpProxyRequest::new(Method::Post, "/v1/responses")
            .with_header(Header::new("X-Codex-Router-Token", "current-token"))
            .with_header(Header::new("Authorization", "Bearer wrong"))
            .with_body(br#"{"model":"gpt-5"}"#.to_vec()),
        HttpProxyRequest::new(Method::Post, "/v1/responses?token=current-token")
            .with_header(Header::new("X-Codex-Router-Token", "current-token"))
            .with_body(br#"{"model":"gpt-5"}"#.to_vec()),
        HttpProxyRequest::new(Method::Post, "/v1/responses")
            .with_header(Header::new("X-Codex-Router-Token", "current-token"))
            .with_header(Header::new("Cookie", "router-token=current-token"))
            .with_body(br#"{"model":"gpt-5"}"#.to_vec()),
        HttpProxyRequest::new(Method::Post, "/v1/responses")
            .with_header(Header::new("X-Codex-Router-Token", "current-token"))
            .with_body(br#"{"model":"gpt-5","x-codex-router-token":"current-token"}"#.to_vec()),
    ];

    for request in cases {
        let upstream = RecordingUpstream::new(HttpProxyResponse::new(
            200,
            HeaderCollection::default(),
            b"should-not-send".to_vec(),
        ));
        let selector = RecordingSelector::new();
        let resolver = RecordingProviderCredentialResolver::new("selected-upstream-token");
        let auth_gate = local_auth_gate();
        let service =
            AuthenticatedHttpProxyService::new(&auth_gate, &selector, &resolver, &upstream)
                .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER);

        assert_eq!(
            service.handle_request(request),
            Err(HttpProxyError::LocalAuth {
                reason: LocalAuthError::Wrong
            })
        );
        assert!(selector.take_recorded().is_empty());
        assert!(upstream.take_recorded().is_empty());
    }
}

#[test]
fn authenticated_http_proxy_allows_nested_local_auth_body_canaries() {
    let upstream = RecordingUpstream::new(HttpProxyResponse::new(
        200,
        HeaderCollection::default(),
        b"data: kept\n\n".to_vec(),
    ));
    let selector = RecordingSelector::new();
    let resolver = RecordingProviderCredentialResolver::new("selected-upstream-token");
    let auth_gate = local_auth_gate();
    let service = AuthenticatedHttpProxyService::new(&auth_gate, &selector, &resolver, &upstream)
        .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER);

    match service.handle_request(
        HttpProxyRequest::new(Method::Post, "/v1/responses")
            .with_header(Header::new("X-Codex-Router-Token", "current-token"))
            .with_body(
                br#"{"model":"gpt-5","input":[{"x-codex-router-token":"nested"}]}"#.to_vec(),
            ),
    ) {
        Ok(_response) => {}
        Err(error) => {
            panic!("nested local auth canary should not be treated as carrier: {error}")
        }
    }

    assert_eq!(
        selector.take_recorded(),
        vec![("/v1/responses".to_owned(), TokenGeneration::new(1))]
    );
}

use super::auth_rejection_fixtures::*;
use super::*;

const ORIGINAL_FRAME: &str =
    r#"{"type":"response.create","model":"gpt-5","input":"exact original websocket frame"}"#;
const COMPLETED_RESPONSE: &str =
    r#"{"type":"response.completed","response":{"id":"resp_request_local_fallback"}}"#;
const ACCEPTED_AUTH_ERROR: &str = r#"{"type":"error","status":401,"error":{"code":"token_revoked","message":"rejected after first-frame send"}}"#;

#[derive(Clone, Copy, Eq, PartialEq)]
enum UpstreamHandshakeScenario {
    RejectThenAccept,
    AcceptThenAuthError,
}

#[allow(clippy::result_large_err)]
fn prove_websocket_recovery(scenario: UpstreamHandshakeScenario) {
    let fixture = AuthRejectionFixture::new("request_local_websocket_recovery");
    let original_credential_metadata = fixture.credential_metadata();
    let listener = TcpListener::bind("127.0.0.1:0").expect("provider should bind");
    let runtime = fixture.start(listener.local_addr().expect("address should read"));
    let router_address = runtime.local_addr();
    let upstream_thread = thread::spawn(move || {
        let mut authorizations = Vec::new();
        if scenario == UpstreamHandshakeScenario::RejectThenAccept {
            let (stream, _) = listener.accept().expect("A should connect");
            let rejected = accept_hdr(stream, |request: &Request, _response: Response| {
                assert_eq!(
                    request.headers().get("chatgpt-account-id"),
                    Some(&HeaderValue::from_static("synthetic-openai-primary")),
                    "A's token and companion account header must match"
                );
                authorizations.push(
                    request
                        .headers()
                        .get("authorization")
                        .expect("A auth should exist")
                        .to_str()
                        .expect("A auth should decode")
                        .to_owned(),
                );
                Err(tokio_tungstenite::tungstenite::http::Response::builder()
                    .status(401)
                    .body(Some(r#"{"error":{"code":"token_revoked","message":"Encountered invalidated oauth token for user, failing request"}}"#.to_owned()))
                    .expect("rejection should build"))
            });
            assert!(
                matches!(rejected, Err(tokio_tungstenite::tungstenite::handshake::HandshakeError::Failure(tokio_tungstenite::tungstenite::Error::Http(response))) if response.status().as_u16() == 401)
            );
        }
        let (stream, _) = listener.accept().expect("accepted provider should connect");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("provider read should be bounded");
        let mut websocket = accept_hdr(stream, |request: &Request, response: Response| {
            let expected_account_header = match scenario {
                UpstreamHandshakeScenario::RejectThenAccept => "synthetic-openai-fallback",
                UpstreamHandshakeScenario::AcceptThenAuthError => "synthetic-openai-primary",
            };
            assert_eq!(
                request.headers().get("chatgpt-account-id"),
                Some(&HeaderValue::from_static(expected_account_header)),
                "accepted token and companion account header must match"
            );
            authorizations.push(
                request
                    .headers()
                    .get("authorization")
                    .expect("accepted auth should exist")
                    .to_str()
                    .expect("accepted auth should decode")
                    .to_owned(),
            );
            Ok(response)
        })
        .expect("provider should upgrade");
        let original_frame = websocket
            .read()
            .expect("provider should receive exactly one initial frame");
        assert_eq!(original_frame.to_string(), ORIGINAL_FRAME);
        let response = match scenario {
            UpstreamHandshakeScenario::RejectThenAccept => COMPLETED_RESPONSE,
            UpstreamHandshakeScenario::AcceptThenAuthError => ACCEPTED_AUTH_ERROR,
        };
        websocket
            .send(Message::text(response))
            .expect("provider response should send");
        let next = websocket.read();
        assert!(
            !matches!(next, Ok(Message::Text(_)) | Ok(Message::Binary(_))),
            "first frame must not repeat: {next:?}"
        );
        let extra_authorization = accept_upstream_before_deadline(&listener).map(|mut stream| {
            let authorization = authorization_from_request(&read_test_http_request(&mut stream));
            write_http_response(&mut stream, 401, b"unexpected extra handshake");
            authorization
        });
        (authorizations, extra_authorization)
    });
    let runtime_thread = thread::spawn(move || runtime.serve_protocol_connections(1));
    let mut request = format!("ws://{router_address}/v1/responses")
        .into_client_request()
        .expect("local request should build");
    request.headers_mut().insert(
        "x-codex-router-token",
        HeaderValue::from_static("current-token"),
    );
    let (mut client, upgrade) = connect(request).expect("local connection should upgrade");
    assert_eq!(upgrade.status().as_u16(), 101);
    if let tokio_tungstenite::tungstenite::stream::MaybeTlsStream::Plain(stream) = client.get_mut()
    {
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("client read should be bounded");
    }
    client
        .send(Message::text(ORIGINAL_FRAME))
        .expect("first frame should send");
    let actual_response = client
        .read()
        .expect("same local socket should receive provider response")
        .to_string();
    assert_eq!(
        actual_response,
        match scenario {
            UpstreamHandshakeScenario::RejectThenAccept => COMPLETED_RESPONSE,
            UpstreamHandshakeScenario::AcceptThenAuthError => ACCEPTED_AUTH_ERROR,
        }
    );
    client.close(None).expect("client should close");
    let (authorizations, extra_authorization) =
        upstream_thread.join().expect("provider should join");
    assert_eq!(
        authorizations,
        match scenario {
            UpstreamHandshakeScenario::RejectThenAccept =>
                vec!["Bearer primary-token", "Bearer fallback-token"],
            UpstreamHandshakeScenario::AcceptThenAuthError => vec!["Bearer primary-token"],
        }
    );
    assert!(
        extra_authorization.is_none(),
        "no additional account handshake is allowed after first-frame send"
    );
    assert_eq!(
        must_ok(runtime_thread.join().expect("runtime should join")),
        1
    );
    assert_eq!(fixture.credential_metadata(), original_credential_metadata);
    let observation_runtime = must_ok(
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build(),
    );
    observation_runtime.block_on(async {
        let state = must_ok(AsyncSqliteStateStore::open(&fixture.database_path).await);
        let counts = must_ok(
            state
                .active_client_counts_for_route_band_read_only("responses", 1_030, 60)
                .await,
        );
        assert_eq!(
            counts
                .iter()
                .map(|count| count.active_clients())
                .sum::<u32>(),
            0,
            "failed and successful socket/load ownership must all release"
        );
        must_ok(state.close().await);
    });
}

#[test]
fn request_local_websocket_token_revoked_retries_original_frame_once_and_releases_load() {
    prove_websocket_recovery(UpstreamHandshakeScenario::RejectThenAccept);
}

#[test]
fn request_local_websocket_accepted_auth_error_is_forwarded_without_replay() {
    prove_websocket_recovery(UpstreamHandshakeScenario::AcceptThenAuthError);
}

#[test]
#[allow(clippy::result_large_err)]
fn request_local_websocket_auth_rejection_preserves_hard_previous_response_owner() {
    let fixture = AuthRejectionFixture::new("request_local_websocket_hard_owner");
    fixture.persist_hard_owner("resp_request_local_ws_owner");
    let listener = TcpListener::bind("127.0.0.1:0").expect("provider should bind");
    let runtime = fixture.start(listener.local_addr().expect("address should read"));
    let router_address = runtime.local_addr();
    let upstream_thread = thread::spawn(move || {
        let (stream, _) = listener.accept().expect("owner should connect");
        let mut owner_authorization = String::new();
        let rejected = accept_hdr(stream, |request: &Request, _response: Response| {
            owner_authorization = request
                .headers()
                .get("authorization")
                .expect("owner auth should exist")
                .to_str()
                .expect("owner auth should decode")
                .to_owned();
            Err(tokio_tungstenite::tungstenite::http::Response::builder()
                .status(401)
                .body(Some("owner rejected".to_owned()))
                .expect("rejection should build"))
        });
        assert!(rejected.is_err());
        let fallback_authorization =
            accept_upstream_before_deadline(&listener).map(|mut stream| {
                let authorization =
                    authorization_from_request(&read_test_http_request(&mut stream));
                write_http_response(&mut stream, 401, b"unsafe hard-owner fallback");
                authorization
            });
        (owner_authorization, fallback_authorization)
    });
    let runtime_thread = thread::spawn(move || runtime.serve_protocol_connections(1));
    let mut request = format!("ws://{router_address}/v1/responses")
        .into_client_request()
        .expect("local request should build");
    request.headers_mut().insert(
        "x-codex-router-token",
        HeaderValue::from_static("current-token"),
    );
    let (mut client, _) = connect(request).expect("local connection should upgrade");
    if let tokio_tungstenite::tungstenite::stream::MaybeTlsStream::Plain(stream) = client.get_mut()
    {
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("client read should be bounded");
    }
    client
        .send(Message::text(
            r#"{"type":"response.create","previous_response_id":"resp_request_local_ws_owner"}"#,
        ))
        .expect("hard continuation should send");
    let response = client.read();
    assert!(
        !matches!(response, Ok(Message::Text(_)) | Ok(Message::Binary(_))),
        "unavailable owner must fail rather than fabricate success: {response:?}"
    );
    drop(client);
    assert_eq!(
        upstream_thread.join().expect("provider should join"),
        ("Bearer primary-token".to_owned(), None)
    );
    let runtime_result = runtime_thread.join().expect("runtime should join");
    assert!(matches!(
        runtime_result,
        Err(crate::server::LoopbackRouterRuntimeError::WebSocket(
            crate::websocket::WebSocketTunnelError::CloseReason(WebSocketCloseReason::Selection {
                reason: QuotaAwareAccountSelectorError::AffinityOwnerUnavailable
            })
        ))
    ));
}

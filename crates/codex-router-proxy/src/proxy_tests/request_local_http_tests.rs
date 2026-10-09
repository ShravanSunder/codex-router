use super::auth_rejection_fixtures::*;
use super::*;

const AUTH_REJECTION: &[u8] = br#"{"error":{"code":"token_revoked","message":"Encountered invalidated oauth token for user, failing request"}}"#;
const QUOTA_REJECTION: &[u8] = br#"{"type":"error","error":{"code":"usage_limit_reached"}}"#;

struct HttpRecoveryScenario {
    request_line: &'static str,
    local_token: &'static str,
    request_body: Vec<u8>,
    upstream_replies: Vec<(u16, &'static [u8])>,
}

struct ObservedHttpRecovery {
    client_response: String,
    upstream_requests: Vec<(String, Vec<u8>)>,
}

fn run_http_recovery_scenario(
    fixture: &AuthRejectionFixture,
    scenario: HttpRecoveryScenario,
) -> ObservedHttpRecovery {
    let original_credential_metadata = fixture.credential_metadata();
    let listener = TcpListener::bind("127.0.0.1:0").expect("synthetic provider should bind");
    listener
        .set_nonblocking(true)
        .expect("async provider listener should be nonblocking");
    let runtime = fixture.start(listener.local_addr().expect("provider address should read"));
    let router_address = runtime.local_addr();
    let provider_shutdown = tokio_util::sync::CancellationToken::new();
    let provider_stop = provider_shutdown.clone();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let provider_requests = Arc::clone(&requests);
    let replies = scenario.upstream_replies;
    let provider_thread = thread::spawn(move || {
        let provider_runtime = must_ok(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build(),
        );
        provider_runtime.block_on(async {
            let listener = tokio::net::TcpListener::from_std(listener)
                .expect("provider listener should register");
            let mut reply_index = 0;
            loop {
                let stream = tokio::select! {
                    biased;
                    () = provider_stop.cancelled() => break,
                    accepted = listener.accept() => accepted.expect("provider should accept").0,
                };
                let mut stream = stream.into_std().expect("provider stream should convert");
                stream
                    .set_nonblocking(false)
                    .expect("provider read should be blocking");
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .expect("provider read should be bounded");
                let request = read_test_http_request(&mut stream);
                let authorization = authorization_from_request(&request);
                let expected_account_header = match authorization.as_str() {
                    "Bearer primary-token" => "synthetic-openai-primary",
                    "Bearer fallback-token" => "synthetic-openai-fallback",
                    "Bearer third-token" => "synthetic-openai-third",
                    _ => panic!("unexpected credential bundle"),
                };
                assert!(
                    request.lines().any(|line| {
                        let Some((name, value)) = line.split_once(':') else {
                            return false;
                        };
                        name.eq_ignore_ascii_case("chatgpt-account-id")
                            && value.trim() == expected_account_header
                    }),
                    "selected token and account header must come from the same synthetic bundle"
                );
                provider_requests
                    .lock()
                    .expect("provider records should lock")
                    .push((
                        authorization_from_request(&request),
                        exact_http_request_body(&request),
                    ));
                let (status, body) = replies
                    .get(reply_index)
                    .copied()
                    .unwrap_or((500, b"unexpected-extra-attempt"));
                write_http_response(&mut stream, status, body);
                reply_index += 1;
            }
        });
    });
    let client_thread = thread::spawn(move || {
        let mut client = TcpStream::connect(router_address).expect("local client should connect");
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("client read should be bounded");
        client
            .set_write_timeout(Some(Duration::from_secs(5)))
            .expect("client write should be bounded");
        write!(client, "{}Host: 127.0.0.1\r\nConnection: close\r\nX-Codex-Router-Token: {}\r\nContent-Length: {}\r\n\r\n", scenario.request_line, scenario.local_token, scenario.request_body.len()).expect("client headers should write");
        client
            .write_all(&scenario.request_body)
            .expect("complete client body should write");
        client
            .shutdown(Shutdown::Write)
            .expect("client write should finish");
        let mut response = String::new();
        client
            .read_to_string(&mut response)
            .expect("client response should finish");
        response
    });
    assert_eq!(must_ok(runtime.serve_http_connections(1)), 1);
    let client_response = client_thread.join().expect("client should join");
    provider_shutdown.cancel();
    provider_thread.join().expect("provider should join");
    assert_eq!(
        fixture.credential_metadata(),
        original_credential_metadata,
        "request-local failover must preserve generations, lifecycle and refresh claims"
    );
    let upstream_requests = requests
        .lock()
        .expect("provider records should lock")
        .clone();
    ObservedHttpRecovery {
        client_response,
        upstream_requests,
    }
}

fn responses_scenario(
    request_body: Vec<u8>,
    replies: Vec<(u16, &'static [u8])>,
) -> HttpRecoveryScenario {
    HttpRecoveryScenario {
        request_line: "POST /v1/responses?transport=fixture HTTP/1.1\r\n",
        local_token: "current-token",
        request_body,
        upstream_replies: replies,
    }
}

#[test]
fn request_local_http_token_revoked_retries_complete_body_without_persistence() {
    let fixture = AuthRejectionFixture::new("request_local_http_complete_body");
    let original_body = format!(
        r#"{{"model":"gpt-5","input":"{}"}}"#,
        "complete-body-".repeat(3_000)
    )
    .into_bytes();
    for _fresh_runtime in 0..2 {
        let observed = run_http_recovery_scenario(
            &fixture,
            responses_scenario(
                original_body.clone(),
                vec![(401, AUTH_REJECTION), (200, b"healthy-account-success")],
            ),
        );
        assert!(observed.client_response.starts_with("HTTP/1.1 200"));
        assert!(
            observed
                .client_response
                .ends_with("healthy-account-success")
        );
        assert!(
            !observed
                .client_response
                .contains("authentication rejection")
        );
        assert_eq!(
            observed.upstream_requests,
            vec![
                ("Bearer primary-token".to_owned(), original_body.clone()),
                ("Bearer fallback-token".to_owned(), original_body.clone())
            ]
        );
        assert!(
            fixture.maintenance().is_none(),
            "this independent frontier must not park credentials"
        );
    }
}

#[test]
fn request_local_http_auth_exhaustion_uses_existing_502_without_quota_signal() {
    let fixture = AuthRejectionFixture::new("request_local_http_auth_exhaustion");
    let observed = run_http_recovery_scenario(
        &fixture,
        responses_scenario(
            br#"{"model":"gpt-5"}"#.to_vec(),
            vec![(401, AUTH_REJECTION), (401, AUTH_REJECTION)],
        ),
    );
    assert!(
        observed.client_response.starts_with("HTTP/1.1 502"),
        "all auth attempts should use credential failure: {}",
        observed.client_response
    );
    assert!(
        !observed.client_response.contains("usage_limit_reached")
            && !observed.client_response.contains("all_accounts_exhausted")
    );
    assert_eq!(
        observed
            .upstream_requests
            .iter()
            .map(|(authorization, _)| authorization.as_str())
            .collect::<Vec<_>>(),
        ["Bearer primary-token", "Bearer fallback-token"]
    );
    assert!(fixture.maintenance().is_none());
}

#[test]
fn request_local_http_token_revoked_accumulates_exclusions_across_two_claims() {
    let fixture = AuthRejectionFixture::new("request_local_http_three_accounts");
    let primary_claim = fixture.claim_successor();
    let fallback_claim = fixture.claim_account_successor(fixture.fallback.account_id());
    let state = must_ok(SqliteStateStore::open(&fixture.database_path));
    let secrets = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store(
            &fixture.secret_path,
        ),
    );
    let healthy = AccountRecord::new(
        Provider::Openai,
        account_id("auth-third-healthy"),
        "third",
        AccountStatus::Enabled,
    );
    persist_account_with_snapshot_and_token(&state, &secrets, &healthy, 70, "third-token");
    let healthy_bundle = AccountCredentialBundle::imported_codex_auth("third-token", None)
        .with_expires_unix_seconds(4_000_000_000)
        .with_chatgpt_account_id("synthetic-openai-third");
    must_ok(secrets.write_secret(
        &must_ok(openai_account_credential_bundle_key(
            healthy.account_id(),
            1,
        )),
        &must_ok(healthy_bundle.to_secret_string()),
    ));
    drop(state);
    let original_body =
        br#"{"model":"gpt-5","input":"same complete three-account request"}"#.to_vec();
    let observed = run_http_recovery_scenario(
        &fixture,
        responses_scenario(
            original_body.clone(),
            vec![
                (401, AUTH_REJECTION),
                (401, AUTH_REJECTION),
                (200, b"third-account-result"),
            ],
        ),
    );
    assert!(observed.client_response.starts_with("HTTP/1.1 200"));
    assert!(observed.client_response.ends_with("third-account-result"));
    assert_eq!(
        observed.upstream_requests,
        vec![
            ("Bearer primary-token".to_owned(), original_body.clone()),
            ("Bearer fallback-token".to_owned(), original_body.clone()),
            ("Bearer third-token".to_owned(), original_body),
        ]
    );
    let metadata = fixture.credential_metadata();
    assert!(metadata.contains(&(
        fixture.primary.with_active_credential_generation(1),
        Some(primary_claim),
    )));
    assert!(metadata.contains(&(
        fixture.fallback.with_active_credential_generation(1),
        Some(fallback_claim),
    )));
}

#[test]
fn request_local_http_incomplete_json_auth_failure_uses_502_without_fallback() {
    let fixture = AuthRejectionFixture::new("request_local_http_incomplete_json");
    let original_body = br#"{"model":"gpt-5","input": "#.to_vec();
    let observed = run_http_recovery_scenario(
        &fixture,
        responses_scenario(original_body.clone(), vec![(401, AUTH_REJECTION)]),
    );
    assert!(observed.client_response.starts_with("HTTP/1.1 502"));
    assert_eq!(
        observed.upstream_requests,
        vec![("Bearer primary-token".to_owned(), original_body)]
    );
}

#[test]
fn request_local_http_accepted_sse_auth_text_is_forwarded_without_replay() {
    let fixture = AuthRejectionFixture::new("request_local_http_accepted_auth_text");
    let original_body = br#"{"model":"gpt-5","stream":true}"#.to_vec();
    let accepted_body = br#"event: response.created
data: {"type":"response.created","response":{"id":"resp_accepted"}}

event: error
data: {"type":"error","status":401,"error":{"code":"token_revoked"}}

"#;
    let observed = run_http_recovery_scenario(
        &fixture,
        responses_scenario(original_body.clone(), vec![(200, accepted_body)]),
    );
    assert!(observed.client_response.starts_with("HTTP/1.1 200"));
    assert!(
        observed
            .client_response
            .contains("content-type: text/event-stream")
    );
    assert!(
        observed
            .client_response
            .ends_with(std::str::from_utf8(accepted_body).expect("SSE fixture should decode"))
    );
    assert_eq!(
        observed.upstream_requests,
        vec![("Bearer primary-token".to_owned(), original_body)]
    );
}

#[test]
fn request_local_http_unreplayable_auth_failure_uses_502_without_fallback() {
    let fixture = AuthRejectionFixture::new("request_local_http_oversize");
    let original_body = format!(
        r#"{{"model":"gpt-5","input":"{}"}}"#,
        "x".repeat(2 * 1024 * 1024)
    )
    .into_bytes();
    let observed = run_http_recovery_scenario(
        &fixture,
        responses_scenario(original_body.clone(), vec![(401, AUTH_REJECTION)]),
    );
    assert!(observed.client_response.starts_with("HTTP/1.1 502"));
    assert_eq!(
        observed.upstream_requests,
        vec![("Bearer primary-token".to_owned(), original_body)]
    );
    assert!(fixture.maintenance().is_none());
}

#[test]
fn request_local_http_hard_owner_failure_never_moves_to_fallback() {
    let fixture = AuthRejectionFixture::new("request_local_http_hard_owner");
    fixture.persist_hard_owner("resp_request_local_owner");
    let original_body =
        br#"{"model":"gpt-5","previous_response_id":"resp_request_local_owner"}"#.to_vec();
    let observed = run_http_recovery_scenario(
        &fixture,
        responses_scenario(original_body.clone(), vec![(401, AUTH_REJECTION)]),
    );
    assert!(
        observed.client_response.starts_with("HTTP/1.1 503"),
        "existing unavailable-owner mapping should remain: {}",
        observed.client_response
    );
    assert_eq!(
        observed.upstream_requests,
        vec![("Bearer primary-token".to_owned(), original_body)]
    );
}

#[test]
fn request_local_http_local_auth_rejection_never_reaches_provider() {
    let fixture = AuthRejectionFixture::new("request_local_http_local_token");
    let mut scenario = responses_scenario(br#"{"model":"gpt-5"}"#.to_vec(), Vec::new());
    scenario.local_token = "invalid-local-token";
    let observed = run_http_recovery_scenario(&fixture, scenario);
    assert!(observed.client_response.starts_with("HTTP/1.1 401"));
    assert!(observed.upstream_requests.is_empty());
}

#[test]
fn request_local_http_models_401_preserves_existing_pass_through() {
    let fixture = AuthRejectionFixture::new("request_local_http_models_characterization");
    let state = must_ok(SqliteStateStore::open(&fixture.database_path));
    for account in [&fixture.primary, &fixture.fallback] {
        persist_account_with_selector_windows(&state, account, &["models"], 80);
    }
    drop(state);
    let observed = run_http_recovery_scenario(
        &fixture,
        HttpRecoveryScenario {
            request_line: "GET /v1/models HTTP/1.1\r\n",
            local_token: "current-token",
            request_body: Vec::new(),
            upstream_replies: vec![(401, AUTH_REJECTION)],
        },
    );
    assert!(
        observed.client_response.starts_with("HTTP/1.1 401"),
        "Models must not inherit Responses auth retry: {}",
        observed.client_response
    );
    assert!(
        observed
            .client_response
            .ends_with(std::str::from_utf8(AUTH_REJECTION).expect("fixture body should be utf8"))
    );
    assert_eq!(observed.upstream_requests.len(), 1);
    assert!(fixture.maintenance().is_none());
}

#[test]
fn request_local_http_auth_then_quota_preserves_true_quota_observation() {
    let fixture = AuthRejectionFixture::new("request_local_http_mixed_quota");
    let observed = run_http_recovery_scenario(
        &fixture,
        responses_scenario(
            br#"{"model":"gpt-5"}"#.to_vec(),
            vec![(401, AUTH_REJECTION), (429, QUOTA_REJECTION)],
        ),
    );
    assert!(
        observed.client_response.starts_with("HTTP/1.1 502"),
        "auth exhaustion must not claim the whole pool has quota exhaustion: {}",
        observed.client_response
    );
    assert_eq!(
        observed
            .upstream_requests
            .iter()
            .map(|(authorization, _)| authorization.as_str())
            .collect::<Vec<_>>(),
        ["Bearer primary-token", "Bearer fallback-token"]
    );
    let state = must_ok(SqliteStateStore::open(&fixture.database_path));
    wait_for_durable_quota_exhaustion(&state, &[fixture.fallback.account_id()]);
    assert!(
        fixture.maintenance().is_none(),
        "true quota observation must not become auth maintenance"
    );
}

#[test]
fn request_local_http_quota_only_exhaustion_preserves_existing_signal() {
    let fixture = AuthRejectionFixture::new("request_local_http_true_quota");
    let observed = run_http_recovery_scenario(
        &fixture,
        responses_scenario(
            br#"{"model":"gpt-5"}"#.to_vec(),
            vec![(429, QUOTA_REJECTION), (429, QUOTA_REJECTION)],
        ),
    );
    assert!(observed.client_response.starts_with("HTTP/1.1 503"));
    assert!(
        observed
            .client_response
            .contains("codex_router_all_accounts_exhausted")
            && observed.client_response.contains("usage_limit_reached")
    );
    assert_eq!(observed.upstream_requests.len(), 2);
    let state = must_ok(SqliteStateStore::open(&fixture.database_path));
    wait_for_durable_quota_exhaustion(
        &state,
        &[fixture.primary.account_id(), fixture.fallback.account_id()],
    );
}

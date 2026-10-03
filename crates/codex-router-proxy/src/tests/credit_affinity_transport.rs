use super::*;

use std::net::SocketAddr;
use std::thread::JoinHandle;

const INCLUDED_PEER_TOKEN: &str = "included-affinity-peer-token";
const CREDIT_AFFINITY_SESSION_ID: &str = "credit-affinity-session";
const CREDIT_PREVIOUS_RESPONSE_ID: &str = "resp_credit_affinity_owner";
const NO_REPLAY_PROBE_PATH: &str = "/__credit_affinity_no_replay_probe";

#[test]
#[allow(clippy::result_large_err)]
fn assembled_loopback_http_session_affinity_yields_credit_owner_to_included_peer() {
    let temp_dir = ProxyTestTempDir::new("credit_affinity_session_yields_to_included_peer");
    let database_path = temp_dir.path().join("state.sqlite");
    let secret_path = temp_dir.path().join("secrets");
    let secrets =
        codex_router_secret_store::test_support::open_encrypted_credential_store(&secret_path)
            .unwrap_or_else(|error| panic!("session-affinity secrets should open: {error}"));
    let quota_observed_at = test_unix_seconds();
    let credit_account = AccountRecord::new(
        Provider::Openai,
        account_id("acct_credit_session_affinity"),
        "credit-session-affinity",
        AccountStatus::Enabled,
    );
    persist_available_credit_owner(&database_path, &secrets, &credit_account, quota_observed_at);
    let state = SqliteStateStore::open(&database_path)
        .unwrap_or_else(|error| panic!("session-affinity state should open: {error}"));

    let included_peer = AccountRecord::new(
        Provider::Openai,
        account_id("acct_included_session_affinity_peer"),
        "included-session-affinity-peer",
        AccountStatus::Enabled,
    );
    let (upstream_address, upstream_requests, upstream_thread) =
        spawn_three_response_http_upstream();
    let mut journey = None;
    let captured_logs = crate::test_log_capture::capture_log_output(|| {
        let runtime = start_credit_affinity_runtime(
            upstream_address,
            &database_path,
            &secret_path,
            secrets.clone(),
            quota_observed_at + 30,
        );
        let router_address = runtime.local_addr();
        let runtime_thread = thread::spawn(move || runtime.serve_http_connections(3));

        let first_client = thread::spawn(move || {
            send_loopback_request_with_session(
                router_address,
                CREDIT_AFFINITY_SESSION_ID,
                br#"{"model":"gpt-5","prompt_cache_key":"credit-affinity"}"#,
            )
        });
        let first_response = first_client
            .join()
            .unwrap_or_else(|error| panic!("first affinity client should finish: {error:?}"));
        let first_upstream = upstream_requests
            .recv_timeout(Duration::from_secs(2))
            .unwrap_or_else(|error| panic!("credit request should reach upstream: {error}"));

        let second_client = thread::spawn(move || {
            send_loopback_request_with_session(
                router_address,
                CREDIT_AFFINITY_SESSION_ID,
                br#"{"model":"gpt-5","prompt_cache_key":"credit-affinity"}"#,
            )
        });
        let second_response = second_client.join().unwrap_or_else(|error| {
            panic!("same-session affinity client should finish: {error:?}")
        });
        let second_upstream = upstream_requests
            .recv_timeout(Duration::from_secs(2))
            .unwrap_or_else(|error| panic!("same-session request should reach upstream: {error}"));

        persist_account_with_snapshot_and_token(
            &state,
            &secrets,
            &included_peer,
            90,
            INCLUDED_PEER_TOKEN,
        );

        let third_client = thread::spawn(move || {
            send_loopback_request_with_session(
                router_address,
                CREDIT_AFFINITY_SESSION_ID,
                br#"{"model":"gpt-5","prompt_cache_key":"credit-affinity"}"#,
            )
        });
        let third_response = third_client
            .join()
            .unwrap_or_else(|error| panic!("yielded affinity client should finish: {error:?}"));
        let third_upstream = upstream_requests
            .recv_timeout(Duration::from_secs(2))
            .unwrap_or_else(|error| panic!("included-peer request should reach upstream: {error}"));
        let handled_connections = runtime_thread
            .join()
            .unwrap_or_else(|error| panic!("HTTP runtime should not panic: {error:?}"))
            .unwrap_or_else(|error| {
                panic!("HTTP runtime should serve all three requests: {error}")
            });
        journey = Some((
            first_response,
            second_response,
            third_response,
            first_upstream,
            second_upstream,
            third_upstream,
            handled_connections,
        ));
    });
    let (
        first_response,
        second_response,
        third_response,
        first_upstream,
        second_upstream,
        third_upstream,
        handled_connections,
    ) = journey.unwrap_or_else(|| panic!("all session-affinity requests should complete"));

    assert!(
        first_response.starts_with("HTTP/1.1 200 OK\r\n"),
        "{first_response}"
    );
    assert!(
        second_response.starts_with("HTTP/1.1 200 OK\r\n"),
        "{second_response}"
    );
    assert!(
        third_response.starts_with("HTTP/1.1 200 OK\r\n"),
        "{third_response}"
    );
    upstream_thread
        .join()
        .unwrap_or_else(|error| panic!("three-request HTTP upstream should finish: {error:?}"));
    assert_eq!(handled_connections, 3);
    assert_eq!(
        first_upstream.authorization.as_deref(),
        Some("authorization: Bearer credit-session-affinity-token")
    );
    assert_eq!(
        second_upstream.authorization.as_deref(),
        Some("authorization: Bearer credit-session-affinity-token"),
        "the live session-id pin should retain its initial credit account before a peer appears"
    );
    assert_eq!(
        third_upstream.authorization.as_deref(),
        Some("authorization: Bearer included-affinity-peer-token"),
        "session affinity must yield to the included-quota peer's real credential"
    );
    assert!(
        captured_logs.lines().any(|line| {
            line.contains("codex_router.account_reserved")
                && line.contains("selection.reason")
                && line.contains("prompt_cache_account_affinity")
        }),
        "the second request should retain prompt-cache/session affinity before a peer appears:\n{captured_logs}"
    );
    assert!(
        captured_logs.lines().any(|line| {
            line.contains("codex_router.account_reserved")
                && line.contains("selection.reason")
                && line.contains("preferred_safest_quota")
        }),
        "the third request should select the included peer with its routing reason:\n{captured_logs}"
    );
    let persisted_session_affinity =
        wait_for_session_affinity(&database_path, CREDIT_AFFINITY_SESSION_ID, |affinity| {
            affinity.account_id() == Some(included_peer.account_id())
        });
    assert_eq!(
        persisted_session_affinity.account_id(),
        Some(included_peer.account_id())
    );
}

#[test]
#[allow(clippy::result_large_err)]
fn assembled_loopback_http_previous_response_credit_owner_fails_closed_without_replay() {
    let temp_dir = ProxyTestTempDir::new("credit_affinity_previous_response_fails_closed");
    let database_path = temp_dir.path().join("state.sqlite");
    let secret_path = temp_dir.path().join("secrets");
    let secrets =
        codex_router_secret_store::test_support::open_encrypted_credential_store(&secret_path)
            .unwrap_or_else(|error| panic!("previous-response secrets should open: {error}"));
    let quota_observed_at = test_unix_seconds();
    let credit_account = AccountRecord::new(
        Provider::Openai,
        account_id("acct_credit_previous_response_affinity"),
        "credit-previous-response-affinity",
        AccountStatus::Enabled,
    );
    persist_available_credit_owner(&database_path, &secrets, &credit_account, quota_observed_at);
    let state = SqliteStateStore::open(&database_path)
        .unwrap_or_else(|error| panic!("previous-response state should open: {error}"));
    let affinity_secret = must_ok(load_or_create_router_affinity_hash_secret(&secrets))
        .secret()
        .clone();
    let included_peer = AccountRecord::new(
        Provider::Openai,
        account_id("acct_included_previous_response_peer"),
        "included-previous-response-peer",
        AccountStatus::Enabled,
    );

    let (upstream_address, upstream_requests, upstream_thread) =
        spawn_previous_response_no_replay_upstream();
    let mut first_response_result = None;
    let mut runtime_server = None;
    let captured_logs = crate::test_log_capture::capture_log_output(|| {
        let runtime = start_credit_affinity_runtime(
            upstream_address,
            &database_path,
            &secret_path,
            secrets.clone(),
            quota_observed_at + 30,
        );
        let router_address = runtime.local_addr();
        let runtime_thread = thread::spawn(move || runtime.serve_http_connections(2));
        let first_client = thread::spawn(move || {
            send_loopback_request(
                router_address,
                "POST /v1/responses HTTP/1.1\r\n",
                br#"{"model":"gpt-5"}"#,
            )
        });
        let first_response = first_client
            .join()
            .unwrap_or_else(|error| panic!("initial credit client should finish: {error:?}"));
        assert!(
            first_response.starts_with("HTTP/1.1 200 OK\r\n"),
            "{first_response}"
        );
        let first_upstream = upstream_requests
            .recv_timeout(Duration::from_secs(2))
            .unwrap_or_else(|error| {
                panic!("initial credit request should reach upstream: {error}")
            });
        assert_eq!(
            first_upstream.authorization.as_deref(),
            Some("authorization: Bearer credit-previous-response-affinity-token")
        );
        first_response_result = Some(first_response);
        runtime_server = Some((router_address, runtime_thread));
    });
    let first_response =
        first_response_result.unwrap_or_else(|| panic!("initial credit request should complete"));
    let (router_address, runtime_thread) = runtime_server
        .unwrap_or_else(|| panic!("previous-response router server should remain available"));
    assert!(
        first_response.starts_with("HTTP/1.1 200 OK\r\n"),
        "{first_response}"
    );
    assert!(
        captured_logs.lines().any(|line| {
            line.contains("codex_router.account_reserved")
                && line.contains("selection.reason")
                && line.contains("credit_backed")
        }),
        "the first request should select the credit-backed owner:\n{captured_logs}"
    );

    let response_id = PreviousResponseId::new(CREDIT_PREVIOUS_RESPONSE_ID)
        .unwrap_or_else(|error| panic!("previous response id should validate: {error}"));
    let owner_hash = hash_previous_response_id(&affinity_secret, &response_id)
        .unwrap_or_else(|error| panic!("previous response affinity hash should compute: {error}"));
    let stored_owner = wait_for_previous_response_owner(&state, &owner_hash);
    let PreviousResponseAffinityOwnerLookup::Found(stored_owner) = stored_owner else {
        panic!("the first upstream response should persist its account owner: {stored_owner:?}");
    };
    assert_eq!(stored_owner.account_id(), credit_account.account_id());

    persist_account_with_snapshot_and_token(
        &state,
        &secrets,
        &included_peer,
        90,
        INCLUDED_PEER_TOKEN,
    );

    let selector_state = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|error| panic!("affinity selector runtime should build: {error}"));
    let async_state = selector_state
        .block_on(AsyncSqliteStateStore::open_read_only(&database_path))
        .unwrap_or_else(|error| panic!("affinity selector state should open: {error}"));
    let selector = AsyncRepositoryBackedAccountSelector::new(&async_state);
    let previous_response_request = HttpProxyRequest::new(Method::Post, "/v1/responses").with_body(
        format!(r#"{{"model":"gpt-5","previous_response_id":"{CREDIT_PREVIOUS_RESPONSE_ID}"}}"#)
            .into_bytes(),
    );
    let direct_selection = selector_state.block_on(selector.select_upstream_account(
        &previous_response_request,
        TokenGeneration::new(1),
        Some(&affinity_secret),
    ));
    let previous_owner_fails_closed = matches!(
        direct_selection.as_ref(),
        Err(HttpProxyError::Selection {
            reason: QuotaAwareAccountSelectorError::AffinityOwnerUnavailable
        })
    );
    selector_state
        .block_on(async_state.close())
        .unwrap_or_else(|error| panic!("affinity selector state should close: {error}"));

    let followup_client = thread::spawn(move || {
        send_loopback_request(
            router_address,
            "POST /v1/responses HTTP/1.1\r\n",
            format!(
                r#"{{"model":"gpt-5","previous_response_id":"{CREDIT_PREVIOUS_RESPONSE_ID}"}}"#
            )
            .as_bytes(),
        )
    });
    let followup_response = followup_client
        .join()
        .unwrap_or_else(|error| panic!("previous-response client should finish: {error:?}"));
    assert_eq!(
        runtime_thread
            .join()
            .unwrap_or_else(|error| panic!("previous-response runtime should not panic: {error:?}"))
            .unwrap_or_else(|error| panic!(
                "previous-response runtime should serve two requests: {error}"
            )),
        2
    );
    let followup_body = followup_response
        .split_once("\r\n\r\n")
        .map(|(_headers, body)| body)
        .unwrap_or_else(|| {
            panic!("previous-response response should have a body: {followup_response}")
        });
    send_upstream_no_replay_probe(upstream_address);
    let upstream_tail = upstream_requests
        .recv_timeout(Duration::from_secs(2))
        .unwrap_or_else(|error| panic!("upstream should observe its no-replay probe: {error}"));
    upstream_thread
        .join()
        .unwrap_or_else(|error| panic!("no-replay upstream should finish: {error:?}"));

    assert!(
        previous_owner_fails_closed,
        "previous-response ownership must fail with the existing AffinityOwnerUnavailable selector error; observed {direct_selection:?}"
    );
    assert!(
        followup_response.starts_with("HTTP/1.1 503 Service Unavailable\r\n"),
        "lost hard affinity should use the existing unavailable transport response: {followup_response}"
    );
    assert_eq!(
        followup_body,
        crate::websocket::ROUTER_QUOTA_STATE_UNAVAILABLE_SIGNAL,
        "the previous-response owner must fail closed through the existing reconnect contract"
    );
    assert_eq!(
        upstream_tail.request_line,
        format!("GET {NO_REPLAY_PROBE_PATH} HTTP/1.1")
    );
    assert_eq!(upstream_tail.authorization, None);
}

fn persist_available_credit_owner(
    database_path: &Path,
    secrets: &EncryptedCredentialStore,
    account: &AccountRecord,
    observed_unix_seconds: u64,
) {
    persist_credit_account_with_availability(
        database_path,
        secrets,
        account,
        &format!("{}-token", account.label()),
        observed_unix_seconds,
        true,
        codex_router_core::credit_usage::CreditAvailability::Available {
            balance: Some(
                codex_router_core::credit_usage::CreditBalance::new("2.75")
                    .expect("credit fixture balance should validate"),
            ),
        },
    );
}

fn start_credit_affinity_runtime(
    upstream_address: SocketAddr,
    database_path: &Path,
    secret_path: &Path,
    secrets: EncryptedCredentialStore,
    quota_now_unix_seconds: u64,
) -> LoopbackRouterRuntime {
    let endpoint = UpstreamEndpoint::new(format!("http://{upstream_address}/v1"))
        .unwrap_or_else(|error| panic!("affinity upstream endpoint should validate: {error}"));
    let bind_address = LoopbackBindAddress::new("127.0.0.1", 0)
        .unwrap_or_else(|error| panic!("affinity router bind address should validate: {error}"));
    let config = LoopbackRouterRuntimeConfig::new(
        bind_address,
        endpoint,
        database_path.to_path_buf(),
        secret_path.to_path_buf(),
        LocalRouterTokenRecord::new(SecretString::new("current-token"), TokenGeneration::new(1)),
    )
    .with_quota_clock(quota_now_unix_seconds, 300);
    LoopbackRouterRuntime::start(config, secrets)
        .unwrap_or_else(|error| panic!("real affinity router runtime should start: {error}"))
}

#[derive(Debug, Eq, PartialEq)]
struct ObservedAffinityUpstreamRequest {
    request_line: String,
    authorization: Option<String>,
}

fn spawn_three_response_http_upstream() -> (
    SocketAddr,
    mpsc::Receiver<ObservedAffinityUpstreamRequest>,
    JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .unwrap_or_else(|error| panic!("affinity upstream should bind: {error}"));
    let address = listener
        .local_addr()
        .unwrap_or_else(|error| panic!("affinity upstream address should read: {error}"));
    let (sender, receiver) = mpsc::channel();
    let server_thread = thread::spawn(move || {
        for response_id in [
            "resp_credit_affinity_first",
            "resp_credit_affinity_second",
            "resp_included_affinity_third",
        ] {
            let (mut stream, _peer) = listener.accept().unwrap_or_else(|error| {
                panic!("affinity upstream should accept a request: {error}")
            });
            let request = read_test_http_request(&mut stream);
            sender
                .send(observe_affinity_upstream_request(&request))
                .unwrap_or_else(|error| panic!("affinity upstream request should record: {error}"));
            write_sse_response(&mut stream, response_id);
        }
    });
    (address, receiver, server_thread)
}

fn spawn_previous_response_no_replay_upstream() -> (
    SocketAddr,
    mpsc::Receiver<ObservedAffinityUpstreamRequest>,
    JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .unwrap_or_else(|error| panic!("previous-response upstream should bind: {error}"));
    let address = listener
        .local_addr()
        .unwrap_or_else(|error| panic!("previous-response upstream address should read: {error}"));
    let (sender, receiver) = mpsc::channel();
    let server_thread = thread::spawn(move || {
        let (mut first_stream, _peer) = listener
            .accept()
            .unwrap_or_else(|error| panic!("credit owner upstream should accept: {error}"));
        let first_request = read_test_http_request(&mut first_stream);
        sender
            .send(observe_affinity_upstream_request(&first_request))
            .unwrap_or_else(|error| panic!("credit owner upstream request should record: {error}"));
        write_sse_response(&mut first_stream, CREDIT_PREVIOUS_RESPONSE_ID);

        let (mut next_stream, _peer) = listener
            .accept()
            .unwrap_or_else(|error| panic!("no-replay probe should release upstream: {error}"));
        let next_request = read_test_http_request(&mut next_stream);
        let next_observation = observe_affinity_upstream_request(&next_request);
        let unexpected_proxy_request =
            next_observation.request_line != format!("GET {NO_REPLAY_PROBE_PATH} HTTP/1.1");
        sender
            .send(next_observation)
            .unwrap_or_else(|error| panic!("upstream tail request should record: {error}"));
        if unexpected_proxy_request {
            let _response_write = next_stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
            let (mut probe_stream, _peer) = listener.accept().unwrap_or_else(|error| {
                panic!("no-replay probe should follow any proxy request: {error}")
            });
            let probe_request = read_test_http_request(&mut probe_stream);
            sender
                .send(observe_affinity_upstream_request(&probe_request))
                .unwrap_or_else(|error| panic!("no-replay probe should record: {error}"));
        }
    });
    (address, receiver, server_thread)
}

fn observe_affinity_upstream_request(request: &str) -> ObservedAffinityUpstreamRequest {
    ObservedAffinityUpstreamRequest {
        request_line: request.lines().next().unwrap_or("<missing>").to_owned(),
        authorization: request
            .lines()
            .find(|line| line.starts_with("authorization: "))
            .map(str::to_owned),
    }
}

fn write_sse_response(stream: &mut TcpStream, response_id: &str) {
    let body = format!("data: {{\"id\":\"{response_id}\"}}\n\n");
    let headers = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream
        .write_all(headers.as_bytes())
        .unwrap_or_else(|error| panic!("affinity upstream response headers should write: {error}"));
    stream
        .write_all(body.as_bytes())
        .unwrap_or_else(|error| panic!("affinity upstream response body should write: {error}"));
}

fn send_upstream_no_replay_probe(upstream_address: SocketAddr) {
    let mut probe = TcpStream::connect(upstream_address)
        .unwrap_or_else(|error| panic!("no-replay probe should connect to upstream: {error}"));
    probe
        .write_all(
            format!(
                "GET {NO_REPLAY_PROBE_PATH} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
            )
            .as_bytes(),
        )
        .unwrap_or_else(|error| panic!("no-replay probe should write: {error}"));
}

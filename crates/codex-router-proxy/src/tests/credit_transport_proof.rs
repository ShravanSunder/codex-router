use super::*;

use std::net::SocketAddr;
use std::net::TcpListener;
use std::net::TcpStream;
use std::sync::mpsc;
use std::sync::mpsc::Receiver;
use std::thread::JoinHandle;
use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn assembled_loopback_http_routes_positive_and_nearzero_allowed_credit_with_credit_reason() {
    for (scenario, balance, upstream_token) in [
        ("positive_allowed", "2.75", "positive-credit-token"),
        ("nearzero_allowed", "0.0001", "nearzero-credit-token"),
    ] {
        assert_assembled_http_credit_case(
            scenario,
            true,
            available_credits(balance),
            Some(upstream_token),
        )
        .await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn assembled_loopback_http_rejects_disallowed_zero_and_depleted_credit() {
    for (scenario, allow_credit_usage, availability) in [
        ("disallowed_positive", false, available_credits("2.75")),
        ("allowed_zero", true, available_credits("0")),
        (
            "allowed_depleted",
            true,
            codex_router_core::credit_usage::CreditAvailability::Depleted,
        ),
    ] {
        assert_assembled_http_credit_case(scenario, allow_credit_usage, availability, None).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn assembled_loopback_http_credits_cover_responses_compact_and_image_routes() {
    for request_line in [
        "POST /v1/responses HTTP/1.1\r\n",
        "POST /v1/responses/compact HTTP/1.1\r\n",
        "POST /v1/images/generations HTTP/1.1\r\n",
        "POST /v1/images/edits HTTP/1.1\r\n",
    ] {
        assert_assembled_http_credit_route(
            "whole_responses_credit_family",
            true,
            available_credits("2.75"),
            Some("credit-family-token"),
            request_line,
        )
        .await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn assembled_loopback_http_preserves_provider_authoritative_hidden_and_unlimited_credits() {
    for availability in [
        codex_router_core::credit_usage::CreditAvailability::Unlimited,
        codex_router_core::credit_usage::CreditAvailability::Available { balance: None },
    ] {
        assert_assembled_http_credit_case(
            "provider_authoritative_credits",
            true,
            availability,
            Some("available-credit-token"),
        )
        .await;
    }
}

fn available_credits(balance: &str) -> codex_router_core::credit_usage::CreditAvailability {
    codex_router_core::credit_usage::CreditAvailability::Available {
        balance: Some(
            codex_router_core::credit_usage::CreditBalance::new(balance)
                .expect("fixture credit balance should be valid"),
        ),
    }
}

async fn assert_assembled_http_credit_case(
    scenario: &str,
    allow_credit_usage: bool,
    availability: codex_router_core::credit_usage::CreditAvailability,
    expected_upstream_token: Option<&str>,
) {
    assert_assembled_http_credit_route(
        scenario,
        allow_credit_usage,
        availability,
        expected_upstream_token,
        "POST /v1/responses HTTP/1.1\r\n",
    )
    .await;
}

async fn assert_assembled_http_credit_route(
    scenario: &str,
    allow_credit_usage: bool,
    availability: codex_router_core::credit_usage::CreditAvailability,
    expected_upstream_token: Option<&str>,
    request_line: &'static str,
) {
    let temp_dir = ProxyTestTempDir::new(&format!("assembled_http_credit_{scenario}"));
    let database_path = temp_dir.path().join("state.sqlite");
    let secret_path = temp_dir.path().join("secrets");
    let secrets =
        codex_router_secret_store::test_support::open_encrypted_credential_store(&secret_path)
            .unwrap_or_else(|error| panic!("credit transport secrets should open: {error}"));
    let account = AccountRecord::new(
        Provider::Openai,
        account_id("acct_assembled_credit_http"),
        scenario,
        AccountStatus::Enabled,
    );
    let upstream_token = expected_upstream_token.unwrap_or("denied-credit-token");
    persist_credit_account_with_availability(
        &database_path,
        &secrets,
        &account,
        upstream_token,
        1_030,
        allow_credit_usage,
        availability,
    )
    .await;

    let upstream_listener = TcpListener::bind("127.0.0.1:0")
        .unwrap_or_else(|error| panic!("credit mock upstream should bind: {error}"));
    let upstream_address = upstream_listener
        .local_addr()
        .unwrap_or_else(|error| panic!("credit mock upstream address should read: {error}"));
    let mut upstream_probe = UpstreamRequestProbe::start(upstream_listener);
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

    let (captured_logs, (handled_connections, response)) =
        crate::test_log_capture::capture_log_output_async(async {
            let runtime = LoopbackRouterRuntime::start(config, secrets)
                .await
                .unwrap_or_else(|error| panic!("credit proxy runtime should start: {error}"));
            let router_address = runtime.local_addr();
            let client_thread = std::thread::spawn(move || {
                send_loopback_request(
                    router_address,
                    request_line,
                    br#"{"model":"gpt-5","credit_backed":true}"#,
                )
            });
            let handled_connections = runtime
                .serve_http_connections(1)
                .await
                .unwrap_or_else(|error| panic!("credit runtime should serve request: {error}"));
            let response = client_thread
                .join()
                .unwrap_or_else(|error| panic!("credit client should finish: {error:?}"));
            (handled_connections, response)
        })
        .await;
    assert_eq!(handled_connections, 1);

    if let Some(expected_token) = expected_upstream_token {
        assert!(
            response.starts_with("HTTP/1.1 200 OK\r\n"),
            "{scenario}: {response}"
        );
        assert!(response.ends_with("\r\n\r\nok"), "{scenario}: {response}");
        let observed_request = upstream_probe.receive_request();
        assert_eq!(
            observed_request.request_line,
            request_line.trim_end(),
            "{scenario} should reach the upstream Responses endpoint"
        );
        assert_eq!(
            observed_request.authorization.as_deref(),
            Some(format!("authorization: Bearer {expected_token}").as_str()),
            "{scenario} should use the real credential resolver's selected account token"
        );
        assert!(
            captured_logs.lines().any(|line| {
                line.contains("codex_router.account_reserved")
                    && line.contains("selection.reason")
                    && line.contains("credit_backed")
            }),
            "{scenario} reservation tracing should record selection.reason=credit_backed:\n{captured_logs}"
        );
        upstream_probe.join_server();
    } else {
        assert!(
            response.starts_with("HTTP/1.1 503 Service Unavailable\r\n"),
            "{scenario} should use the all-exhausted router response: {response}"
        );
        let body = response
            .split_once("\r\n\r\n")
            .map(|(_headers, body)| body)
            .unwrap_or_else(|| {
                panic!("{scenario} response should include an error body: {response}")
            });
        assert_eq!(
            body,
            crate::websocket::ROUTER_ALL_ACCOUNTS_EXHAUSTED_SIGNAL,
            "{scenario} should preserve the exact all-exhausted error code and reason"
        );

        upstream_probe.release_with_shutdown_probe();
        let observed_request = upstream_probe.receive_request();
        assert_eq!(
            observed_request.request_line, "GET /__credit_probe_shutdown HTTP/1.1",
            "{scenario} must not forward an unauthorized Responses request"
        );
        assert_eq!(observed_request.authorization, None);
        upstream_probe.join_server();
    }
}

struct ObservedUpstreamRequest {
    request_line: String,
    authorization: Option<String>,
}

struct UpstreamRequestProbe {
    address: SocketAddr,
    receiver: Receiver<ObservedUpstreamRequest>,
    server_thread: Option<JoinHandle<()>>,
}

impl UpstreamRequestProbe {
    fn start(listener: TcpListener) -> Self {
        let address = listener
            .local_addr()
            .unwrap_or_else(|error| panic!("credit upstream address should read: {error}"));
        let (sender, receiver) = mpsc::channel();
        let server_thread = std::thread::spawn(move || {
            let (mut stream, _peer_address) = listener
                .accept()
                .unwrap_or_else(|error| panic!("credit upstream should accept a probe: {error}"));
            let request = read_test_http_request(&mut stream);
            let request_line = request.lines().next().unwrap_or("<missing>").to_owned();
            let authorization = request
                .lines()
                .find(|line| line.starts_with("authorization: "))
                .map(str::to_owned);
            sender
                .send(ObservedUpstreamRequest {
                    request_line: request_line.clone(),
                    authorization,
                })
                .unwrap_or_else(|error| {
                    panic!("credit upstream request should be observed: {error}")
                });
            if request_line != "GET /__credit_probe_shutdown HTTP/1.1" {
                stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                    )
                    .unwrap_or_else(|error| {
                        panic!("credit upstream response should write: {error}")
                    });
            }
        });
        Self {
            address,
            receiver,
            server_thread: Some(server_thread),
        }
    }

    fn receive_request(&self) -> ObservedUpstreamRequest {
        self.receiver
            .recv_timeout(Duration::from_secs(2))
            .unwrap_or_else(|error| {
                panic!("credit upstream request should arrive within its bound: {error}")
            })
    }

    fn release_with_shutdown_probe(&self) {
        let mut stream = TcpStream::connect(self.address).unwrap_or_else(|error| {
            panic!("credit upstream probe should release its listener: {error}")
        });
        stream
            .write_all(b"GET /__credit_probe_shutdown HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .unwrap_or_else(|error| panic!("credit upstream shutdown probe should write: {error}"));
    }

    fn join_server(&mut self) {
        if let Some(server_thread) = self.server_thread.take() {
            server_thread
                .join()
                .unwrap_or_else(|error| panic!("credit upstream probe should finish: {error:?}"));
        }
    }
}

impl Drop for UpstreamRequestProbe {
    fn drop(&mut self) {
        if let Some(server_thread) = self.server_thread.take() {
            if let Ok(mut stream) = TcpStream::connect(self.address) {
                let _ = stream
                    .write_all(b"GET /__credit_probe_shutdown HTTP/1.1\r\nHost: localhost\r\n\r\n");
            }
            let _ = server_thread.join();
        }
    }
}

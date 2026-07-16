use std::collections::BTreeMap;
use std::io::Read;
use std::io::Write;
use std::net::TcpListener;
use std::net::TcpStream;
use std::sync::mpsc;
use std::time::Duration;

use codex_router_core::redaction::SecretString;
use serde_json::json;

use super::super::HttpLiveQuotaResetProvider;
use super::super::LiveResetAccountAuth;
use crate::quota_reset::domain::ConsumePortResult;
use crate::quota_reset::domain::ConsumeUnknownReason;
use crate::quota_reset::domain::KnownConsumeOutcome;

use super::MAXIMUM_RESPONSE_BODY_BYTES;

const CONSUME_ACCESS_TOKEN: &str = "consume-loopback-token";
const CONSUME_ACCOUNT_ID: &str = "consume-loopback-account";
const CONSUME_CREDIT_ID: &str = "credit-loopback-selected";
const CONSUME_REDEEM_REQUEST_ID: &str = "redeem-loopback-attempt";
const TEST_REQUEST_TIMEOUT: Duration = Duration::from_millis(100);

#[tokio::test]
async fn consume_adapter_sends_exact_single_post_contract() {
    let (outcome, requests) = invoke_consume_with_behavior(ResponseBehavior::complete(
        200,
        r#"{"code":"already_redeemed","windows_reset":0}"#,
    ))
    .await;

    assert_eq!(
        outcome,
        ConsumePortResult::Known(KnownConsumeOutcome::AlreadyRedeemed)
    );
    assert_exact_single_consume_post(&requests);
}

#[tokio::test]
async fn consume_adapter_maps_all_four_known_codes_after_one_post_each() {
    for (body, expected) in [
        (
            r#"{"code":"reset","windows_reset":2}"#,
            KnownConsumeOutcome::Reset { windows_reset: 2 },
        ),
        (
            r#"{"code":"nothing_to_reset","windows_reset":0}"#,
            KnownConsumeOutcome::NothingToReset,
        ),
        (
            r#"{"code":"no_credit","windows_reset":0}"#,
            KnownConsumeOutcome::NoCredit,
        ),
        (
            r#"{"code":"already_redeemed","windows_reset":0}"#,
            KnownConsumeOutcome::AlreadyRedeemed,
        ),
    ] {
        let (outcome, requests) =
            invoke_consume_with_behavior(ResponseBehavior::complete(200, body)).await;

        assert_eq!(outcome, ConsumePortResult::Known(expected), "{body}");
        assert_exact_single_consume_post(&requests);
    }
}

#[tokio::test]
async fn consume_adapter_maps_non_success_status_to_unknown_after_one_post() {
    for status in [400, 429, 500] {
        let (outcome, requests) = invoke_consume_with_behavior(ResponseBehavior::complete(
            status,
            "provider-error-body-must-not-be-parsed",
        ))
        .await;

        assert_eq!(
            outcome,
            ConsumePortResult::OutcomeUnknown(ConsumeUnknownReason::ProviderStatus),
            "status {status}"
        );
        assert_exact_single_consume_post(&requests);
    }
}

#[tokio::test]
async fn consume_adapter_maps_timeout_to_unknown_after_one_post() {
    let (outcome, requests) =
        invoke_consume_with_behavior(ResponseBehavior::WaitForClientTimeout).await;

    assert_eq!(
        outcome,
        ConsumePortResult::OutcomeUnknown(ConsumeUnknownReason::TimedOut)
    );
    assert_exact_single_consume_post(&requests);
}

#[tokio::test]
async fn consume_adapter_maps_close_after_request_bytes_to_unknown_after_one_post() {
    let (outcome, requests) = invoke_consume_with_behavior(ResponseBehavior::Close).await;

    assert_eq!(
        outcome,
        ConsumePortResult::OutcomeUnknown(ConsumeUnknownReason::Transport)
    );
    assert_exact_single_consume_post(&requests);
}

#[tokio::test]
async fn consume_adapter_maps_truncated_body_read_to_unknown_after_one_post() {
    let response = b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 128\r\nconnection: close\r\n\r\n{\"code\":".to_vec();
    let (outcome, requests) =
        invoke_consume_with_behavior(ResponseBehavior::RawResponse(response)).await;

    assert_eq!(
        outcome,
        ConsumePortResult::OutcomeUnknown(ConsumeUnknownReason::InvalidResponse)
    );
    assert_exact_single_consume_post(&requests);
}

#[tokio::test]
async fn consume_adapter_maps_oversized_and_malformed_responses_to_unknown_after_one_post_each() {
    let oversized = format!(
        "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        MAXIMUM_RESPONSE_BODY_BYTES + 1
    )
    .into_bytes();
    let malformed_json = ResponseBehavior::complete(200, r#"{"code":"reset"#);
    let malformed_http = ResponseBehavior::RawResponse(b"not-an-http-response\r\n\r\n".to_vec());

    for (behavior, expected_reason) in [
        (
            ResponseBehavior::RawResponse(oversized),
            ConsumeUnknownReason::InvalidResponse,
        ),
        (malformed_json, ConsumeUnknownReason::InvalidResponse),
        (malformed_http, ConsumeUnknownReason::Transport),
    ] {
        let (outcome, requests) = invoke_consume_with_behavior(behavior).await;

        assert_eq!(outcome, ConsumePortResult::OutcomeUnknown(expected_reason));
        assert_exact_single_consume_post(&requests);
    }
}

#[tokio::test]
async fn consume_adapter_maps_unknown_code_to_unknown_after_one_post() {
    let (outcome, requests) = invoke_consume_with_behavior(ResponseBehavior::complete(
        200,
        r#"{"code":"future_provider_code","windows_reset":0}"#,
    ))
    .await;

    assert_eq!(
        outcome,
        ConsumePortResult::OutcomeUnknown(ConsumeUnknownReason::InvalidResponse)
    );
    assert_exact_single_consume_post(&requests);
}

enum ResponseBehavior {
    RawResponse(Vec<u8>),
    Close,
    WaitForClientTimeout,
}

impl ResponseBehavior {
    fn complete(status: u16, body: &str) -> Self {
        let reason = match status {
            200 => "OK",
            400 => "Bad Request",
            429 => "Too Many Requests",
            500 => "Internal Server Error",
            _ => "Test Status",
        };
        Self::RawResponse(
            format!(
                "HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            )
            .into_bytes(),
        )
    }
}

#[derive(Debug)]
struct RecordedRequest {
    request_line: String,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

async fn invoke_consume_with_behavior(
    behavior: ResponseBehavior,
) -> (ConsumePortResult, Vec<RecordedRequest>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .unwrap_or_else(|error| panic!("consume listener should bind: {error}"));
    let address = listener
        .local_addr()
        .unwrap_or_else(|error| panic!("consume listener address should resolve: {error}"));
    let retry_probe = listener
        .try_clone()
        .unwrap_or_else(|error| panic!("consume listener should clone: {error}"));
    let (release_sender, release_receiver) = mpsc::channel();
    let should_time_out = matches!(behavior, ResponseBehavior::WaitForClientTimeout);
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener
            .accept()
            .unwrap_or_else(|error| panic!("consume request should connect: {error}"));
        let request = read_complete_request(&mut stream);
        match behavior {
            ResponseBehavior::RawResponse(response) => {
                let _write_result = stream.write_all(&response);
            }
            ResponseBehavior::Close => {}
            ResponseBehavior::WaitForClientTimeout => {
                release_receiver
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap_or_else(|error| {
                        panic!("consume timeout server should release: {error}")
                    });
            }
        }
        request
    });
    let provider = HttpLiveQuotaResetProvider::new_loopback(address)
        .unwrap_or_else(|error| panic!("loopback consume provider should build: {error}"));
    let mut prepared = provider
        .prepare_consume_reset_credit(
            &LiveResetAccountAuth {
                access_token: SecretString::new(CONSUME_ACCESS_TOKEN),
                chatgpt_account_id: CONSUME_ACCOUNT_ID.to_owned(),
            },
            CONSUME_CREDIT_ID,
            CONSUME_REDEEM_REQUEST_ID,
        )
        .unwrap_or_else(|error| panic!("consume request should prepare: {error}"));
    if should_time_out {
        *prepared.request.timeout_mut() = Some(TEST_REQUEST_TIMEOUT);
    }

    let outcome = provider.invoke_prepared_consume(prepared).await;
    let _release_result = release_sender.send(());
    let request = server
        .join()
        .unwrap_or_else(|error| panic!("consume server should join: {error:?}"));
    assert_no_retry_connection(retry_probe);

    (outcome, vec![request])
}

fn read_complete_request(stream: &mut TcpStream) -> RecordedRequest {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap_or_else(|error| panic!("consume request timeout should set: {error}"));
    let mut request = Vec::new();
    let header_end = loop {
        if let Some(position) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
            break position + 4;
        }
        assert!(request.len() < 16_384, "consume headers should be bounded");
        read_request_bytes(stream, &mut request);
    };
    let header_bytes = request
        .get(..header_end - 4)
        .unwrap_or_else(|| panic!("consume header boundary should be valid"));
    let header_text = std::str::from_utf8(header_bytes)
        .unwrap_or_else(|error| panic!("consume headers should be utf-8: {error}"));
    let mut lines = header_text.split("\r\n");
    let request_line = lines.next().unwrap_or_default().to_owned();
    let headers = lines
        .map(|line| {
            let (name, value) = line
                .split_once(':')
                .unwrap_or_else(|| panic!("consume header should contain a colon: {line}"));
            (name.to_ascii_lowercase(), value.trim().to_owned())
        })
        .collect::<BTreeMap<_, _>>();
    let content_length = headers
        .get("content-length")
        .unwrap_or_else(|| panic!("consume request should declare content-length"))
        .parse::<usize>()
        .unwrap_or_else(|error| panic!("consume content-length should parse: {error}"));
    while request.len() < header_end + content_length {
        read_request_bytes(stream, &mut request);
    }
    assert_eq!(request.len(), header_end + content_length);

    RecordedRequest {
        request_line,
        headers,
        body: request
            .get(header_end..)
            .unwrap_or_else(|| panic!("consume body boundary should be valid"))
            .to_vec(),
    }
}

fn read_request_bytes(stream: &mut TcpStream, request: &mut Vec<u8>) {
    let mut buffer = [0_u8; 1024];
    let bytes_read = stream
        .read(&mut buffer)
        .unwrap_or_else(|error| panic!("consume request should read: {error}"));
    assert_ne!(bytes_read, 0, "consume request should be complete");
    request.extend_from_slice(&buffer[..bytes_read]);
}

fn assert_exact_single_consume_post(requests: &[RecordedRequest]) {
    assert_eq!(requests.len(), 1, "consume must issue exactly one POST");
    let request = requests
        .first()
        .unwrap_or_else(|| panic!("consume request ledger should contain one request"));
    // `credit_id` is optional in the provider protocol; this guarded flow pins
    // one selected credit and must therefore send the optional field.
    assert_eq!(
        request.request_line,
        "POST /api/codex/rate-limit-reset-credits/consume HTTP/1.1"
    );
    assert_eq!(
        request.headers.get("authorization").map(String::as_str),
        Some("Bearer consume-loopback-token")
    );
    assert_eq!(
        request
            .headers
            .get("chatgpt-account-id")
            .map(String::as_str),
        Some(CONSUME_ACCOUNT_ID)
    );
    assert_eq!(
        request.headers.get("content-type").map(String::as_str),
        Some("application/json")
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&request.body)
            .unwrap_or_else(|error| panic!("consume body should be JSON: {error}")),
        json!({
            "redeem_request_id": CONSUME_REDEEM_REQUEST_ID,
            "credit_id": CONSUME_CREDIT_ID,
        })
    );
}

fn assert_no_retry_connection(listener: TcpListener) {
    listener
        .set_nonblocking(true)
        .unwrap_or_else(|error| panic!("consume retry probe should be nonblocking: {error}"));
    assert!(
        matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock),
        "consume adapter must not retry after the single POST"
    );
}

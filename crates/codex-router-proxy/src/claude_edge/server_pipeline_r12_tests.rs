use crate::http_sse::AsyncHttpBodyError;
use bytes::Bytes;
use codex_router_core::ids::AccountId;
use codex_router_core::local_auth::LocalRouterTokenRecord;
use codex_router_core::redaction::SecretString;
use http::Response;
use http::StatusCode;
use http_body_util::BodyExt;
use http_body_util::combinators::BoxBody;
use std::io::Read;
use std::io::Write;
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;

static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

#[path = "server_pipeline_r12_tests/attempt_capture.rs"]
mod attempt_capture;
#[path = "server_pipeline_r12_tests/attempt_telemetry.rs"]
mod attempt_telemetry;
#[path = "server_pipeline_r12_tests/listener_path.rs"]
mod listener_path;
#[path = "server_pipeline_r12_tests/selection_responses.rs"]
mod selection_responses;

pub(super) fn test_database_path(test_name: &str) -> PathBuf {
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "codex-router-claude-r12-{test_name}-{}-{counter}.sqlite",
        std::process::id()
    ))
}

pub(super) fn account_id(label: &str) -> AccountId {
    AccountId::new(format!("acct_{label}"))
        .unwrap_or_else(|error| panic!("test account id should be valid: {error}"))
}

pub(super) fn local_router_token(token: &str, generation: u64) -> LocalRouterTokenRecord {
    LocalRouterTokenRecord::new(
        SecretString::new(token),
        codex_router_core::ids::TokenGeneration::new(generation),
    )
}

pub(super) fn send_claude_request(address: std::net::SocketAddr) -> String {
    let mut stream = TcpStream::connect(address)
        .unwrap_or_else(|error| panic!("fixture Claude client should connect: {error}"));
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap_or_else(|error| panic!("fixture client timeout should be set: {error}"));
    stream
        .write_all(
            b"POST /anthropic/v1/messages HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: 2\r\nAuthorization: Bearer claude-r12-local\r\n\r\n{}",
        )
        .unwrap_or_else(|error| panic!("fixture Claude request should be sent: {error}"));
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .unwrap_or_else(|error| panic!("fixture Claude response should be readable: {error}"));
    response
}

pub(super) fn read_upstream_request(stream: &mut TcpStream) {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 2_048];
    loop {
        let read_length = stream
            .read(&mut buffer)
            .unwrap_or_else(|error| panic!("fixture upstream request should be readable: {error}"));
        if read_length == 0 {
            return;
        }
        request.extend_from_slice(&buffer[..read_length]);
        let Some(headers_end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") else {
            continue;
        };
        let headers = std::str::from_utf8(&request[..headers_end])
            .unwrap_or_else(|error| panic!("fixture upstream headers should be UTF-8: {error}"));
        let content_length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })
            .unwrap_or(0);
        if request.len() >= headers_end + 4 + content_length {
            return;
        }
    }
}

pub(super) async fn read_response(
    response: Response<BoxBody<Bytes, AsyncHttpBodyError>>,
) -> (StatusCode, http::HeaderMap, serde_json::Value) {
    let (parts, body) = response.into_parts();
    let body = body
        .collect()
        .await
        .unwrap_or_else(|error| panic!("test response body should be readable: {error}"))
        .to_bytes();
    let json = serde_json::from_slice(&body)
        .unwrap_or_else(|error| panic!("test response should be valid JSON: {error}"));
    (parts.status, parts.headers, json)
}

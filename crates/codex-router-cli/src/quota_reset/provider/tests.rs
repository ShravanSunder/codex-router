use std::io::Read;
use std::io::Write;
use std::net::TcpListener;

use super::http::MAXIMUM_RESPONSE_BODY_BYTES;
use super::parsing::ResetCreditsResponse;
use super::parsing::parse_utc_rfc3339_unix_seconds;
use super::parsing::remaining_percent_from_used;
use super::parsing::reset_credits_from_response;
use super::parsing::validate_credit_status;
use super::*;
use crate::quota_reset::domain::ConsumePortResult;
use crate::quota_reset::domain::KnownConsumeOutcome;

#[test]
fn used_percentage_conversion_is_strict() {
    assert_eq!(remaining_percent_from_used(100).ok(), Some(0));
    assert_eq!(remaining_percent_from_used(99).ok(), Some(1));
    assert!(remaining_percent_from_used(-1).is_err());
    assert!(remaining_percent_from_used(101).is_err());
}

#[test]
fn credit_status_validation_refuses_unknown_values() {
    assert!(validate_credit_status("available").is_ok());
    assert!(validate_credit_status("redeeming").is_ok());
    assert!(validate_credit_status("redeemed").is_ok());
    assert!(validate_credit_status("future-provider-status").is_err());
}

#[test]
fn consume_response_refuses_unknown_codes() {
    assert!(
        serde_json::from_str::<ConsumeResetCreditResponse>(
            r#"{"code":"unexpected","windows_reset":0}"#,
        )
        .is_err()
    );
}

#[test]
fn reset_credit_payload_validation_fails_closed() {
    for response in [
        r#"{"credits":[{"id":"credit-a","status":"unknown","expires_at":null,"title":null}]}"#,
        r#"{"credits":[{"id":" ","status":"available","expires_at":null,"title":null}]}"#,
        r#"{"credits":[{"id":"credit\na","status":"available","expires_at":null,"title":null}]}"#,
        r#"{"credits":[{"id":"credit-a","status":"available","expires_at":null,"title":"unsafe\nlabel"}]}"#,
    ] {
        let details = serde_json::from_str::<ResetCreditsResponse>(response)
            .unwrap_or_else(|error| panic!("test response should deserialize: {error}"));

        assert!(reset_credits_from_response(details).is_err(), "{response}");
    }
}

#[test]
fn reset_credit_conversion_preserves_the_complete_validated_inventory() {
    let details = serde_json::from_str::<ResetCreditsResponse>(
            r#"{"credits":[{"id":"later","status":"redeeming","expires_at":null,"title":null},{"id":"earlier","status":"available","expires_at":"2026-07-14T00:00:00Z","title":"Weekly reset"}]}"#,
        )
        .unwrap_or_else(|error| panic!("test response should deserialize: {error}"));

    let credits = reset_credits_from_response(details)
        .unwrap_or_else(|error| panic!("complete inventory should validate: {error}"));

    assert_eq!(credits.len(), 2);
    assert_eq!(credits[0].id, "later");
    assert_eq!(credits[0].status, "redeeming");
    assert_eq!(credits[1].id, "earlier");
    assert_eq!(credits[1].title.as_deref(), Some("Weekly reset"));
    assert!(credits[1].expires_unix_seconds.is_some());
}

#[test]
fn utc_rfc3339_parser_orders_expirations_without_external_dependencies() {
    let earlier = parse_utc_rfc3339_unix_seconds("2026-07-14T00:00:00Z")
        .unwrap_or_else(|error| panic!("earlier timestamp should parse: {error}"));
    let later = parse_utc_rfc3339_unix_seconds("2026-07-20T00:00:00.123Z")
        .unwrap_or_else(|error| panic!("later timestamp should parse: {error}"));

    assert!(earlier < later);
    assert!(parse_utc_rfc3339_unix_seconds("2026-02-30T00:00:00Z").is_err());
    assert!(parse_utc_rfc3339_unix_seconds("2026-07-14T-1:00:00Z").is_err());
    assert!(parse_utc_rfc3339_unix_seconds("2026-7-14T00:00:00Z").is_err());
    assert!(parse_utc_rfc3339_unix_seconds("+026-07-14T00:00:00Z").is_err());
    assert!(parse_utc_rfc3339_unix_seconds("2026-07-14T00:00:00.fooZ").is_err());
    assert!(parse_utc_rfc3339_unix_seconds("2026-07-14T00:00:00.Z").is_err());
    assert!(parse_utc_rfc3339_unix_seconds("2026-07-14T00:00:00-04:00").is_err());
}

#[tokio::test]
async fn async_provider_uses_exact_get_paths_and_headers() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .unwrap_or_else(|error| panic!("loopback listener should bind: {error}"));
    let address = listener
        .local_addr()
        .unwrap_or_else(|error| panic!("loopback address should resolve: {error}"));
    let server = std::thread::spawn(move || serve_two_provider_requests(listener));
    let provider = HttpLiveQuotaResetProvider::new_loopback(format!("http://{address}"))
        .unwrap_or_else(|error| panic!("loopback provider should build: {error}"));
    let auth = LiveResetAccountAuth {
        access_token: SecretString::new("loopback-token"),
        chatgpt_account_id: "chatgpt-loopback-account".to_owned(),
    };

    let weekly = provider
        .fetch_weekly_remaining_percent(&auth)
        .await
        .unwrap_or_else(|error| panic!("loopback usage should parse: {error}"));
    let credits = provider
        .fetch_reset_credits(&auth)
        .await
        .unwrap_or_else(|error| panic!("loopback credits should parse: {error}"));
    let requests = server
        .join()
        .unwrap_or_else(|error| panic!("loopback server should join: {error:?}"));

    assert_eq!(weekly, Some(0));
    assert_eq!(credits[0].id, "credit-earliest");
    assert!(requests[0].starts_with("GET /api/codex/usage HTTP/1.1"));
    assert!(requests[1].starts_with("GET /api/codex/rate-limit-reset-credits HTTP/1.1"));
    assert!(requests.iter().all(|request| {
        request
            .to_ascii_lowercase()
            .contains("authorization: bearer loopback-token")
            && request
                .to_ascii_lowercase()
                .contains("chatgpt-account-id: chatgpt-loopback-account")
    }));
}

#[test]
fn production_provider_has_only_the_fixed_chatgpt_origin() {
    let provider = HttpLiveQuotaResetProvider::new()
        .unwrap_or_else(|error| panic!("fixed provider should build: {error}"));

    assert_eq!(provider.base_url, "https://chatgpt.com/backend-api");
}

#[test]
fn account_auth_debug_output_redacts_both_secret_and_routing_identity() {
    let debug = format!(
        "{:?}",
        LiveResetAccountAuth {
            access_token: SecretString::new("debug-secret-token"),
            chatgpt_account_id: "debug-routing-identity".to_owned(),
        }
    );

    assert!(!debug.contains("debug-secret-token"));
    assert!(!debug.contains("debug-routing-identity"));
    assert_eq!(debug.matches("[REDACTED]").count(), 2);
}

#[test]
fn loopback_test_constructor_rejects_non_loopback_origins() {
    for origin in [
        "https://chatgpt.com/backend-api",
        "http://example.test",
        "http://127.0.0.1:1234/path",
        "http://user:password@127.0.0.1:1234",
    ] {
        assert!(
            HttpLiveQuotaResetProvider::new_loopback(origin).is_err(),
            "{origin}"
        );
    }
}

#[tokio::test]
async fn consume_preparation_sends_nothing_and_invocation_sends_once() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .unwrap_or_else(|error| panic!("loopback listener should bind: {error}"));
    let address = listener.local_addr().expect("loopback address");
    let provider =
        HttpLiveQuotaResetProvider::new_loopback(format!("http://{address}")).expect("provider");
    let auth = LiveResetAccountAuth {
        access_token: SecretString::new("consume-token"),
        chatgpt_account_id: "consume-account".to_owned(),
    };

    let prepared = provider
        .prepare_consume_reset_credit(&auth, "credit-a", "redeem-a")
        .expect("prepared request");
    listener
        .set_nonblocking(true)
        .expect("nonblocking listener");
    assert!(
        matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
    );
    listener.set_nonblocking(false).expect("blocking listener");
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("single consume request");
        let mut request = [0_u8; 4096];
        let bytes = stream.read(&mut request).expect("consume request");
        assert!(
            String::from_utf8_lossy(&request[..bytes])
                .starts_with("POST /api/codex/rate-limit-reset-credits/consume HTTP/1.1")
        );
        let body = r#"{"code":"already_redeemed","windows_reset":0}"#;
        write!(
            stream,
            "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        )
        .expect("response");
    });

    let outcome = provider.invoke_prepared_consume(prepared).await;
    server.join().expect("server join");

    assert_eq!(
        outcome,
        ConsumePortResult::Known(KnownConsumeOutcome::AlreadyRedeemed)
    );
}

#[tokio::test]
async fn async_provider_refuses_redirects_without_replaying_account_requests() {
    let redirect_target = TcpListener::bind("127.0.0.1:0")
        .unwrap_or_else(|error| panic!("redirect target should bind: {error}"));
    let redirect_target_address = redirect_target
        .local_addr()
        .unwrap_or_else(|error| panic!("redirect target address should resolve: {error}"));
    let source = TcpListener::bind("127.0.0.1:0")
        .unwrap_or_else(|error| panic!("redirect source should bind: {error}"));
    let source_address = source
        .local_addr()
        .unwrap_or_else(|error| panic!("redirect source address should resolve: {error}"));
    let source_server = std::thread::spawn(move || {
        let (mut stream, _) = source
            .accept()
            .unwrap_or_else(|error| panic!("redirect source should accept: {error}"));
        let mut buffer = [0_u8; 4096];
        let _bytes = stream
            .read(&mut buffer)
            .unwrap_or_else(|error| panic!("redirect source should read: {error}"));
        let response = format!(
            "HTTP/1.1 307 Temporary Redirect\r\nlocation: http://{redirect_target_address}/captured\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
        );
        stream
            .write_all(response.as_bytes())
            .unwrap_or_else(|error| panic!("redirect source should write: {error}"));
    });
    let provider = HttpLiveQuotaResetProvider::new_loopback(format!("http://{source_address}"))
        .unwrap_or_else(|error| panic!("redirect provider should build: {error}"));
    let auth = LiveResetAccountAuth {
        access_token: SecretString::new("redirect-token"),
        chatgpt_account_id: "redirect-account".to_owned(),
    };

    let result = provider.fetch_weekly_remaining_percent(&auth).await;
    source_server
        .join()
        .unwrap_or_else(|error| panic!("redirect source should join: {error:?}"));
    redirect_target
        .set_nonblocking(true)
        .unwrap_or_else(|error| panic!("redirect target should become nonblocking: {error}"));

    assert!(matches!(
        result,
        Err(QuotaResetError::ProviderStatus { status: 307 })
    ));
    assert!(matches!(
        redirect_target.accept(),
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock
    ));
}

#[tokio::test]
async fn declared_response_lengths_enforce_the_limit_plus_one_boundary() {
    for (body_length, should_succeed) in [
        (MAXIMUM_RESPONSE_BODY_BYTES - 1, true),
        (MAXIMUM_RESPONSE_BODY_BYTES, true),
        (MAXIMUM_RESPONSE_BODY_BYTES + 1, false),
    ] {
        let body = valid_usage_body_with_length(body_length);
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        let result = fetch_usage_from_loopback_response(response, body).await;

        assert_eq!(result.is_ok(), should_succeed, "body length {body_length}");
    }
}

#[tokio::test]
async fn chunked_and_missing_lengths_enforce_the_streaming_limit() {
    for (body_length, should_succeed) in [
        (MAXIMUM_RESPONSE_BODY_BYTES - 1, true),
        (MAXIMUM_RESPONSE_BODY_BYTES, true),
        (MAXIMUM_RESPONSE_BODY_BYTES + 1, false),
    ] {
        let body = valid_usage_body_with_length(body_length);
        let chunk_header = format!("{:x}\r\n", body.len());
        let response =
            b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n".to_vec();
        let mut chunked_body = chunk_header.into_bytes();
        chunked_body.extend_from_slice(&body);
        chunked_body.extend_from_slice(b"\r\n0\r\n\r\n");
        let result = fetch_usage_from_loopback_response(response, chunked_body).await;
        assert_eq!(
            result.is_ok(),
            should_succeed,
            "chunked length {body_length}"
        );

        let response = b"HTTP/1.1 200 OK\r\nconnection: close\r\n\r\n".to_vec();
        let result = fetch_usage_from_loopback_response(response, body).await;
        assert_eq!(
            result.is_ok(),
            should_succeed,
            "missing length {body_length}"
        );
    }
}

#[tokio::test]
async fn lying_declared_lengths_fail_closed_without_exposing_response_data() {
    for declared_length in [
        MAXIMUM_RESPONSE_BODY_BYTES - 1,
        MAXIMUM_RESPONSE_BODY_BYTES,
        MAXIMUM_RESPONSE_BODY_BYTES + 1,
    ] {
        let result = fetch_usage_from_loopback_response(
            format!(
                "HTTP/1.1 200 OK\r\ncontent-length: {declared_length}\r\nconnection: close\r\n\r\n"
            )
            .into_bytes(),
            b"{}".to_vec(),
        )
        .await;
        let expected_message = if declared_length > MAXIMUM_RESPONSE_BODY_BYTES {
            "provider response body exceeds the size limit"
        } else {
            "provider response body could not be read"
        };
        assert!(matches!(
            result,
            Err(QuotaResetError::ProviderResponse { message })
                if message == expected_message
        ));
    }

    let result = fetch_usage_from_loopback_response(
        b"HTTP/1.1 200 OK\r\ncontent-length: 1\r\nconnection: close\r\n\r\n".to_vec(),
        b"{provider-secret-body}".to_vec(),
    )
    .await;
    assert!(matches!(
        result,
        Err(QuotaResetError::ProviderResponse { message })
            if message == "provider response was malformed"
                && !message.contains("provider-secret-body")
    ));
}

#[tokio::test]
async fn truncation_and_transport_failures_have_only_sanitized_classes() {
    let truncated = fetch_usage_from_loopback_response(
        b"HTTP/1.1 200 OK\r\ncontent-length: 10\r\nconnection: close\r\n\r\n".to_vec(),
        b"{}".to_vec(),
    )
    .await;
    assert!(matches!(
        truncated,
        Err(QuotaResetError::ProviderResponse { message })
            if message == "provider response body could not be read"
    ));

    let listener = TcpListener::bind("127.0.0.1:0")
        .unwrap_or_else(|error| panic!("loopback listener should bind: {error}"));
    let address = listener
        .local_addr()
        .unwrap_or_else(|error| panic!("loopback address should resolve: {error}"));
    let server = std::thread::spawn(move || {
        let (stream, _) = listener
            .accept()
            .unwrap_or_else(|error| panic!("loopback request should connect: {error}"));
        drop(stream);
    });
    let provider = HttpLiveQuotaResetProvider::new_loopback(format!("http://{address}"))
        .unwrap_or_else(|error| panic!("loopback provider should build: {error}"));
    let transport = provider
        .fetch_weekly_remaining_percent(&LiveResetAccountAuth {
            access_token: SecretString::new("must-not-appear"),
            chatgpt_account_id: "must-not-appear".to_owned(),
        })
        .await;
    server
        .join()
        .unwrap_or_else(|error| panic!("loopback server should join: {error:?}"));

    assert!(matches!(
        transport,
        Err(QuotaResetError::ProviderRequest { message })
            if message == "provider transport failed"
                && !message.contains("must-not-appear")
                && !message.contains(&address.to_string())
    ));
}

async fn fetch_usage_from_loopback_response(
    response_headers: Vec<u8>,
    response_body: Vec<u8>,
) -> Result<Option<u32>, QuotaResetError> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .unwrap_or_else(|error| panic!("loopback listener should bind: {error}"));
    let address = listener
        .local_addr()
        .unwrap_or_else(|error| panic!("loopback address should resolve: {error}"));
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener
            .accept()
            .unwrap_or_else(|error| panic!("loopback request should connect: {error}"));
        read_request_headers(&mut stream);
        let _write_result = stream.write_all(&response_headers);
        let _write_result = stream.write_all(&response_body);
    });
    let provider = HttpLiveQuotaResetProvider::new_loopback(format!("http://{address}"))
        .unwrap_or_else(|error| panic!("loopback provider should build: {error}"));
    let result = provider
        .fetch_weekly_remaining_percent(&LiveResetAccountAuth {
            access_token: SecretString::new("bounded-body-token"),
            chatgpt_account_id: "bounded-body-account".to_owned(),
        })
        .await;
    server
        .join()
        .unwrap_or_else(|error| panic!("loopback server should join: {error:?}"));
    result
}

fn valid_usage_body_with_length(length: usize) -> Vec<u8> {
    assert!(length >= 2);
    let mut body = Vec::with_capacity(length);
    body.push(b'{');
    body.resize(length - 1, b' ');
    body.push(b'}');
    body
}

fn read_request_headers(stream: &mut std::net::TcpStream) {
    let mut request = Vec::new();
    while !request.ends_with(b"\r\n\r\n") {
        assert!(
            request.len() < 16_384,
            "loopback request headers are bounded"
        );
        let mut buffer = [0_u8; 1024];
        let bytes_read = stream
            .read(&mut buffer)
            .unwrap_or_else(|error| panic!("loopback request should read: {error}"));
        assert_ne!(bytes_read, 0, "loopback request headers should complete");
        request.extend_from_slice(&buffer[..bytes_read]);
    }
}

fn serve_two_provider_requests(listener: TcpListener) -> Vec<String> {
    let bodies = [
        r#"{"rate_limit":{"primary_window":{"used_percent":10,"reset_at":1,"limit_window_seconds":18000},"secondary_window":{"used_percent":100,"reset_at":2,"limit_window_seconds":604800}},"additional_rate_limits":[]}"#,
        r#"{"credits":[{"id":"credit-earliest","status":"available","expires_at":"2026-07-14T00:00:00Z","title":"Weekly reset"}],"available_count":1}"#,
    ];
    let mut requests = Vec::new();
    for body in bodies {
        let (mut stream, _) = listener
            .accept()
            .unwrap_or_else(|error| panic!("loopback request should connect: {error}"));
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .unwrap_or_else(|error| panic!("loopback timeout should set: {error}"));
        let mut buffer = [0_u8; 8192];
        let bytes = stream
            .read(&mut buffer)
            .unwrap_or_else(|error| panic!("loopback request should read: {error}"));
        requests.push(String::from_utf8_lossy(&buffer[..bytes]).into_owned());
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        stream
            .write_all(response.as_bytes())
            .unwrap_or_else(|error| panic!("loopback response should write: {error}"));
    }
    requests
}

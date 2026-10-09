use super::*;
use codex_router_core::ids::AccountId;
use codex_router_core::provider::Provider;
use codex_router_core::redaction::SecretString;
use std::io::Read;
use std::io::Write;
use std::net::TcpListener;
use std::sync::mpsc;
use std::thread;

#[tokio::test]
async fn fake_usage_endpoint_receives_oauth_headers_and_returns_windows() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake usage listener");
    let endpoint = format!(
        "http://{}/api/oauth/usage",
        listener.local_addr().expect("listener address")
    );
    let (request_sender, request_receiver) = mpsc::channel();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept usage request");
        let mut request = Vec::new();
        let mut buffer = [0_u8; 1024];
        let request_end = loop {
            let count = stream.read(&mut buffer).expect("read usage request");
            assert!(count > 0, "usage request includes headers");
            request.extend_from_slice(&buffer[..count]);
            if let Some(index) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                break index + 4;
            }
        };
        request_sender
            .send(String::from_utf8(request[..request_end].to_vec()).expect("request headers"))
            .expect("test receives request headers");
        let body = r#"{"five_hour":{"utilization":25,"resets_at":"2026-10-01T00:00:00Z"},"seven_day":{"utilization":80,"resets_at":null}}"#;
        write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .expect("write fake usage response");
    });
    let fetcher = ClaudeQuotaFetcher::new_with_endpoint_for_test(Duration::from_secs(2), endpoint)
        .expect("fixture client");
    let account_id = AccountId::new("acct_claude_usage").expect("account id");
    let response = fetcher
        .fetch_quota(QuotaRefreshProviderRequest::new_for_provider(
            Provider::Claude,
            account_id,
            "claude account",
            "claude_messages",
            "unused-openai-base-url",
            SecretString::new("usage-access-canary"),
            None,
        ))
        .await
        .expect("usage response should decode");
    let request = request_receiver
        .recv_timeout(Duration::from_secs(2))
        .expect("fake usage server should observe one request")
        .to_ascii_lowercase();

    assert!(request.starts_with("get /api/oauth/usage "));
    assert!(request.contains("authorization: bearer usage-access-canary"));
    assert!(request.contains("anthropic-beta: oauth-2025-04-20"));
    assert_eq!(response.windows.len(), 2);
    assert_eq!(
        response.windows[0].limit_window_seconds,
        CLAUDE_FIVE_HOUR_WINDOW_SECONDS
    );
    assert_eq!(
        response.windows[0].headroom,
        QuotaWindowHeadroom::BasisPoints(7_500)
    );
    assert_eq!(
        response.windows[1].limit_window_seconds,
        CLAUDE_WEEKLY_WINDOW_SECONDS
    );
    assert_eq!(
        response.windows[1].headroom,
        QuotaWindowHeadroom::BasisPoints(2_000)
    );
    server.join().expect("fake usage server should finish");
}

#[tokio::test]
async fn fake_usage_endpoint_accepts_overlapping_legacy_and_limits_windows() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake usage listener");
    listener
        .set_nonblocking(true)
        .expect("fake usage listener should be bounded");
    let endpoint = format!(
        "http://{}/api/oauth/usage",
        listener.local_addr().expect("listener address")
    );
    let (request_sender, request_receiver) = mpsc::channel();
    let body = r#"{"five_hour":{"utilization":25,"resets_at":"2026-10-01T00:00:00Z"},"seven_day":{"utilization":80,"resets_at":"2026-10-02T00:00:00Z"},"limits":[{"kind":"session","percent":60,"resets_at":"2026-10-03T00:00:00Z"},{"kind":"weekly_all","percent":100,"resets_at":"2026-10-04T00:00:00Z"}]}"#;
    let server_body = body.to_owned();
    let server = thread::spawn(move || {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        let (mut stream, _) = loop {
            match listener.accept() {
                Ok(connection) => break connection,
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        && std::time::Instant::now() < deadline =>
                {
                    thread::yield_now();
                }
                Err(error) => panic!("fake usage server should accept request: {error}"),
            }
        };
        stream
            .set_nonblocking(false)
            .expect("accepted fake usage stream should use blocking reads");
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("fake usage stream should have bounded reads");
        let mut request = Vec::new();
        let mut buffer = [0_u8; 1024];
        let request_end = loop {
            let count = stream.read(&mut buffer).expect("read usage request");
            assert!(count > 0, "usage request includes headers");
            request.extend_from_slice(&buffer[..count]);
            if let Some(index) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                break index + 4;
            }
        };
        request_sender
            .send(String::from_utf8(request[..request_end].to_vec()).expect("request headers"))
            .expect("test receives request headers");
        write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                server_body.len(),
                server_body
            )
            .expect("write fake usage response");
    });
    let fetcher = ClaudeQuotaFetcher::new_with_endpoint_for_test(Duration::from_secs(2), endpoint)
        .expect("fixture client");
    let account_id = AccountId::new("acct_claude_overlap").expect("account id");
    let response = fetcher
        .fetch_quota(QuotaRefreshProviderRequest::new_for_provider(
            Provider::Claude,
            account_id,
            "claude account",
            "claude_messages",
            "unused-openai-base-url",
            SecretString::new("usage-access-canary"),
            None,
        ))
        .await;
    let request = request_receiver
        .recv_timeout(Duration::from_secs(3))
        .expect("fake usage server should observe one request")
        .to_ascii_lowercase();
    server.join().expect("fake usage server should finish");

    assert!(request.starts_with("get /api/oauth/usage "));
    assert!(request.contains("anthropic-beta: oauth-2025-04-20"));
    let response = response.expect("overlapping provider representations should parse");
    assert_eq!(response.windows.len(), 2);
    assert_eq!(
        response.windows[0].headroom,
        QuotaWindowHeadroom::BasisPoints(4_000)
    );
    assert_eq!(response.windows[0].reset_unix_seconds, Some(1_790_985_600));
    assert_eq!(
        response.windows[1].headroom,
        QuotaWindowHeadroom::BasisPoints(0)
    );
    assert_eq!(response.windows[1].reset_unix_seconds, Some(1_791_072_000));
}

#[test]
fn claude_usage_parses_legacy_and_limits_windows_without_inventing_missing_usage() {
    let legacy = parse_claude_usage_response(
        r#"{"five_hour":{"utilization":45,"resets_at":"2026-10-01T00:00:00Z"},"seven_day":null}"#,
    )
    .expect("legacy usage response should parse");
    assert_eq!(legacy.windows.len(), 1);
    assert_eq!(
        legacy.windows[0].limit_window_seconds,
        CLAUDE_FIVE_HOUR_WINDOW_SECONDS
    );
    assert_eq!(
        legacy.windows[0].headroom,
        QuotaWindowHeadroom::BasisPoints(5_500)
    );
    assert_eq!(legacy.windows[0].reset_unix_seconds, Some(1_790_812_800));

    let limits = parse_claude_usage_response(
            r#"{"five_hour":null,"seven_day":null,"limits":[{"kind":"session","percent":12.5,"resets_at":"2026-10-01T00:00:00Z"},{"kind":"weekly_all","percent":80,"resets_at":null},{"kind":"weekly_scoped","percent":99,"resets_at":null}]}"#,
        )
        .expect("limits response should parse");
    assert_eq!(limits.windows.len(), 2);
    assert_eq!(
        limits.windows[0].limit_window_seconds,
        CLAUDE_FIVE_HOUR_WINDOW_SECONDS
    );
    assert_eq!(
        limits.windows[0].headroom,
        QuotaWindowHeadroom::BasisPoints(8_750)
    );
    assert_eq!(
        limits.windows[1].limit_window_seconds,
        CLAUDE_WEEKLY_WINDOW_SECONDS
    );
    assert_eq!(
        limits.windows[1].headroom,
        QuotaWindowHeadroom::BasisPoints(2_000)
    );

    let unknown = parse_claude_usage_response(r#"{"five_hour":null,"seven_day":null}"#)
        .expect("valid response with unknown windows remains unknown");
    assert!(unknown.windows.is_empty());
}

#[test]
fn claude_usage_structured_limits_override_overlapping_legacy_windows() {
    let response = parse_claude_usage_response(
            r#"{"five_hour":{"utilization":25,"resets_at":"2026-10-01T00:00:00Z"},"seven_day":{"utilization":80,"resets_at":"2026-10-02T00:00:00Z"},"limits":[{"kind":"session","percent":60,"resets_at":"2026-10-03T00:00:00Z"},{"kind":"weekly_all","percent":100,"resets_at":"2026-10-04T00:00:00Z"}]}"#,
        )
        .expect("structured limits should override overlapping legacy windows");

    assert_eq!(response.windows.len(), 2);
    assert_eq!(
        response.windows[0].limit_window_seconds,
        CLAUDE_FIVE_HOUR_WINDOW_SECONDS
    );
    assert_eq!(
        response.windows[0].headroom,
        QuotaWindowHeadroom::BasisPoints(4_000)
    );
    assert_eq!(response.windows[0].reset_unix_seconds, Some(1_790_985_600));
    assert_eq!(
        response.windows[1].limit_window_seconds,
        CLAUDE_WEEKLY_WINDOW_SECONDS
    );
    assert_eq!(
        response.windows[1].headroom,
        QuotaWindowHeadroom::BasisPoints(0)
    );
    assert_eq!(response.windows[1].reset_unix_seconds, Some(1_791_072_000));
}

#[test]
fn claude_usage_keeps_equal_structured_values_and_structured_missing_reset() {
    let equal_overlap = parse_claude_usage_response(
            r#"{"five_hour":{"utilization":25,"resets_at":"2026-10-01T00:00:00Z"},"seven_day":{"utilization":80,"resets_at":"2026-10-02T00:00:00Z"},"limits":[{"kind":"session","percent":25,"resets_at":"2026-10-01T00:00:00Z"},{"kind":"weekly_all","percent":80,"resets_at":"2026-10-02T00:00:00Z"}]}"#,
        )
        .expect("equal overlapping values should parse");
    assert_eq!(equal_overlap.windows.len(), 2);
    assert_eq!(
        equal_overlap.windows[0].headroom,
        QuotaWindowHeadroom::BasisPoints(7_500)
    );
    assert_eq!(
        equal_overlap.windows[0].reset_unix_seconds,
        Some(1_790_812_800)
    );
    assert_eq!(
        equal_overlap.windows[1].headroom,
        QuotaWindowHeadroom::BasisPoints(2_000)
    );
    assert_eq!(
        equal_overlap.windows[1].reset_unix_seconds,
        Some(1_790_899_200)
    );

    let structured_missing_reset = parse_claude_usage_response(
            r#"{"five_hour":{"utilization":25,"resets_at":"2026-10-01T00:00:00Z"},"limits":[{"kind":"session","percent":25,"resets_at":null}]}"#,
        )
        .expect("selected structured null reset should override the legacy reset");
    assert_eq!(structured_missing_reset.windows.len(), 1);
    assert_eq!(structured_missing_reset.windows[0].reset_unix_seconds, None);
}

#[test]
fn claude_usage_uses_per_window_fallback_and_keeps_weekly_exhaustion() {
    let structured_session_with_legacy_weekly = parse_claude_usage_response(
            r#"{"five_hour":{"utilization":25,"resets_at":"2026-10-01T00:00:00Z"},"seven_day":{"utilization":80,"resets_at":"2026-10-02T00:00:00Z"},"limits":[{"kind":"session","percent":40,"resets_at":"2026-10-03T00:00:00Z"}]}"#,
        )
        .expect("legacy weekly window should fill only its missing structured kind");
    assert_eq!(structured_session_with_legacy_weekly.windows.len(), 2);
    assert_eq!(
        structured_session_with_legacy_weekly.windows[0].headroom,
        QuotaWindowHeadroom::BasisPoints(6_000)
    );
    assert_eq!(
        structured_session_with_legacy_weekly.windows[0].reset_unix_seconds,
        Some(1_790_985_600)
    );
    assert_eq!(
        structured_session_with_legacy_weekly.windows[1].headroom,
        QuotaWindowHeadroom::BasisPoints(2_000)
    );
    assert_eq!(
        structured_session_with_legacy_weekly.windows[1].reset_unix_seconds,
        Some(1_790_899_200)
    );

    let legacy_session_with_exhausted_structured_weekly = parse_claude_usage_response(
            r#"{"five_hour":{"utilization":25,"resets_at":"2026-10-01T00:00:00Z"},"seven_day":{"utilization":80,"resets_at":"2026-10-02T00:00:00Z"},"limits":[{"kind":"weekly_all","percent":100,"resets_at":"2026-10-04T00:00:00Z"}]}"#,
        )
        .expect("legacy session should fill only its missing structured kind");
    assert_eq!(
        legacy_session_with_exhausted_structured_weekly
            .windows
            .len(),
        2
    );
    assert_eq!(
        legacy_session_with_exhausted_structured_weekly.windows[0].headroom,
        QuotaWindowHeadroom::BasisPoints(7_500)
    );
    assert_eq!(
        legacy_session_with_exhausted_structured_weekly.windows[1].headroom,
        QuotaWindowHeadroom::BasisPoints(0)
    );
    assert_eq!(
        legacy_session_with_exhausted_structured_weekly.windows[1].reset_unix_seconds,
        Some(1_791_072_000)
    );
}

#[test]
fn claude_usage_rejects_repeated_structured_kinds_in_either_list_order() {
    for limits in [
        r#"[{"kind":"session","percent":20,"resets_at":null},{"kind":"weekly_all","percent":30,"resets_at":null},{"kind":"session","percent":40,"resets_at":null}]"#,
        r#"[{"kind":"weekly_all","percent":30,"resets_at":null},{"kind":"session","percent":20,"resets_at":null},{"kind":"weekly_all","percent":40,"resets_at":null}]"#,
    ] {
        let body = format!(
            r#"{{"five_hour":{{"utilization":25,"resets_at":null}},"seven_day":{{"utilization":80,"resets_at":null}},"limits":{limits}}}"#
        );
        let error = parse_claude_usage_response(&body)
            .expect_err("repeated entries within limits remain malformed");
        assert!(error.to_string().contains("duplicate Claude quota window"));
    }
}

#[test]
fn claude_usage_rejects_malformed_selected_limits_and_ignores_unused_legacy_semantics() {
    let malformed_percent = parse_claude_usage_response(
            r#"{"five_hour":{"utilization":25,"resets_at":null},"limits":[{"kind":"session","percent":"sixty","resets_at":null}]}"#,
        )
        .expect_err("malformed selected structured percentage must not fall back to legacy");
    assert!(
        malformed_percent
            .to_string()
            .contains("Claude quota utilization percentage is not numeric")
    );

    let malformed_reset = parse_claude_usage_response(
        r#"{"seven_day":{"utilization":80,"resets_at":null},"limits":[{"kind":"weekly_all","percent":60,"resets_at":"not-a-timestamp"}]}"#,
    );
    assert!(malformed_reset.is_err());

    let selected_limits = parse_claude_usage_response(
            r#"{"five_hour":{"utilization":-10,"resets_at":"invalid-unused-reset"},"seven_day":{"utilization":-20,"resets_at":"invalid-unused-reset"},"limits":[{"kind":"session","percent":35,"resets_at":"2026-10-03T00:00:00Z"},{"kind":"weekly_all","percent":65,"resets_at":"2026-10-04T00:00:00Z"}]}"#,
        )
        .expect("unused legacy semantic values should not defeat selected structured windows");
    assert_eq!(selected_limits.windows.len(), 2);
    assert_eq!(
        selected_limits.windows[0].headroom,
        QuotaWindowHeadroom::BasisPoints(6_500)
    );
    assert_eq!(
        selected_limits.windows[1].headroom,
        QuotaWindowHeadroom::BasisPoints(3_500)
    );
}

#[test]
fn claude_usage_ignores_unknown_fields_and_irrelevant_window_utilization() {
    let usage = parse_claude_usage_response(
            r#"{"oauth_app":{},"five_hour":{"utilization":25,"resets_at":null,"is_local_entitlement":true},"seven_day":null,"limits":[{"kind":"weekly_scoped","percent":1000,"resets_at":null,"model":"claude-opus"}]}"#,
        )
        .expect("unknown fields and irrelevant utilization must not reject account windows");

    assert_eq!(usage.windows.len(), 1);
    assert_eq!(
        usage.windows[0].limit_window_seconds,
        CLAUDE_FIVE_HOUR_WINDOW_SECONDS
    );
    assert_eq!(
        usage.windows[0].headroom,
        QuotaWindowHeadroom::BasisPoints(7_500)
    );
}

#[test]
fn claude_utilization_above_one_hundred_percent_clamps_headroom_to_zero() {
    let response = parse_claude_usage_response(
        r#"{"five_hour":{"utilization":125,"resets_at":null},"seven_day":null}"#,
    )
    .expect("over-limit utilization should represent an exhausted window");

    assert_eq!(
        response.windows[0].headroom,
        QuotaWindowHeadroom::BasisPoints(0)
    );
}

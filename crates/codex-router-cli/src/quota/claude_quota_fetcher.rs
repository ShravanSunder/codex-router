//! Claude subscription quota polling and observed OAuth usage response parsing.

use super::*;

const CLAUDE_USAGE_ENDPOINT: &str = "https://api.anthropic.com/api/oauth/usage";
const CLAUDE_OAUTH_BETA: &str = "oauth-2025-04-20";
const CLAUDE_FIVE_HOUR_WINDOW_SECONDS: u64 = 5 * 60 * 60;
const CLAUDE_WEEKLY_WINDOW_SECONDS: u64 = 7 * 24 * 60 * 60;

#[derive(Debug)]
pub(super) struct ClaudeQuotaFetcher {
    client: reqwest::Client,
    usage_endpoint: String,
}

impl ClaudeQuotaFetcher {
    pub(super) fn new(client: reqwest::Client) -> Self {
        Self {
            client,
            usage_endpoint: CLAUDE_USAGE_ENDPOINT.to_owned(),
        }
    }

    #[cfg(test)]
    pub(super) fn new_with_endpoint_for_test(
        timeout: Duration,
        usage_endpoint: impl Into<String>,
    ) -> Self {
        let client = reqwest::Client::builder()
            .user_agent("codex-router-claude-quota-refresh")
            .timeout(timeout)
            .build()
            .expect("test HTTP client should build");
        let mut fetcher = Self::new(client);
        fetcher.usage_endpoint = usage_endpoint.into();
        fetcher
    }

    pub(super) async fn fetch_quota(
        &self,
        request: QuotaRefreshProviderRequest,
    ) -> Result<QuotaRefreshProviderResponse, QuotaCommandError> {
        // Endpoint and oauth beta are pinned in claude-oauth-external-facts.md §3 and
        // claude-oss-proxy-source-audit.md §3.4 (CRS usage fetcher anchors).
        let response = self
            .client
            .get(&self.usage_endpoint)
            .bearer_auth(request.access_token().expose_secret())
            .header("anthropic-beta", CLAUDE_OAUTH_BETA)
            .send()
            .await
            .map_err(|error| QuotaCommandError::ProviderRequest {
                message: error.to_string(),
            })?;
        let status = response.status();
        if !status.is_success() {
            return Err(QuotaCommandError::ProviderStatus {
                status: status.as_u16(),
            });
        }
        let body = response
            .text()
            .await
            .map_err(|error| QuotaCommandError::ProviderRequest {
                message: error.to_string(),
            })?;
        parse_claude_usage_response(&body)
    }
}

#[derive(serde::Deserialize)]
struct ClaudeUsageResponse {
    five_hour: Option<ClaudeLegacyUsageWindow>,
    seven_day: Option<ClaudeLegacyUsageWindow>,
    #[serde(default)]
    limits: Option<Vec<ClaudeUsageLimit>>,
}

#[derive(serde::Deserialize)]
struct ClaudeLegacyUsageWindow {
    utilization: Option<f64>,
    resets_at: Option<String>,
}

#[derive(serde::Deserialize)]
struct ClaudeUsageLimit {
    kind: String,
    percent: serde_json::Value,
    resets_at: Option<String>,
}

fn parse_claude_usage_response(
    body: &str,
) -> Result<QuotaRefreshProviderResponse, QuotaCommandError> {
    let usage: ClaudeUsageResponse =
        serde_json::from_str(body).map_err(|error| QuotaCommandError::ProviderResponse {
            message: error.to_string(),
        })?;
    let mut five_hour = usage
        .five_hour
        .map(parse_legacy_window)
        .transpose()?
        .flatten();
    let mut weekly = usage
        .seven_day
        .map(parse_legacy_window)
        .transpose()?
        .flatten();

    for limit in usage.limits.unwrap_or_default() {
        match limit.kind.as_str() {
            "session" if five_hour.is_none() => {
                five_hour = Some(parse_limit_window(&limit)?);
            }
            "weekly_all" if weekly.is_none() => {
                weekly = Some(parse_limit_window(&limit)?);
            }
            "session" | "weekly_all" => {
                return Err(QuotaCommandError::ProviderResponse {
                    message: "duplicate Claude quota window in usage response".to_owned(),
                });
            }
            // The inspected clients distinguish weekly_scoped model windows; Router's account
            // selector owns only the shared five-hour and all-model weekly windows.
            "weekly_scoped" => {}
            _ => {}
        }
    }

    let mut windows = Vec::with_capacity(2);
    if let Some(window) = five_hour {
        windows.push(quota_window_from_parsed(
            window,
            CLAUDE_FIVE_HOUR_WINDOW_SECONDS,
        ));
    }
    if let Some(window) = weekly {
        windows.push(quota_window_from_parsed(
            window,
            CLAUDE_WEEKLY_WINDOW_SECONDS,
        ));
    }
    Ok(QuotaRefreshProviderResponse {
        windows,
        reset_credits_available: None,
    })
}

fn parse_limit_window(
    limit: &ClaudeUsageLimit,
) -> Result<ParsedClaudeUsageWindow, QuotaCommandError> {
    let utilization_percent =
        limit
            .percent
            .as_f64()
            .ok_or_else(|| QuotaCommandError::ProviderResponse {
                message: "Claude quota utilization percentage is not numeric".to_owned(),
            })?;
    parse_percent_window(utilization_percent, limit.resets_at.as_deref())
}

#[derive(Clone, Copy)]
struct ParsedClaudeUsageWindow {
    remaining_basis_points: u32,
    reset_unix_seconds: Option<u64>,
}

fn parse_legacy_window(
    window: ClaudeLegacyUsageWindow,
) -> Result<Option<ParsedClaudeUsageWindow>, QuotaCommandError> {
    window
        .utilization
        .map(|percent| parse_percent_window(percent, window.resets_at.as_deref()))
        .transpose()
}

fn parse_percent_window(
    utilization_percent: f64,
    resets_at: Option<&str>,
) -> Result<ParsedClaudeUsageWindow, QuotaCommandError> {
    if !utilization_percent.is_finite() || utilization_percent < 0.0 {
        return Err(QuotaCommandError::ProviderResponse {
            message: "Claude quota utilization percentage is negative or not finite".to_owned(),
        });
    }
    let remaining_basis_points = ((100.0 - utilization_percent.min(100.0)) * 100.0).round() as u32;
    let reset_unix_seconds = resets_at
        .map(|reset| {
            chrono::DateTime::parse_from_rfc3339(reset)
                .map(|date_time| date_time.timestamp())
                .map_err(|error| QuotaCommandError::ProviderResponse {
                    message: error.to_string(),
                })
        })
        .transpose()?
        .and_then(|timestamp| u64::try_from(timestamp).ok());
    Ok(ParsedClaudeUsageWindow {
        remaining_basis_points,
        reset_unix_seconds,
    })
}

fn quota_window_from_parsed(
    parsed: ParsedClaudeUsageWindow,
    limit_window_seconds: u64,
) -> QuotaRefreshProviderWindow {
    QuotaRefreshProviderWindow {
        limit_window_seconds,
        headroom: QuotaWindowHeadroom::BasisPoints(parsed.remaining_basis_points),
        reset_unix_seconds: parsed.reset_unix_seconds,
        effective: true,
    }
}

#[cfg(test)]
mod tests {
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
        let fetcher =
            ClaudeQuotaFetcher::new_with_endpoint_for_test(Duration::from_secs(2), endpoint);
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
}

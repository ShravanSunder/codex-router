//! Async live provider protocol for guarded quota reset.

use codex_router_auth::live_quota::UsageResponse;
use codex_router_auth::live_quota::reset_credits_url;
use codex_router_auth::live_quota::usage_url;
use codex_router_core::redaction::SecretString;
use reqwest::header::CONTENT_TYPE;
use serde::Deserialize;
use serde::Serialize;

use super::QuotaResetError;
use super::domain::LiveResetCredit;

const WEEKLY_WINDOW_SECONDS: i64 = 604_800;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LiveResetAccountAuth {
    pub(crate) access_token: SecretString,
    pub(crate) chatgpt_account_id: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ConsumeResetCreditCode {
    Reset,
    NothingToReset,
    NoCredit,
    AlreadyRedeemed,
}

impl ConsumeResetCreditCode {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Reset => "reset",
            Self::NothingToReset => "nothing_to_reset",
            Self::NoCredit => "no_credit",
            Self::AlreadyRedeemed => "already_redeemed",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub(crate) struct ConsumeResetCreditResponse {
    pub(crate) code: ConsumeResetCreditCode,
    #[serde(default)]
    pub(crate) windows_reset: i64,
}

pub(crate) trait LiveQuotaResetProvider {
    async fn fetch_weekly_remaining_percent(
        &self,
        auth: &LiveResetAccountAuth,
    ) -> Result<Option<u32>, QuotaResetError>;

    async fn fetch_reset_credits(
        &self,
        auth: &LiveResetAccountAuth,
    ) -> Result<Vec<LiveResetCredit>, QuotaResetError>;

    async fn consume_reset_credit(
        &self,
        auth: &LiveResetAccountAuth,
        credit_id: &str,
        redeem_request_id: &str,
    ) -> Result<ConsumeResetCreditResponse, QuotaResetError>;
}

#[derive(Clone, Debug)]
pub(crate) struct HttpLiveQuotaResetProvider {
    client: reqwest::Client,
    base_url: String,
}

impl HttpLiveQuotaResetProvider {
    pub(crate) fn new(base_url: impl Into<String>) -> Result<Self, QuotaResetError> {
        let client = reqwest::Client::builder()
            .user_agent("codex-router-quota-reset")
            .timeout(std::time::Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(provider_request_error)?;
        Ok(Self {
            client,
            base_url: base_url.into(),
        })
    }

    fn authenticated_request(
        &self,
        request: reqwest::RequestBuilder,
        auth: &LiveResetAccountAuth,
    ) -> reqwest::RequestBuilder {
        let request = request.bearer_auth(auth.access_token.expose_secret());
        request.header("ChatGPT-Account-ID", &auth.chatgpt_account_id)
    }

    async fn successful_body(request: reqwest::RequestBuilder) -> Result<String, QuotaResetError> {
        let response = request.send().await.map_err(provider_request_error)?;
        let status = response.status();
        let body = response.text().await.map_err(provider_request_error)?;
        if !status.is_success() {
            return Err(QuotaResetError::ProviderStatus {
                status: status.as_u16(),
            });
        }
        Ok(body)
    }
}

impl LiveQuotaResetProvider for HttpLiveQuotaResetProvider {
    async fn fetch_weekly_remaining_percent(
        &self,
        auth: &LiveResetAccountAuth,
    ) -> Result<Option<u32>, QuotaResetError> {
        let request = self.authenticated_request(self.client.get(usage_url(&self.base_url)), auth);
        let body = Self::successful_body(request).await?;
        let usage =
            serde_json::from_str::<UsageResponse>(&body).map_err(provider_response_error)?;
        let weekly_window = usage.rate_limit.as_ref().and_then(|windows| {
            windows
                .primary_window
                .iter()
                .chain(windows.secondary_window.iter())
                .find(|window| window.limit_window_seconds == Some(WEEKLY_WINDOW_SECONDS))
        });
        weekly_window
            .and_then(|window| window.used_percent)
            .map(remaining_percent_from_used)
            .transpose()
    }

    async fn fetch_reset_credits(
        &self,
        auth: &LiveResetAccountAuth,
    ) -> Result<Vec<LiveResetCredit>, QuotaResetError> {
        let request =
            self.authenticated_request(self.client.get(reset_credits_url(&self.base_url)), auth);
        let body = Self::successful_body(request).await?;
        let details =
            serde_json::from_str::<ResetCreditsResponse>(&body).map_err(provider_response_error)?;
        reset_credits_from_response(details)
    }

    async fn consume_reset_credit(
        &self,
        auth: &LiveResetAccountAuth,
        credit_id: &str,
        redeem_request_id: &str,
    ) -> Result<ConsumeResetCreditResponse, QuotaResetError> {
        let url = format!(
            "{}/consume",
            reset_credits_url(&self.base_url).trim_end_matches('/')
        );
        let body = serde_json::to_vec(&ConsumeResetCreditRequest {
            redeem_request_id,
            credit_id,
        })
        .map_err(provider_response_error)?;
        let request = self.authenticated_request(
            self.client
                .post(url)
                .header(CONTENT_TYPE, "application/json")
                .body(body),
            auth,
        );
        let body = Self::successful_body(request).await?;
        serde_json::from_str(&body).map_err(provider_response_error)
    }
}

fn reset_credits_from_response(
    details: ResetCreditsResponse,
) -> Result<Vec<LiveResetCredit>, QuotaResetError> {
    details
        .credits
        .into_iter()
        .map(|credit| {
            validate_credit_status(&credit.status)?;
            if credit.id.trim().is_empty() {
                return Err(QuotaResetError::ProviderResponse {
                    message: "reset credit id must not be empty".to_owned(),
                });
            }
            if credit
                .title
                .as_deref()
                .is_some_and(|title| title.chars().any(char::is_control))
            {
                return Err(QuotaResetError::ProviderResponse {
                    message: "reset credit title contains control characters".to_owned(),
                });
            }
            let expires_unix_seconds = credit
                .expires_at
                .as_deref()
                .map(parse_utc_rfc3339_unix_seconds)
                .transpose()?;
            Ok(LiveResetCredit {
                id: credit.id,
                status: credit.status,
                expires_unix_seconds,
                expires_at: credit.expires_at,
                title: credit.title,
            })
        })
        .collect()
}

#[derive(Debug, Deserialize)]
struct ResetCreditsResponse {
    credits: Vec<ResetCreditPayload>,
}

#[derive(Debug, Deserialize)]
struct ResetCreditPayload {
    id: String,
    status: String,
    expires_at: Option<String>,
    title: Option<String>,
}

#[derive(Debug, Serialize)]
struct ConsumeResetCreditRequest<'a> {
    redeem_request_id: &'a str,
    credit_id: &'a str,
}

fn remaining_percent_from_used(used_percent: i64) -> Result<u32, QuotaResetError> {
    if !(0..=100).contains(&used_percent) {
        return Err(QuotaResetError::ProviderResponse {
            message: "weekly used_percent must be between 0 and 100".to_owned(),
        });
    }
    u32::try_from(100 - used_percent).map_err(provider_response_error)
}

fn validate_credit_status(status: &str) -> Result<(), QuotaResetError> {
    if matches!(status, "available" | "redeeming" | "redeemed") {
        return Ok(());
    }
    Err(QuotaResetError::ProviderResponse {
        message: "unknown reset credit status".to_owned(),
    })
}

fn parse_utc_rfc3339_unix_seconds(value: &str) -> Result<i64, QuotaResetError> {
    let value = value
        .strip_suffix('Z')
        .ok_or_else(|| QuotaResetError::ProviderResponse {
            message: "reset-credit expiration must be a UTC RFC 3339 timestamp".to_owned(),
        })?;
    let (date, time) = value
        .split_once('T')
        .ok_or_else(|| QuotaResetError::ProviderResponse {
            message: "reset-credit expiration must contain T".to_owned(),
        })?;
    let mut date_parts = date.split('-');
    let year = parse_timestamp_part(date_parts.next(), "year", 4)?;
    let month = parse_timestamp_part(date_parts.next(), "month", 2)?;
    let day = parse_timestamp_part(date_parts.next(), "day", 2)?;
    if date_parts.next().is_some() || !(1..=12).contains(&month) {
        return Err(invalid_timestamp("date"));
    }
    let month_days = [
        31,
        28 + i64::from(is_leap_year(year)),
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let month_index = usize::try_from(month - 1).map_err(provider_response_error)?;
    if day < 1 || day > month_days.get(month_index).copied().unwrap_or(0) {
        return Err(invalid_timestamp("day"));
    }

    let mut time_parts = time.split(':');
    let hour = parse_timestamp_part(time_parts.next(), "hour", 2)?;
    let minute = parse_timestamp_part(time_parts.next(), "minute", 2)?;
    let second_part = time_parts
        .next()
        .ok_or_else(|| invalid_timestamp("second"))?;
    let mut second_parts = second_part.split('.');
    let second = parse_timestamp_part(second_parts.next(), "second", 2)?;
    if let Some(fraction) = second_parts.next()
        && (fraction.is_empty() || !fraction.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return Err(invalid_timestamp("fraction"));
    }
    if second_parts.next().is_some()
        || time_parts.next().is_some()
        || !(0..=23).contains(&hour)
        || !(0..=59).contains(&minute)
        || !(0..=59).contains(&second)
    {
        return Err(invalid_timestamp("time"));
    }

    let days = days_since_unix_epoch(year, month, day);
    days.checked_mul(86_400)
        .and_then(|seconds| seconds.checked_add(hour * 3_600 + minute * 60 + second))
        .ok_or_else(|| invalid_timestamp("range"))
}

fn parse_timestamp_part(
    value: Option<&str>,
    name: &str,
    expected_width: usize,
) -> Result<i64, QuotaResetError> {
    let value = value.ok_or_else(|| invalid_timestamp(name))?;
    if value.len() != expected_width || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid_timestamp(name));
    }
    value.parse::<i64>().map_err(provider_response_error)
}

const fn is_leap_year(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

const fn days_since_unix_epoch(year: i64, month: i64, day: i64) -> i64 {
    let adjusted_year = year - if month <= 2 { 1 } else { 0 };
    let era = if adjusted_year >= 0 {
        adjusted_year
    } else {
        adjusted_year - 399
    } / 400;
    let year_of_era = adjusted_year - era * 400;
    let adjusted_month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * adjusted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn invalid_timestamp(part: &str) -> QuotaResetError {
    QuotaResetError::ProviderResponse {
        message: format!("invalid reset-credit expiration {part}"),
    }
}

fn provider_request_error(error: impl std::fmt::Display) -> QuotaResetError {
    QuotaResetError::ProviderRequest {
        message: error.to_string(),
    }
}

fn provider_response_error(error: impl std::fmt::Display) -> QuotaResetError {
    QuotaResetError::ProviderResponse {
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::io::Read;
    use std::io::Write;
    use std::net::TcpListener;

    use super::*;

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
            r#"{"credits":[{"id":"credit-a","status":"available","expires_at":null,"title":"unsafe\nlabel"}]}"#,
        ] {
            let details = serde_json::from_str::<ResetCreditsResponse>(response)
                .unwrap_or_else(|error| panic!("test response should deserialize: {error}"));

            assert!(reset_credits_from_response(details).is_err(), "{response}");
        }
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
    async fn async_provider_uses_exact_account_paths_headers_and_consume_payload() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .unwrap_or_else(|error| panic!("loopback listener should bind: {error}"));
        let address = listener
            .local_addr()
            .unwrap_or_else(|error| panic!("loopback address should resolve: {error}"));
        let server = std::thread::spawn(move || serve_three_provider_requests(listener));
        let provider = HttpLiveQuotaResetProvider::new(format!("http://{address}"))
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
        let consumed = provider
            .consume_reset_credit(&auth, "credit-earliest", "redeem-loopback")
            .await
            .unwrap_or_else(|error| panic!("loopback consume should parse: {error}"));
        let requests = server
            .join()
            .unwrap_or_else(|error| panic!("loopback server should join: {error:?}"));

        assert_eq!(weekly, Some(0));
        assert_eq!(credits[0].id, "credit-earliest");
        assert_eq!(consumed.code, ConsumeResetCreditCode::Reset);
        assert!(requests[0].starts_with("GET /api/codex/usage HTTP/1.1"));
        assert!(requests[1].starts_with("GET /api/codex/rate-limit-reset-credits HTTP/1.1"));
        assert!(
            requests[2].starts_with("POST /api/codex/rate-limit-reset-credits/consume HTTP/1.1")
        );
        assert!(requests.iter().all(|request| {
            request
                .to_ascii_lowercase()
                .contains("authorization: bearer loopback-token")
                && request
                    .to_ascii_lowercase()
                    .contains("chatgpt-account-id: chatgpt-loopback-account")
        }));
        assert!(requests[2].contains("\"credit_id\":\"credit-earliest\""));
        assert!(requests[2].contains("\"redeem_request_id\":\"redeem-loopback\""));
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
        let provider = HttpLiveQuotaResetProvider::new(format!("http://{source_address}"))
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

    fn serve_three_provider_requests(listener: TcpListener) -> Vec<String> {
        let bodies = [
            r#"{"rate_limit":{"primary_window":{"used_percent":10,"reset_at":1,"limit_window_seconds":18000},"secondary_window":{"used_percent":100,"reset_at":2,"limit_window_seconds":604800}},"additional_rate_limits":[]}"#,
            r#"{"credits":[{"id":"credit-earliest","status":"available","expires_at":"2026-07-14T00:00:00Z","title":"Weekly reset"}],"available_count":1}"#,
            r#"{"code":"reset","windows_reset":2}"#,
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
}

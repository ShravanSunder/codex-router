//! Live ChatGPT quota probe using Codex OAuth auth.json credentials.

use std::net::IpAddr;
use std::path::Path;
use std::time::Duration;

use serde::Deserialize;
use serde::Deserializer;
use thiserror::Error;

/// Default ChatGPT backend base used by Codex OAuth quota checks.
pub const DEFAULT_CHATGPT_BACKEND_BASE_URL: &str = "https://chatgpt.com/backend-api";

/// Policy for quota endpoint base URLs that receive OAuth bearer tokens.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuotaEndpointPolicy {
    /// Only the known HTTPS ChatGPT backend is allowed.
    ProviderOnly,
    /// Loopback HTTP is allowed for deterministic local tests and mocks.
    AllowLoopbackForTesting,
}

/// Creates the quota usage URL for a ChatGPT backend base URL.
#[must_use]
pub fn usage_url(base_url: &str) -> String {
    let base_url = base_url.trim_end_matches('/');
    if base_url.contains("/backend-api") {
        format!("{base_url}/wham/usage")
    } else {
        format!("{base_url}/api/codex/usage")
    }
}

/// Stored Codex auth secret shape.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct StoredAuth {
    auth_mode: Option<String>,
    tokens: Option<StoredTokens>,
    #[serde(rename = "OPENAI_API_KEY")]
    openai_api_key: Option<String>,
}

/// Stored OAuth token shape.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct StoredTokens {
    access_token: Option<String>,
}

/// Auth material usable for live quota calls.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UsageAuth {
    access_token: String,
}

impl UsageAuth {
    /// Creates usage auth from an already router-owned access token.
    pub fn from_access_token(access_token: impl Into<String>) -> Result<Self, LiveQuotaError> {
        let access_token = access_token.into();
        let access_token = access_token.trim();
        if access_token.is_empty() {
            return Err(LiveQuotaError::MissingAccessToken);
        }

        Ok(Self {
            access_token: access_token.to_owned(),
        })
    }

    /// Returns the bearer token for provider calls.
    #[must_use]
    pub fn access_token(&self) -> &str {
        &self.access_token
    }
}

/// Window usage from the ChatGPT usage endpoint.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct UsageWindow {
    /// Percent already used.
    pub used_percent: Option<i64>,
    /// Unix reset time.
    pub reset_at: Option<i64>,
    /// Provider window length in seconds.
    pub limit_window_seconds: Option<i64>,
}

/// Burn-down projection for one usage window.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UsageWindowBurnDown {
    /// Expected used percentage at this point in the reset window.
    pub expected_used_percent: i64,
    /// Actual used percentage minus expected used percentage.
    pub pace_delta_percent: i64,
    /// Projected time when this window reaches 100% at observed pace.
    pub runout_unix_seconds: Option<i64>,
    /// Whether projected runout happens before the provider reset.
    pub runout_before_reset: bool,
}

/// Primary and secondary quota windows.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct WindowPair {
    /// Shorter provider window.
    pub primary_window: Option<UsageWindow>,
    /// Longer provider window.
    pub secondary_window: Option<UsageWindow>,
}

/// ChatGPT usage response fields consumed by codex-router.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct UsageResponse {
    /// General Codex usage windows.
    pub rate_limit: Option<WindowPair>,
    /// Optional code-review usage windows.
    pub code_review_rate_limit: Option<WindowPair>,
    /// Other provider windows, such as workspace or monthly metered limits.
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub additional_rate_limits: Vec<AdditionalRateLimit>,
}

/// Additional provider quota family.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct AdditionalRateLimit {
    /// Human label supplied by the provider.
    pub limit_name: Option<String>,
    /// Provider metered feature, used as a fallback label.
    pub metered_feature: Option<String>,
    /// Quota windows for this provider limit.
    pub rate_limit: Option<WindowPair>,
}

/// Live quota auth or provider failure.
#[derive(Debug, Error)]
pub enum LiveQuotaError {
    /// Stored auth file could not be read.
    #[error("failed to read auth json: {message}")]
    ReadAuth {
        /// Redacted message.
        message: String,
    },
    /// Stored auth file was not valid JSON.
    #[error("failed to parse auth json: {message}")]
    ParseAuth {
        /// Redacted message.
        message: String,
    },
    /// API-key auth cannot call ChatGPT quota windows.
    #[error("quota endpoint requires Codex OAuth auth.json tokens, not API-key auth")]
    ApiKeyAuth,
    /// OAuth access token is absent.
    #[error("access token not found in auth json")]
    MissingAccessToken,
    /// HTTP client failed before a provider status could be read.
    #[error("quota request failed: {message}")]
    Request {
        /// Redacted message.
        message: String,
    },
    /// Provider returned a non-success status.
    #[error("quota endpoint returned HTTP {status}")]
    ProviderStatus {
        /// HTTP status code.
        status: u16,
    },
    /// Provider returned malformed JSON.
    #[error("quota endpoint returned invalid JSON: {message}")]
    ResponseJson {
        /// Redacted message.
        message: String,
    },
    /// Base URL could not be parsed.
    #[error("invalid quota base URL: {message}")]
    InvalidBaseUrl {
        /// Redacted parser message.
        message: String,
    },
    /// Base URL is not allowed to receive OAuth bearer tokens.
    #[error(
        "quota base URL must be https://chatgpt.com/backend-api unless explicit loopback testing is enabled"
    )]
    DisallowedBaseUrl,
}

/// Parses auth.json text into quota-compatible OAuth credentials.
pub fn usage_auth_from_auth_text(content: &str) -> Result<UsageAuth, LiveQuotaError> {
    let stored_auth: StoredAuth =
        serde_json::from_str(content).map_err(|error| LiveQuotaError::ParseAuth {
            message: error.to_string(),
        })?;
    usage_auth_from_stored_auth(&stored_auth)
}

/// Parses stored auth into quota-compatible OAuth credentials.
pub fn usage_auth_from_stored_auth(stored_auth: &StoredAuth) -> Result<UsageAuth, LiveQuotaError> {
    let has_api_key = stored_auth
        .openai_api_key
        .as_deref()
        .is_some_and(|key| !key.trim().is_empty());
    let auth_mode = stored_auth.auth_mode.as_deref().map(normalize_auth_mode);
    if auth_mode.as_deref() == Some("apikey") || has_api_key {
        return Err(LiveQuotaError::ApiKeyAuth);
    }

    let access_token = stored_auth
        .tokens
        .as_ref()
        .and_then(|tokens| tokens.access_token.as_deref())
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .ok_or(LiveQuotaError::MissingAccessToken)?
        .to_owned();

    Ok(UsageAuth { access_token })
}

fn normalize_auth_mode(value: &str) -> String {
    value
        .trim()
        .chars()
        .filter(|character| !matches!(character, '_' | '-' | ' '))
        .flat_map(char::to_lowercase)
        .collect()
}

/// Returns remaining percent for one provider quota window.
#[must_use]
pub fn usage_window_remaining_percent(window: Option<&UsageWindow>) -> Option<i64> {
    let window = window?;
    let used_percent = window.used_percent?;

    Some(100_i64.saturating_sub(used_percent).max(0))
}

/// Returns the routing headroom for a primary/secondary pair.
///
/// This is the bottleneck rule: when both short and long windows exist, the
/// usable headroom is the lower remaining percent.
#[must_use]
pub fn quota_pair_remaining_headroom(pair: Option<&WindowPair>) -> Option<i64> {
    let pair = pair?;
    let primary = usage_window_remaining_percent(pair.primary_window.as_ref());
    let secondary = usage_window_remaining_percent(pair.secondary_window.as_ref());
    match (primary, secondary) {
        (Some(primary), Some(secondary)) => Some(primary.min(secondary)),
        (Some(primary), None) => Some(primary),
        (None, Some(secondary)) => Some(secondary),
        (None, None) => None,
    }
}

/// Returns the window that currently constrains a quota pair.
#[must_use]
pub fn quota_pair_bottleneck_window(pair: Option<&WindowPair>) -> Option<&UsageWindow> {
    let pair = pair?;
    match (pair.primary_window.as_ref(), pair.secondary_window.as_ref()) {
        (Some(primary), Some(secondary)) => {
            let primary_remaining = usage_window_remaining_percent(Some(primary));
            let secondary_remaining = usage_window_remaining_percent(Some(secondary));
            match (primary_remaining, secondary_remaining) {
                (Some(primary_remaining), Some(secondary_remaining))
                    if primary_remaining < secondary_remaining =>
                {
                    Some(primary)
                }
                (Some(primary_remaining), Some(secondary_remaining))
                    if secondary_remaining < primary_remaining =>
                {
                    Some(secondary)
                }
                (Some(_), Some(_)) => earliest_reset_window(primary, secondary),
                (Some(_), None) => Some(primary),
                (None, Some(_)) => Some(secondary),
                (None, None) => None,
            }
        }
        (Some(primary), None) => Some(primary),
        (None, Some(secondary)) => Some(secondary),
        (None, None) => None,
    }
}

fn earliest_reset_window<'a>(
    left: &'a UsageWindow,
    right: &'a UsageWindow,
) -> Option<&'a UsageWindow> {
    match (left.reset_at, right.reset_at) {
        (Some(left_reset), Some(right_reset)) if right_reset < left_reset => Some(right),
        (Some(_), Some(_)) | (Some(_), None) | (None, None) => Some(left),
        (None, Some(_)) => Some(right),
    }
}

/// Computes pace and projected runout for one provider quota window.
#[must_use]
pub fn quota_window_burn_down(
    window: Option<&UsageWindow>,
    now_unix_seconds: i64,
) -> Option<UsageWindowBurnDown> {
    let window = window?;
    let used_percent = window.used_percent?.clamp(0, 100);
    let reset_at = window.reset_at?;
    let limit_window_seconds = window.limit_window_seconds?;
    if limit_window_seconds <= 0 {
        return None;
    }

    let window_start = reset_at.saturating_sub(limit_window_seconds);
    let elapsed_seconds = now_unix_seconds
        .saturating_sub(window_start)
        .clamp(0, limit_window_seconds);
    let expected_used_percent = elapsed_seconds.saturating_mul(100) / limit_window_seconds;
    let pace_delta_percent = used_percent.saturating_sub(expected_used_percent);
    let runout_unix_seconds =
        projected_runout_unix_seconds(now_unix_seconds, used_percent, elapsed_seconds);
    let runout_before_reset = runout_unix_seconds.is_some_and(|runout| runout <= reset_at);

    Some(UsageWindowBurnDown {
        expected_used_percent,
        pace_delta_percent,
        runout_unix_seconds,
        runout_before_reset,
    })
}

fn projected_runout_unix_seconds(
    now_unix_seconds: i64,
    used_percent: i64,
    elapsed_seconds: i64,
) -> Option<i64> {
    if used_percent >= 100 {
        return Some(now_unix_seconds);
    }
    if used_percent <= 0 || elapsed_seconds <= 0 {
        return None;
    }

    let remaining_percent = 100_i64.saturating_sub(used_percent);
    let seconds_to_runout = remaining_percent
        .saturating_mul(elapsed_seconds)
        .checked_div(used_percent)?;
    Some(now_unix_seconds.saturating_add(seconds_to_runout))
}

fn deserialize_null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

/// Blocking live quota client.
#[derive(Clone, Debug)]
pub struct LiveQuotaClient {
    client: reqwest::blocking::Client,
    base_url: String,
}

impl LiveQuotaClient {
    /// Creates a live quota client.
    pub fn new(base_url: impl Into<String>) -> Result<Self, LiveQuotaError> {
        Self::new_with_timeout(base_url, None)
    }

    /// Creates a live quota client with an optional request timeout.
    pub fn new_with_timeout(
        base_url: impl Into<String>,
        timeout: Option<Duration>,
    ) -> Result<Self, LiveQuotaError> {
        Self::new_with_timeout_and_policy(base_url, timeout, QuotaEndpointPolicy::ProviderOnly)
    }

    /// Creates a live quota client with an explicit endpoint policy.
    pub fn new_with_timeout_and_policy(
        base_url: impl Into<String>,
        timeout: Option<Duration>,
        endpoint_policy: QuotaEndpointPolicy,
    ) -> Result<Self, LiveQuotaError> {
        let base_url = base_url.into();
        validate_quota_base_url(&base_url, endpoint_policy)?;
        let mut builder =
            reqwest::blocking::Client::builder().user_agent("codex-router-live-quota");
        if let Some(timeout) = timeout {
            builder = builder.timeout(timeout);
        }
        let client = builder.build().map_err(|error| LiveQuotaError::Request {
            message: error.to_string(),
        })?;
        Ok(Self { client, base_url })
    }

    /// Reads auth JSON from disk and fetches live usage.
    pub fn fetch_from_auth_json(
        &self,
        auth_json_path: &Path,
    ) -> Result<UsageResponse, LiveQuotaError> {
        let content =
            std::fs::read_to_string(auth_json_path).map_err(|error| LiveQuotaError::ReadAuth {
                message: error.to_string(),
            })?;
        let auth = usage_auth_from_auth_text(&content)?;
        self.fetch(&auth)
    }

    /// Fetches live usage using already-parsed auth.
    pub fn fetch(&self, auth: &UsageAuth) -> Result<UsageResponse, LiveQuotaError> {
        let response = self
            .client
            .get(usage_url(&self.base_url))
            .bearer_auth(auth.access_token())
            .send()
            .map_err(|error| LiveQuotaError::Request {
                message: error.to_string(),
            })?;
        let status = response.status();
        if !status.is_success() {
            return Err(LiveQuotaError::ProviderStatus {
                status: status.as_u16(),
            });
        }
        let body = response.text().map_err(|error| LiveQuotaError::Request {
            message: error.to_string(),
        })?;
        serde_json::from_str::<UsageResponse>(&body).map_err(|error| LiveQuotaError::ResponseJson {
            message: error.to_string(),
        })
    }
}

fn validate_quota_base_url(
    base_url: &str,
    endpoint_policy: QuotaEndpointPolicy,
) -> Result<(), LiveQuotaError> {
    let parsed = reqwest::Url::parse(base_url).map_err(|error| LiveQuotaError::InvalidBaseUrl {
        message: error.to_string(),
    })?;
    let Some(host) = parsed.host_str() else {
        return Err(LiveQuotaError::DisallowedBaseUrl);
    };
    if endpoint_policy == QuotaEndpointPolicy::AllowLoopbackForTesting && is_loopback_host(host) {
        return Ok(());
    }
    if parsed.scheme() == "https" && host.eq_ignore_ascii_case("chatgpt.com") {
        return Ok(());
    }

    Err(LiveQuotaError::DisallowedBaseUrl)
}

fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

#[cfg(test)]
mod tests {
    use super::LiveQuotaClient;
    use super::LiveQuotaError;
    use super::QuotaEndpointPolicy;
    use super::usage_auth_from_auth_text;
    use super::usage_url;

    #[test]
    fn usage_url_matches_chatgpt_backend_shape() {
        assert_eq!(
            usage_url("https://chatgpt.com/backend-api"),
            "https://chatgpt.com/backend-api/wham/usage"
        );
        assert_eq!(
            usage_url("https://example.test"),
            "https://example.test/api/codex/usage"
        );
    }

    #[test]
    fn quota_client_rejects_non_provider_base_url_by_default() {
        let error = match LiveQuotaClient::new("http://127.0.0.1:8080") {
            Ok(client) => panic!("loopback HTTP should need explicit test policy: {client:?}"),
            Err(error) => error,
        };

        assert!(matches!(error, LiveQuotaError::DisallowedBaseUrl));
    }

    #[test]
    fn quota_client_allows_loopback_only_with_test_policy() {
        let client = LiveQuotaClient::new_with_timeout_and_policy(
            "http://127.0.0.1:8080",
            None,
            QuotaEndpointPolicy::AllowLoopbackForTesting,
        );

        match client {
            Ok(_client) => {}
            Err(error) => panic!("loopback should be allowed by explicit test policy: {error}"),
        }
    }

    #[test]
    fn quota_client_rejects_non_loopback_http_even_with_test_policy() {
        let error = match LiveQuotaClient::new_with_timeout_and_policy(
            "http://example.test",
            None,
            QuotaEndpointPolicy::AllowLoopbackForTesting,
        ) {
            Ok(client) => panic!("non-loopback HTTP should reject: {client:?}"),
            Err(error) => error,
        };

        assert!(matches!(error, LiveQuotaError::DisallowedBaseUrl));
    }

    #[test]
    fn usage_auth_rejects_api_key_auth_for_quota() {
        let error = match usage_auth_from_auth_text(
            r#"{"auth_mode":"api_key","OPENAI_API_KEY":"sk-test"}"#,
        ) {
            Ok(_) => panic!("api-key auth must not be quota-compatible"),
            Err(error) => error,
        };

        assert!(matches!(error, LiveQuotaError::ApiKeyAuth));
    }

    #[test]
    fn usage_auth_accepts_oauth_access_token() {
        let auth = match usage_auth_from_auth_text(
            r#"{"auth_mode":"chatgpt","tokens":{"access_token":" bearer-token "}}"#,
        ) {
            Ok(auth) => auth,
            Err(error) => panic!("oauth auth should parse: {error}"),
        };

        assert_eq!(auth.access_token(), "bearer-token");
    }
}

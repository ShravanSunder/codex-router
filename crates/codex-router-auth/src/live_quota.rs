//! Live ChatGPT quota probe using Codex OAuth auth.json credentials.

use std::path::Path;

use serde::Deserialize;
use thiserror::Error;

/// Default ChatGPT backend base used by Codex OAuth quota checks.
pub const DEFAULT_CHATGPT_BACKEND_BASE_URL: &str = "https://chatgpt.com/backend-api";

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

/// Creates the reset-credit URL for a ChatGPT backend base URL.
#[must_use]
pub fn reset_credits_url(base_url: &str) -> String {
    let base_url = base_url.trim_end_matches('/');
    if base_url.contains("/backend-api") {
        format!("{base_url}/wham/rate-limit-reset-credits")
    } else {
        format!("{base_url}/api/codex/rate-limit-reset-credits")
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
    /// Other provider windows, counted but not rendered verbatim.
    #[serde(default)]
    pub additional_rate_limits: Vec<serde_json::Value>,
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

/// Blocking live quota client.
#[derive(Clone, Debug)]
pub struct LiveQuotaClient {
    client: reqwest::blocking::Client,
    base_url: String,
}

impl LiveQuotaClient {
    /// Creates a live quota client.
    pub fn new(base_url: impl Into<String>) -> Result<Self, LiveQuotaError> {
        let client = reqwest::blocking::Client::builder()
            .user_agent("codex-router-live-quota")
            .build()
            .map_err(|error| LiveQuotaError::Request {
                message: error.to_string(),
            })?;
        Ok(Self {
            client,
            base_url: base_url.into(),
        })
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

#[cfg(test)]
mod tests {
    use super::LiveQuotaError;
    use super::reset_credits_url;
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
    fn reset_credits_url_matches_chatgpt_backend_shape() {
        assert_eq!(
            reset_credits_url("https://chatgpt.com/backend-api"),
            "https://chatgpt.com/backend-api/wham/rate-limit-reset-credits"
        );
        assert_eq!(
            reset_credits_url("https://example.test"),
            "https://example.test/api/codex/rate-limit-reset-credits"
        );
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

use super::claude_quota_fetcher::ClaudeQuotaFetcher;
use super::*;
use codex_router_core::provider::Provider;
use std::future::Future;

/// Quota provider request after provider credentials have been resolved.
pub struct QuotaRefreshProviderRequest {
    provider: Provider,
    account_id: AccountId,
    account_label: String,
    route_band: String,
    base_url: String,
    access_token: SecretString,
    chatgpt_account_id: Option<String>,
}

impl QuotaRefreshProviderRequest {
    pub fn new(
        account_id: AccountId,
        account_label: impl Into<String>,
        route_band: impl Into<String>,
        base_url: impl Into<String>,
        access_token: SecretString,
        chatgpt_account_id: Option<&str>,
    ) -> Self {
        Self {
            provider: Provider::Openai,
            account_id,
            account_label: account_label.into(),
            route_band: route_band.into(),
            base_url: base_url.into(),
            access_token,
            chatgpt_account_id: chatgpt_account_id.map(str::to_owned),
        }
    }

    pub fn new_for_provider(
        provider: Provider,
        account_id: AccountId,
        account_label: impl Into<String>,
        route_band: impl Into<String>,
        base_url: impl Into<String>,
        access_token: SecretString,
        chatgpt_account_id: Option<&str>,
    ) -> Self {
        let mut request = Self::new(
            account_id,
            account_label,
            route_band,
            base_url,
            access_token,
            chatgpt_account_id,
        );
        request.provider = provider;
        request
    }

    #[must_use]
    pub const fn provider(&self) -> Provider {
        self.provider
    }

    /// Returns the account id.
    #[must_use]
    pub const fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    /// Returns the account label.
    #[must_use]
    pub fn account_label(&self) -> &str {
        &self.account_label
    }

    /// Returns the route band.
    #[must_use]
    pub fn route_band(&self) -> &str {
        &self.route_band
    }

    /// Returns the provider base URL.
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Returns the provider bearer token.
    #[must_use]
    pub const fn access_token(&self) -> &SecretString {
        &self.access_token
    }

    /// Returns the ChatGPT account id header value, if known.
    #[must_use]
    pub fn chatgpt_account_id(&self) -> Option<&str> {
        self.chatgpt_account_id.as_deref()
    }
}

/// Quota provider response.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QuotaRefreshProviderResponse {
    pub windows: Vec<QuotaRefreshProviderWindow>,
    pub reset_credits_available: Option<u32>,
    pub credit_provider_observation: CreditProviderObservation,
}

impl QuotaRefreshProviderResponse {
    fn without_credit_facts() -> Self {
        Self {
            windows: Vec::new(),
            reset_credits_available: None,
            credit_provider_observation: CreditProviderObservation::missing(),
        }
    }

    fn with_credit_facts_from_usage(mut self, usage: &UsageResponse) -> Self {
        self.credit_provider_observation = usage.credit_provider_observation();
        self
    }

    pub(super) fn effective_window(&self) -> Option<&QuotaRefreshProviderWindow> {
        self.windows
            .iter()
            .find(|window| window.effective)
            .or_else(|| self.windows.first())
    }
}

impl Default for QuotaRefreshProviderResponse {
    fn default() -> Self {
        Self::without_credit_facts()
    }
}

/// Quota provider response for one limit window.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QuotaRefreshProviderWindow {
    pub limit_window_seconds: u64,
    pub headroom: QuotaWindowHeadroom,
    pub reset_unix_seconds: Option<u64>,
    pub effective: bool,
}

/// Explicit units for the remaining usage reported by different providers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuotaWindowHeadroom {
    Percent(u32),
    BasisPoints(u32),
}

impl QuotaWindowHeadroom {
    pub const fn percent(self) -> Option<u32> {
        match self {
            Self::Percent(value) => Some(value),
            Self::BasisPoints(_) => None,
        }
    }

    pub const fn basis_points(self) -> Option<u32> {
        match self {
            Self::Percent(_) => None,
            Self::BasisPoints(value) => Some(value),
        }
    }
}

/// Provider egress dependency for quota refresh.
pub trait QuotaRefreshProvider {
    /// Fetches one route-band quota snapshot using resolved provider auth.
    fn fetch_quota(
        &self,
        request: QuotaRefreshProviderRequest,
    ) -> impl Future<Output = Result<QuotaRefreshProviderResponse, QuotaRefreshError>> + Send;
}

/// HTTP quota refresh provider for ChatGPT/Codex usage endpoints.
#[derive(Debug)]
pub struct HttpQuotaRefreshProvider {
    client: reqwest::Client,
    claude_quota_fetcher: ClaudeQuotaFetcher,
}

impl HttpQuotaRefreshProvider {
    /// Creates an HTTP quota refresh provider.
    pub fn new() -> Result<Self, QuotaRefreshError> {
        Self::new_with_timeout(Duration::from_secs(30))
    }

    /// Creates an HTTP quota refresh provider with a bounded request timeout.
    pub fn new_with_timeout(timeout: Duration) -> Result<Self, QuotaRefreshError> {
        let client = reqwest::Client::builder()
            .user_agent("codex-router-quota-refresh")
            .timeout(timeout)
            .build()
            .map_err(|error| QuotaRefreshError::ProviderRequest {
                message: error.to_string(),
            })?;
        Ok(Self {
            claude_quota_fetcher: ClaudeQuotaFetcher::new(client.clone()),
            client,
        })
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn new_with_claude_usage_endpoint_for_test(
        timeout: Duration,
        usage_endpoint: impl Into<String>,
    ) -> Result<Self, QuotaRefreshError> {
        let client = reqwest::Client::builder()
            .user_agent("codex-router-quota-refresh")
            .timeout(timeout)
            .build()
            .map_err(|error| QuotaRefreshError::ProviderRequest {
                message: error.to_string(),
            })?;
        Ok(Self {
            claude_quota_fetcher: ClaudeQuotaFetcher::new_with_endpoint_for_test(
                timeout,
                usage_endpoint,
            )?,
            client,
        })
    }
}

impl QuotaRefreshProvider for HttpQuotaRefreshProvider {
    async fn fetch_quota(
        &self,
        request: QuotaRefreshProviderRequest,
    ) -> Result<QuotaRefreshProviderResponse, QuotaRefreshError> {
        if request.provider() == Provider::Claude {
            return self.claude_quota_fetcher.fetch_quota(request).await;
        }
        let _account_context = (request.account_id(), request.account_label());
        let mut usage_request = self
            .client
            .get(usage_url(request.base_url()))
            .bearer_auth(request.access_token().expose_secret());
        if let Some(chatgpt_account_id) = request.chatgpt_account_id() {
            usage_request = usage_request.header("ChatGPT-Account-ID", chatgpt_account_id);
        }
        let response =
            usage_request
                .send()
                .await
                .map_err(|error| QuotaRefreshError::ProviderRequest {
                    message: error.to_string(),
                })?;
        let status = response.status();
        if !status.is_success() {
            return Err(QuotaRefreshError::ProviderStatus {
                status: status.as_u16(),
            });
        }
        let body = response
            .text()
            .await
            .map_err(|error| QuotaRefreshError::ProviderRequest {
                message: error.to_string(),
            })?;
        let usage_value = serde_json::from_str::<Value>(&body).map_err(|error| {
            QuotaRefreshError::ProviderResponse {
                message: error.to_string(),
            }
        })?;
        let usage = serde_json::from_value::<UsageResponse>(usage_value).map_err(|error| {
            QuotaRefreshError::ProviderResponse {
                message: error.to_string(),
            }
        })?;
        let reset_credits_available = self.fetch_reset_credits_available(&request).await?;
        quota_response_for_route_band(&usage, request.route_band()).map(|mut response| {
            response.reset_credits_available = reset_credits_available;
            response
        })
    }
}

impl HttpQuotaRefreshProvider {
    async fn fetch_reset_credits_available(
        &self,
        request: &QuotaRefreshProviderRequest,
    ) -> Result<Option<u32>, QuotaRefreshError> {
        let mut reset_request = self
            .client
            .get(reset_credits_url(request.base_url()))
            .bearer_auth(request.access_token().expose_secret());
        if let Some(chatgpt_account_id) = request.chatgpt_account_id() {
            reset_request = reset_request.header("ChatGPT-Account-ID", chatgpt_account_id);
        }
        let response =
            reset_request
                .send()
                .await
                .map_err(|error| QuotaRefreshError::ProviderRequest {
                    message: error.to_string(),
                })?;
        let status = response.status();
        if !status.is_success() {
            return Err(QuotaRefreshError::ProviderStatus {
                status: status.as_u16(),
            });
        }
        let body = response
            .text()
            .await
            .map_err(|error| QuotaRefreshError::ProviderRequest {
                message: error.to_string(),
            })?;
        let value = serde_json::from_str::<Value>(&body).map_err(|error| {
            QuotaRefreshError::ProviderResponse {
                message: error.to_string(),
            }
        })?;
        Ok(reset_credits_available_from_json(&value))
    }
}

pub fn quota_response_for_route_band(
    usage: &UsageResponse,
    route_band: &str,
) -> Result<QuotaRefreshProviderResponse, QuotaRefreshError> {
    let mut response = if route_band == "code_review" {
        let window_pair = usage.code_review_rate_limit.as_ref().ok_or_else(|| {
            QuotaRefreshError::ProviderResponse {
                message: format!("missing quota window for route band {route_band}"),
            }
        })?;
        quota_response_from_window_pair(window_pair, route_band)?
    } else {
        let window_pair =
            usage
                .rate_limit
                .as_ref()
                .ok_or_else(|| QuotaRefreshError::ProviderResponse {
                    message: format!("missing quota window for route band {route_band}"),
                })?;
        quota_response_from_window_pair(window_pair, route_band)?
    };

    if route_band == "responses" {
        response = response.with_credit_facts_from_usage(usage);
    }
    Ok(response)
}

pub const fn stale_after_unix_seconds(observed_unix_seconds: u64) -> u64 {
    observed_unix_seconds.saturating_add(DEFAULT_REFRESH_STALE_AFTER_GRACE_SECONDS)
}

fn quota_response_from_window_pair(
    window_pair: &WindowPair,
    route_band: &str,
) -> Result<QuotaRefreshProviderResponse, QuotaRefreshError> {
    let mut windows = Vec::new();
    if let Some(primary_window) = window_pair.primary_window.as_ref() {
        windows.push(quota_provider_window_from_usage_window(
            primary_window,
            route_band,
            true,
        )?);
    }
    if let Some(secondary_window) = window_pair.secondary_window.as_ref() {
        windows.push(quota_provider_window_from_usage_window(
            secondary_window,
            route_band,
            window_pair.primary_window.is_none(),
        )?);
    }
    if windows.is_empty() {
        return Err(QuotaRefreshError::ProviderResponse {
            message: format!("missing provider quota windows for route band {route_band}"),
        });
    }

    Ok(QuotaRefreshProviderResponse {
        windows,
        reset_credits_available: None,
        ..QuotaRefreshProviderResponse::default()
    })
}

fn reset_credits_available_from_json(value: &Value) -> Option<u32> {
    match value {
        Value::Object(object) => {
            for (key, child) in object {
                let normalized_key = normalize_json_key(key);
                if matches!(
                    normalized_key.as_str(),
                    "resetcreditsavailable" | "availableresetcredits" | "availablecount"
                ) && let Some(value) = json_u32(child)
                {
                    return Some(value);
                }
                if normalized_key == "resetcredits"
                    && let Some(value) = reset_credits_available_from_reset_credits_value(child)
                {
                    return Some(value);
                }
            }
            object.values().find_map(reset_credits_available_from_json)
        }
        Value::Array(values) => values.iter().find_map(reset_credits_available_from_json),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => None,
    }
}

fn reset_credits_available_from_reset_credits_value(value: &Value) -> Option<u32> {
    match value {
        Value::Number(_) | Value::String(_) => json_u32(value),
        Value::Object(object) => object.iter().find_map(|(key, child)| {
            let normalized_key = normalize_json_key(key);
            if matches!(normalized_key.as_str(), "available" | "remaining" | "count") {
                json_u32(child)
            } else {
                reset_credits_available_from_reset_credits_value(child)
            }
        }),
        Value::Array(values) => values
            .iter()
            .find_map(reset_credits_available_from_reset_credits_value),
        Value::Null | Value::Bool(_) => None,
    }
}

fn normalize_json_key(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn json_u32(value: &Value) -> Option<u32> {
    match value {
        Value::Number(number) => number.as_u64().and_then(|value| u32::try_from(value).ok()),
        Value::String(value) => value.trim().parse::<u32>().ok(),
        Value::Null | Value::Bool(_) | Value::Array(_) | Value::Object(_) => None,
    }
}

fn quota_provider_window_from_usage_window(
    window: &codex_router_auth::live_quota::UsageWindow,
    route_band: &str,
    effective: bool,
) -> Result<QuotaRefreshProviderWindow, QuotaRefreshError> {
    let used_percent = window
        .used_percent
        .ok_or_else(|| QuotaRefreshError::ProviderResponse {
            message: format!("missing used_percent for route band {route_band}"),
        })?
        .clamp(0, 100);
    let remaining_headroom = u32::try_from(100_i64 - used_percent).map_err(|_error| {
        QuotaRefreshError::ProviderResponse {
            message: format!("invalid used_percent for route band {route_band}"),
        }
    })?;
    let limit_window_seconds = window
        .limit_window_seconds
        .and_then(|limit_window_seconds| u64::try_from(limit_window_seconds).ok())
        .ok_or_else(|| QuotaRefreshError::ProviderResponse {
            message: format!("missing limit_window_seconds for route band {route_band}"),
        })?;
    let reset_unix_seconds = window
        .reset_at
        .and_then(|reset_at| u64::try_from(reset_at).ok());

    Ok(QuotaRefreshProviderWindow {
        limit_window_seconds,
        headroom: QuotaWindowHeadroom::Percent(remaining_headroom),
        reset_unix_seconds,
        effective,
    })
}

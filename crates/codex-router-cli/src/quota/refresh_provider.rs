use super::*;

/// Quota provider request after provider credentials have been resolved.
pub(crate) struct QuotaRefreshProviderRequest {
    account_id: AccountId,
    account_label: String,
    route_band: String,
    base_url: String,
    access_token: SecretString,
    chatgpt_account_id: Option<String>,
}

impl QuotaRefreshProviderRequest {
    pub(crate) fn new(
        account_id: AccountId,
        account_label: impl Into<String>,
        route_band: impl Into<String>,
        base_url: impl Into<String>,
        access_token: SecretString,
        chatgpt_account_id: Option<&str>,
    ) -> Self {
        Self {
            account_id,
            account_label: account_label.into(),
            route_band: route_band.into(),
            base_url: base_url.into(),
            access_token,
            chatgpt_account_id: chatgpt_account_id.map(str::to_owned),
        }
    }

    /// Returns the account id.
    #[must_use]
    pub(crate) const fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    /// Returns the account label.
    #[must_use]
    pub(crate) fn account_label(&self) -> &str {
        &self.account_label
    }

    /// Returns the route band.
    #[must_use]
    pub(crate) fn route_band(&self) -> &str {
        &self.route_band
    }

    /// Returns the provider base URL.
    #[must_use]
    pub(crate) fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Returns the provider bearer token.
    #[must_use]
    pub(crate) const fn access_token(&self) -> &SecretString {
        &self.access_token
    }

    /// Returns the ChatGPT account id header value, if known.
    #[must_use]
    pub(crate) fn chatgpt_account_id(&self) -> Option<&str> {
        self.chatgpt_account_id.as_deref()
    }
}

/// Quota provider response.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct QuotaRefreshProviderResponse {
    pub(crate) windows: Vec<QuotaRefreshProviderWindow>,
    pub(crate) reset_credits_available: Option<u32>,
}

impl QuotaRefreshProviderResponse {
    pub(super) fn effective_window(&self) -> Option<&QuotaRefreshProviderWindow> {
        self.windows
            .iter()
            .find(|window| window.effective)
            .or_else(|| self.windows.first())
    }
}

/// Quota provider response for one limit window.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct QuotaRefreshProviderWindow {
    pub(crate) limit_window_seconds: u64,
    pub(crate) remaining_headroom: u32,
    pub(crate) reset_unix_seconds: Option<u64>,
    pub(crate) effective: bool,
}

/// Provider egress dependency for quota refresh.
pub(crate) trait QuotaRefreshProvider {
    /// Fetches one route-band quota snapshot using resolved provider auth.
    async fn fetch_quota(
        &self,
        request: QuotaRefreshProviderRequest,
    ) -> Result<QuotaRefreshProviderResponse, QuotaCommandError>;
}

/// HTTP quota refresh provider for ChatGPT/Codex usage endpoints.
#[derive(Debug)]
pub(crate) struct HttpQuotaRefreshProvider {
    client: reqwest::Client,
}

impl HttpQuotaRefreshProvider {
    /// Creates an HTTP quota refresh provider.
    pub(crate) fn new() -> Result<Self, QuotaCommandError> {
        Self::new_with_timeout(Duration::from_secs(30))
    }

    /// Creates an HTTP quota refresh provider with a bounded request timeout.
    pub(crate) fn new_with_timeout(timeout: Duration) -> Result<Self, QuotaCommandError> {
        let client = reqwest::Client::builder()
            .user_agent("codex-router-quota-refresh")
            .timeout(timeout)
            .build()
            .map_err(|error| QuotaCommandError::ProviderRequest {
                message: error.to_string(),
            })?;
        Ok(Self { client })
    }
}

impl QuotaRefreshProvider for HttpQuotaRefreshProvider {
    async fn fetch_quota(
        &self,
        request: QuotaRefreshProviderRequest,
    ) -> Result<QuotaRefreshProviderResponse, QuotaCommandError> {
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
        let usage_value = serde_json::from_str::<Value>(&body).map_err(|error| {
            QuotaCommandError::ProviderResponse {
                message: error.to_string(),
            }
        })?;
        let usage = serde_json::from_value::<UsageResponse>(usage_value).map_err(|error| {
            QuotaCommandError::ProviderResponse {
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
    ) -> Result<Option<u32>, QuotaCommandError> {
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
        let value = serde_json::from_str::<Value>(&body).map_err(|error| {
            QuotaCommandError::ProviderResponse {
                message: error.to_string(),
            }
        })?;
        Ok(reset_credits_available_from_json(&value))
    }
}

pub(super) fn quota_response_for_route_band(
    usage: &UsageResponse,
    route_band: &str,
) -> Result<QuotaRefreshProviderResponse, QuotaCommandError> {
    if route_band == "code_review" {
        let window_pair = usage.code_review_rate_limit.as_ref().ok_or_else(|| {
            QuotaCommandError::ProviderResponse {
                message: format!("missing quota window for route band {route_band}"),
            }
        })?;
        return quota_response_from_window_pair(window_pair, route_band);
    }

    let window_pair =
        usage
            .rate_limit
            .as_ref()
            .ok_or_else(|| QuotaCommandError::ProviderResponse {
                message: format!("missing quota window for route band {route_band}"),
            })?;
    quota_response_from_window_pair(window_pair, route_band)
}

pub(super) const fn stale_after_unix_seconds(observed_unix_seconds: u64) -> u64 {
    observed_unix_seconds.saturating_add(DEFAULT_REFRESH_STALE_AFTER_GRACE_SECONDS)
}

fn quota_response_from_window_pair(
    window_pair: &WindowPair,
    route_band: &str,
) -> Result<QuotaRefreshProviderResponse, QuotaCommandError> {
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
        return Err(QuotaCommandError::ProviderResponse {
            message: format!("missing provider quota windows for route band {route_band}"),
        });
    }

    Ok(QuotaRefreshProviderResponse {
        windows,
        reset_credits_available: None,
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
) -> Result<QuotaRefreshProviderWindow, QuotaCommandError> {
    let used_percent = window
        .used_percent
        .ok_or_else(|| QuotaCommandError::ProviderResponse {
            message: format!("missing used_percent for route band {route_band}"),
        })?
        .clamp(0, 100);
    let remaining_headroom = u32::try_from(100_i64 - used_percent).map_err(|_error| {
        QuotaCommandError::ProviderResponse {
            message: format!("invalid used_percent for route band {route_band}"),
        }
    })?;
    let limit_window_seconds = window
        .limit_window_seconds
        .and_then(|limit_window_seconds| u64::try_from(limit_window_seconds).ok())
        .ok_or_else(|| QuotaCommandError::ProviderResponse {
            message: format!("missing limit_window_seconds for route band {route_band}"),
        })?;
    let reset_unix_seconds = window
        .reset_at
        .and_then(|reset_at| u64::try_from(reset_at).ok());

    Ok(QuotaRefreshProviderWindow {
        limit_window_seconds,
        remaining_headroom,
        reset_unix_seconds,
        effective,
    })
}

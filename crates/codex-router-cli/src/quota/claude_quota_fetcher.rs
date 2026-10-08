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
    let mut structured_five_hour = None;
    let mut structured_weekly = None;
    for limit in usage.limits.unwrap_or_default() {
        match limit.kind.as_str() {
            "session" if structured_five_hour.is_none() => {
                structured_five_hour = Some(parse_limit_window(&limit)?);
            }
            "weekly_all" if structured_weekly.is_none() => {
                structured_weekly = Some(parse_limit_window(&limit)?);
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

    let five_hour = match structured_five_hour {
        Some(window) => Some(window),
        None => usage
            .five_hour
            .map(parse_legacy_window)
            .transpose()?
            .flatten(),
    };
    let weekly = match structured_weekly {
        Some(window) => Some(window),
        None => usage
            .seven_day
            .map(parse_legacy_window)
            .transpose()?
            .flatten(),
    };

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
        credit_provider_observation:
            codex_router_core::credit_usage::CreditProviderObservation::missing(),
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
#[path = "claude_quota_fetcher_tests.rs"]
mod tests;

//! Live ChatGPT quota probe using Codex OAuth auth.json credentials.

use std::path::Path;

use codex_router_core::credit_usage::CreditAvailability;
use codex_router_core::credit_usage::CreditBalance;
use codex_router_core::credit_usage::CreditProviderLimitReason;
use codex_router_core::credit_usage::CreditProviderObservation;
use codex_router_core::credit_usage::CreditSpendControl;
use serde::Deserialize;
use serde::Deserializer;
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
    /// Independently metered provider windows.
    #[serde(default, deserialize_with = "deserialize_additional_rate_limits")]
    pub additional_rate_limits: Vec<serde_json::Value>,
    /// Partial provider credit envelope, validated independently from quota windows.
    #[serde(default)]
    credits: Option<serde_json::Value>,
    /// Partial provider spend-control envelope, validated independently from quota windows.
    #[serde(default)]
    spend_control: Option<serde_json::Value>,
    /// Top-level closed provider reason, validated independently from quota windows.
    #[serde(default)]
    rate_limit_reached_type: Option<serde_json::Value>,
}

impl UsageResponse {
    /// Returns the validated credit facts from this usage response.
    #[must_use]
    pub fn credit_provider_observation(&self) -> CreditProviderObservation {
        CreditProviderObservation::new(
            self.credit_availability(),
            self.credit_spend_control(),
            self.credit_provider_limit_reason(),
        )
    }

    fn credit_availability(&self) -> CreditAvailability {
        credit_availability_from_json(self.credits.as_ref())
    }

    fn credit_spend_control(&self) -> CreditSpendControl {
        credit_spend_control_from_json(self.spend_control.as_ref())
    }

    fn credit_provider_limit_reason(&self) -> Option<CreditProviderLimitReason> {
        credit_provider_limit_reason_from_json(self.rate_limit_reached_type.as_ref())
    }
}

fn credit_availability_from_json(value: Option<&serde_json::Value>) -> CreditAvailability {
    let Some(credit_object) = value.and_then(serde_json::Value::as_object) else {
        return CreditAvailability::Unknown;
    };
    let Some(has_credits) = credit_object
        .get("has_credits")
        .and_then(serde_json::Value::as_bool)
    else {
        return CreditAvailability::Unknown;
    };
    let Some(unlimited) = credit_object
        .get("unlimited")
        .and_then(serde_json::Value::as_bool)
    else {
        return CreditAvailability::Unknown;
    };
    let balance = match credit_object.get("balance") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(balance)) => match CreditBalance::new(balance.clone()) {
            Ok(balance) => Some(balance),
            Err(_) => return CreditAvailability::Unknown,
        },
        Some(_) => return CreditAvailability::Unknown,
    };

    if unlimited {
        return CreditAvailability::Unlimited;
    }

    match (has_credits, balance) {
        (true, balance) => CreditAvailability::Available { balance },
        (false, None) => CreditAvailability::Depleted,
        (false, Some(balance)) if !balance.is_positive() => CreditAvailability::Depleted,
        (false, Some(_)) => CreditAvailability::Unknown,
    }
}

fn credit_spend_control_from_json(value: Option<&serde_json::Value>) -> CreditSpendControl {
    let Some(value) = value else {
        return CreditSpendControl::Unreported;
    };
    if value.is_null() {
        return CreditSpendControl::Unreported;
    }
    let Some(spend_control) = value.as_object() else {
        return CreditSpendControl::Unknown;
    };

    match spend_control.get("reached") {
        Some(serde_json::Value::Bool(false)) => CreditSpendControl::Clear,
        Some(serde_json::Value::Bool(true)) => CreditSpendControl::Reached,
        Some(_) => CreditSpendControl::Unknown,
        None => CreditSpendControl::Unknown,
    }
}

fn credit_provider_limit_reason_from_json(
    value: Option<&serde_json::Value>,
) -> Option<CreditProviderLimitReason> {
    match value {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::Object(object)) => Some(
            object
                .get("type")
                .and_then(serde_json::Value::as_str)
                .and_then(CreditProviderLimitReason::parse)
                .unwrap_or(CreditProviderLimitReason::Unknown),
        ),
        Some(_) => Some(CreditProviderLimitReason::Unknown),
    }
}

fn deserialize_additional_rate_limits<'de, D>(
    deserializer: D,
) -> Result<Vec<serde_json::Value>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Option::<Vec<serde_json::Value>>::deserialize(deserializer)?.unwrap_or_default())
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
    use codex_router_core::credit_usage::CreditAvailability;
    use codex_router_core::credit_usage::CreditBalance;
    use codex_router_core::credit_usage::CreditProviderLimitReason;
    use codex_router_core::credit_usage::CreditSpendControl;

    use super::LiveQuotaError;
    use super::UsageResponse;
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

    #[test]
    fn usage_response_accepts_null_additional_rate_limits() {
        let response: UsageResponse =
            match serde_json::from_str(r#"{"rate_limit":null,"additional_rate_limits":null}"#) {
                Ok(response) => response,
                Err(error) => panic!("null additional rate limits should deserialize: {error}"),
            };

        assert!(response.additional_rate_limits.is_empty());
        assert_eq!(response.credit_availability(), CreditAvailability::Unknown);
        assert_eq!(
            response.credit_spend_control(),
            CreditSpendControl::Unreported
        );
        assert_eq!(response.credit_provider_limit_reason(), None);
    }

    #[test]
    fn usage_response_keeps_canonical_quota_when_additional_meter_shape_drifts() {
        let response: UsageResponse = serde_json::from_str(
            r#"{
                "rate_limit": {
                    "primary_window": null,
                    "secondary_window": {
                        "used_percent": 20,
                        "reset_at": 9000,
                        "limit_window_seconds": 604800
                    }
                },
                "additional_rate_limits": [null, 42, {"unexpected": true}]
            }"#,
        )
        .unwrap_or_else(|error| {
            panic!("independent meter drift must not veto canonical quota: {error}")
        });

        assert!(response.rate_limit.is_some());
        assert_eq!(response.additional_rate_limits.len(), 3);
    }

    #[test]
    fn usage_response_classifies_credit_and_spend_control_facts() {
        let response: UsageResponse = serde_json::from_str(
            r#"{
                "credits": {"has_credits": true, "unlimited": false},
                "spend_control": {"reached": true},
                "rate_limit_reached_type": {"type": "workspace_owner_credits_depleted"}
            }"#,
        )
        .expect("known credit facts should deserialize");

        assert_eq!(
            response.credit_availability(),
            CreditAvailability::Available { balance: None },
            "provider has_credits remains distinct from a hidden numeric balance"
        );
        assert_eq!(response.credit_spend_control(), CreditSpendControl::Reached);
        assert_eq!(
            response.credit_provider_limit_reason(),
            Some(CreditProviderLimitReason::WorkspaceOwnerCreditsDepleted)
        );
        assert!(response.credit_spend_control().blocks_credit_usage());
        assert!(
            response
                .credit_provider_limit_reason()
                .is_some_and(CreditProviderLimitReason::blocks_credit_usage)
        );
    }

    #[test]
    fn spend_control_distinguishes_unreported_clear_and_malformed_values() {
        let cases = [
            (r#"{}"#, CreditSpendControl::Unreported),
            (r#"{"spend_control":null}"#, CreditSpendControl::Unreported),
            (
                r#"{"spend_control":{"reached":false}}"#,
                CreditSpendControl::Clear,
            ),
            (
                r#"{"spend_control":{"reached":true}}"#,
                CreditSpendControl::Reached,
            ),
            (r#"{"spend_control":{}}"#, CreditSpendControl::Unknown),
            (
                r#"{"spend_control":{"reached":null}}"#,
                CreditSpendControl::Unknown,
            ),
        ];

        for (json, expected_control) in cases {
            let response: UsageResponse =
                serde_json::from_str(json).expect("partial provider envelope should deserialize");
            assert_eq!(response.credit_spend_control(), expected_control, "{json}");
        }
    }

    #[test]
    fn top_level_provider_credit_rejections_override_absent_or_clear_spend_control() {
        let rejection_reasons = [
            (
                "workspace_owner_credits_depleted",
                CreditProviderLimitReason::WorkspaceOwnerCreditsDepleted,
            ),
            (
                "workspace_member_credits_depleted",
                CreditProviderLimitReason::WorkspaceMemberCreditsDepleted,
            ),
            (
                "workspace_owner_usage_limit_reached",
                CreditProviderLimitReason::WorkspaceOwnerUsageLimitReached,
            ),
            (
                "workspace_member_usage_limit_reached",
                CreditProviderLimitReason::WorkspaceMemberUsageLimitReached,
            ),
        ];
        let spend_control_shapes = ["", r#", "spend_control":{"reached":false}"#];

        for (reason_name, expected_reason) in rejection_reasons {
            for spend_control_json in spend_control_shapes {
                let json = format!(
                    r#"{{"credits":{{"has_credits":true,"unlimited":false,"balance":"3.50"}},"rate_limit_reached_type":{{"type":"{reason_name}"}}{spend_control_json}}}"#
                );
                let response: UsageResponse =
                    serde_json::from_str(&json).expect("actual provider schema should deserialize");
                assert!(response.credit_availability().can_authorize_spending());
                assert_eq!(
                    response.credit_provider_limit_reason(),
                    Some(expected_reason),
                    "{json}"
                );
                assert!(
                    response
                        .credit_provider_limit_reason()
                        .is_some_and(CreditProviderLimitReason::blocks_credit_usage),
                    "provider credit rejection must block with spend_control {spend_control_json:?}"
                );
            }
        }
    }

    #[test]
    fn ordinary_quota_exhaustion_allows_credits_and_unknown_reasons_fail_closed() {
        let ordinary_exhaustion: UsageResponse = serde_json::from_str(
            r#"{
                "credits": {"has_credits": true, "unlimited": false, "balance": "0.25"},
                "rate_limit_reached_type": {"type": "rate_limit_reached"}
            }"#,
        )
        .expect("ordinary quota exhaustion response should parse");
        assert!(
            ordinary_exhaustion
                .credit_availability()
                .can_authorize_spending()
        );
        assert_eq!(
            ordinary_exhaustion.credit_provider_limit_reason(),
            Some(CreditProviderLimitReason::RateLimitReached)
        );
        assert!(
            !ordinary_exhaustion
                .credit_provider_limit_reason()
                .is_some_and(CreditProviderLimitReason::blocks_credit_usage)
        );

        for json in [
            r#"{"rate_limit_reached_type":{"type":"future_reason"}}"#,
            r#"{"rate_limit_reached_type":{}}"#,
            r#"{"rate_limit_reached_type":true}"#,
        ] {
            let response: UsageResponse =
                serde_json::from_str(json).expect("unknown reason shape should not erase quota");
            assert_eq!(
                response.credit_provider_limit_reason(),
                Some(CreditProviderLimitReason::Unknown),
                "{json}"
            );
            assert!(
                response
                    .credit_provider_limit_reason()
                    .is_some_and(CreditProviderLimitReason::blocks_credit_usage),
                "unknown non-null provider reason must fail closed"
            );
        }
    }

    #[test]
    fn malformed_credit_facts_fail_closed_without_erasing_quota() {
        let response: UsageResponse = serde_json::from_str(
            r#"{
                "rate_limit": {"primary_window": null, "secondary_window": null},
                "credits": {"has_credits": true, "unlimited": false, "balance": 1.25},
                "spend_control": {"reached": "true"}
            }"#,
        )
        .expect("credit-field drift must not erase canonical quota");

        assert!(response.rate_limit.is_some());
        assert_eq!(response.credit_availability(), CreditAvailability::Unknown);
        assert_eq!(response.credit_spend_control(), CreditSpendControl::Unknown);

        let malformed_unlimited: UsageResponse = serde_json::from_str(
            r#"{"credits":{"has_credits":false,"unlimited":true,"balance":1.25}}"#,
        )
        .expect("malformed credit details must not erase the usage response");
        assert_eq!(
            malformed_unlimited.credit_availability(),
            CreditAvailability::Unknown,
            "unlimited does not override a malformed balance field"
        );
    }

    #[test]
    fn usage_response_classifies_hidden_balance_unlimited_depleted_and_zero() {
        let cases = [
            (
                r#"{"credits":{"has_credits":true,"unlimited":false,"balance":"7.2500"}}"#,
                CreditAvailability::Available {
                    balance: Some(CreditBalance::new("7.2500").expect("valid decimal")),
                },
            ),
            (
                r#"{"credits":{"has_credits":true,"unlimited":true,"balance":null}}"#,
                CreditAvailability::Unlimited,
            ),
            (
                r#"{"credits":{"has_credits":false,"unlimited":true,"balance":null}}"#,
                CreditAvailability::Unlimited,
            ),
            (
                r#"{"credits":{"has_credits":false,"unlimited":true,"balance":"0"}}"#,
                CreditAvailability::Unlimited,
            ),
            (
                r#"{"credits":{"has_credits":true,"unlimited":true,"balance":"0"}}"#,
                CreditAvailability::Unlimited,
            ),
            (
                r#"{"credits":{"has_credits":false,"unlimited":false,"balance":"0"}}"#,
                CreditAvailability::Depleted,
            ),
            (
                r#"{"credits":{"has_credits":true,"unlimited":false,"balance":"0.000"}}"#,
                CreditAvailability::Available {
                    balance: Some(CreditBalance::new("0.000").expect("valid zero")),
                },
            ),
        ];

        for (json, expected_availability) in cases {
            let response: UsageResponse = serde_json::from_str(json)
                .expect("each credit representation should preserve the response envelope");
            assert_eq!(response.credit_availability(), expected_availability);
        }
    }
}

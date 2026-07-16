//! Fixed-origin HTTP composition and bounded provider transport.

use codex_router_auth::live_quota::DEFAULT_CHATGPT_BACKEND_BASE_URL;
use codex_router_auth::live_quota::UsageResponse;
use codex_router_auth::live_quota::reset_credits_url;
use codex_router_auth::live_quota::usage_url;
use reqwest::header::CONTENT_TYPE;
use serde::Serialize;

use super::ConsumeResetCreditResponse;
use super::HttpLiveQuotaResetProvider;
use super::LiveQuotaResetProvider;
use super::LiveResetAccountAuth;
use super::LiveResetCredit;
use super::PreparedConsumeRequest;
use super::QuotaResetError;
use super::parsing::remaining_percent_from_used;
use super::parsing::reset_credits_from_response_body;
use crate::quota_reset::domain::ConsumePortResult;
use crate::quota_reset::domain::ConsumeUnknownReason;
use crate::quota_reset::domain::KnownConsumeOutcome;

const WEEKLY_WINDOW_SECONDS: i64 = 604_800;
pub(super) const MAXIMUM_RESPONSE_BODY_BYTES: usize = 1_048_576;
const PROVIDER_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
const PROVIDER_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

impl HttpLiveQuotaResetProvider {
    pub(crate) fn new() -> Result<Self, QuotaResetError> {
        Self::from_validated_base_url(DEFAULT_CHATGPT_BACKEND_BASE_URL.to_owned())
    }

    #[cfg(any(test, feature = "quota-reset-test-harness"))]
    pub(crate) fn new_loopback(base_url: impl AsRef<str>) -> Result<Self, QuotaResetError> {
        let parsed = reqwest::Url::parse(base_url.as_ref()).map_err(|_| {
            provider_response_failure("loopback provider origin is not a valid URL")
        })?;
        let is_loopback_http = parsed.scheme() == "http"
            && parsed
                .host_str()
                .and_then(|host| host.parse::<std::net::IpAddr>().ok())
                .is_some_and(|address| address.is_loopback());
        let is_bare_origin = parsed.port().is_some()
            && parsed.path() == "/"
            && parsed.query().is_none()
            && parsed.fragment().is_none()
            && parsed.username().is_empty()
            && parsed.password().is_none();
        if !is_loopback_http || !is_bare_origin {
            return Err(provider_response_failure(
                "loopback provider origin must be a bare loopback HTTP origin with an explicit port",
            ));
        }
        Self::from_validated_base_url(parsed.as_str().trim_end_matches('/').to_owned())
    }

    fn from_validated_base_url(base_url: String) -> Result<Self, QuotaResetError> {
        let client = reqwest::Client::builder()
            .user_agent("codex-router-quota-reset")
            .connect_timeout(PROVIDER_CONNECT_TIMEOUT)
            .timeout(PROVIDER_REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .no_proxy()
            .build()
            .map_err(provider_request_error)?;
        Ok(Self { client, base_url })
    }

    fn authenticated_request(
        &self,
        request: reqwest::RequestBuilder,
        auth: &LiveResetAccountAuth,
    ) -> reqwest::RequestBuilder {
        request
            .bearer_auth(auth.access_token.expose_secret())
            .header("ChatGPT-Account-ID", &auth.chatgpt_account_id)
    }

    async fn successful_body(request: reqwest::RequestBuilder) -> Result<Vec<u8>, QuotaResetError> {
        let response = request.send().await.map_err(provider_request_error)?;
        Self::successful_response_body(response).await
    }

    async fn successful_response_body(
        response: reqwest::Response,
    ) -> Result<Vec<u8>, QuotaResetError> {
        let status = response.status();
        if !status.is_success() {
            return Err(QuotaResetError::ProviderStatus {
                status: status.as_u16(),
            });
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAXIMUM_RESPONSE_BODY_BYTES as u64)
        {
            return Err(provider_response_failure(
                "provider response body exceeds the size limit",
            ));
        }

        let mut response = response;
        let mut body = Vec::with_capacity(
            response
                .content_length()
                .and_then(|length| usize::try_from(length).ok())
                .unwrap_or(0)
                .min(MAXIMUM_RESPONSE_BODY_BYTES),
        );
        while let Some(chunk) = response.chunk().await.map_err(provider_body_read_error)? {
            let remaining = MAXIMUM_RESPONSE_BODY_BYTES.saturating_sub(body.len());
            if chunk.len() > remaining {
                return Err(provider_response_failure(
                    "provider response body exceeds the size limit",
                ));
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }

    pub(in crate::quota_reset) fn prepare_consume_reset_credit(
        &self,
        auth: &LiveResetAccountAuth,
        credit_id: &str,
        redeem_request_id: &str,
    ) -> Result<PreparedConsumeRequest, QuotaResetError> {
        let url = format!(
            "{}/consume",
            reset_credits_url(&self.base_url).trim_end_matches('/')
        );
        let body = serde_json::to_vec(&ConsumeResetCreditRequest {
            redeem_request_id,
            credit_id,
        })
        .map_err(provider_response_error)?;
        let request = self
            .authenticated_request(
                self.client
                    .post(url)
                    .header(CONTENT_TYPE, "application/json")
                    .body(body),
                auth,
            )
            .build()
            .map_err(provider_request_error)?;
        Ok(PreparedConsumeRequest { request })
    }

    pub(in crate::quota_reset) async fn invoke_prepared_consume(
        &self,
        prepared: PreparedConsumeRequest,
    ) -> ConsumePortResult {
        let response = match self.client.execute(prepared.request).await {
            Ok(response) => response,
            Err(error) => {
                let reason = if error.is_timeout() {
                    ConsumeUnknownReason::TimedOut
                } else {
                    ConsumeUnknownReason::Transport
                };
                return ConsumePortResult::OutcomeUnknown(reason);
            }
        };
        let body = match Self::successful_response_body(response).await {
            Ok(body) => body,
            Err(QuotaResetError::ProviderStatus { .. }) => {
                return ConsumePortResult::OutcomeUnknown(ConsumeUnknownReason::ProviderStatus);
            }
            Err(QuotaResetError::ProviderRequest { .. }) => {
                return ConsumePortResult::OutcomeUnknown(ConsumeUnknownReason::Transport);
            }
            Err(_) => {
                return ConsumePortResult::OutcomeUnknown(ConsumeUnknownReason::InvalidResponse);
            }
        };
        match serde_json::from_slice::<ConsumeResetCreditResponse>(&body) {
            Ok(response) => ConsumePortResult::Known(match response.code {
                super::ConsumeResetCreditCode::Reset => {
                    let Ok(windows_reset) = u32::try_from(response.windows_reset) else {
                        return ConsumePortResult::OutcomeUnknown(
                            ConsumeUnknownReason::InvalidResponse,
                        );
                    };
                    KnownConsumeOutcome::Reset { windows_reset }
                }
                super::ConsumeResetCreditCode::NothingToReset => {
                    KnownConsumeOutcome::NothingToReset
                }
                super::ConsumeResetCreditCode::NoCredit => KnownConsumeOutcome::NoCredit,
                super::ConsumeResetCreditCode::AlreadyRedeemed => {
                    KnownConsumeOutcome::AlreadyRedeemed
                }
            }),
            Err(_error) => ConsumePortResult::OutcomeUnknown(ConsumeUnknownReason::InvalidResponse),
        }
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
            serde_json::from_slice::<UsageResponse>(&body).map_err(provider_response_error)?;
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
        reset_credits_from_response_body(&body)
    }

    async fn consume_reset_credit(
        &self,
        auth: &LiveResetAccountAuth,
        credit_id: &str,
        redeem_request_id: &str,
    ) -> Result<ConsumeResetCreditResponse, QuotaResetError> {
        let prepared = self.prepare_consume_reset_credit(auth, credit_id, redeem_request_id)?;
        match self.invoke_prepared_consume(prepared).await {
            ConsumePortResult::Known(KnownConsumeOutcome::Reset { windows_reset }) => {
                Ok(ConsumeResetCreditResponse {
                    code: super::ConsumeResetCreditCode::Reset,
                    windows_reset: i64::from(windows_reset),
                })
            }
            ConsumePortResult::Known(KnownConsumeOutcome::NothingToReset) => {
                Ok(ConsumeResetCreditResponse {
                    code: super::ConsumeResetCreditCode::NothingToReset,
                    windows_reset: 0,
                })
            }
            ConsumePortResult::Known(KnownConsumeOutcome::NoCredit) => {
                Ok(ConsumeResetCreditResponse {
                    code: super::ConsumeResetCreditCode::NoCredit,
                    windows_reset: 0,
                })
            }
            ConsumePortResult::Known(KnownConsumeOutcome::AlreadyRedeemed) => {
                Ok(ConsumeResetCreditResponse {
                    code: super::ConsumeResetCreditCode::AlreadyRedeemed,
                    windows_reset: 0,
                })
            }
            ConsumePortResult::OutcomeUnknown(reason) => {
                Err(provider_response_failure(reason.message()))
            }
        }
    }
}

#[derive(Debug, Serialize)]
struct ConsumeResetCreditRequest<'a> {
    redeem_request_id: &'a str,
    credit_id: &'a str,
}

pub(super) fn provider_request_error(error: impl std::fmt::Display) -> QuotaResetError {
    let _ = error;
    QuotaResetError::ProviderRequest {
        message: "provider transport failed".to_owned(),
    }
}

pub(super) fn provider_response_error(error: impl std::fmt::Display) -> QuotaResetError {
    let _ = error;
    QuotaResetError::ProviderResponse {
        message: "provider response was malformed".to_owned(),
    }
}

fn provider_body_read_error(error: impl std::fmt::Display) -> QuotaResetError {
    let _ = error;
    provider_response_failure("provider response body could not be read")
}

pub(super) fn provider_response_failure(message: &'static str) -> QuotaResetError {
    QuotaResetError::ProviderResponse {
        message: message.to_owned(),
    }
}

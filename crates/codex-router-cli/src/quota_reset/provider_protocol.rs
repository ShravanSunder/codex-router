//! Async live provider protocol for guarded quota reset.

use codex_router_core::redaction::SecretString;
use serde::Deserialize;

use super::QuotaResetError;
use super::reset_credit_policy::LiveResetCredit;

mod bounded_http_transport;
mod provider_response_validation;

#[derive(Clone, Eq, PartialEq)]
pub(crate) struct LiveResetAccountAuth {
    pub(crate) access_token: SecretString,
    pub(crate) chatgpt_account_id: String,
}

impl std::fmt::Debug for LiveResetAccountAuth {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LiveResetAccountAuth")
            .field("access_token", &"[REDACTED]")
            .field("chatgpt_account_id", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ConsumeResetCreditCode {
    Reset,
    NothingToReset,
    NoCredit,
    AlreadyRedeemed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub(crate) struct ConsumeResetCreditResponse {
    pub(crate) code: ConsumeResetCreditCode,
    #[serde(default)]
    pub(crate) windows_reset: i64,
}

#[derive(Clone, Debug)]
pub(crate) struct HttpLiveQuotaResetProvider {
    client: reqwest::Client,
    base_url: String,
}

/// Fully validated and serialized consume request that has not been dispatched.
pub(in crate::quota_reset) struct PreparedConsumeRequest {
    request: reqwest::Request,
}

impl std::fmt::Debug for PreparedConsumeRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("PreparedConsumeRequest(<redacted>)")
    }
}

#[cfg(test)]
mod provider_consume_protocol_test;
#[cfg(test)]
mod provider_read_protocol_test;

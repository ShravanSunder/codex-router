//! Async live provider protocol for guarded quota reset.

use codex_router_core::redaction::SecretString;
use serde::Deserialize;

use super::QuotaResetError;
use super::domain::LiveResetCredit;

mod http;
mod parsing;

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
mod tests;

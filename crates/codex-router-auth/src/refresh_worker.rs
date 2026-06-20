//! OAuth credential background refresh planning.

use crate::oauth::OAuthTokenStatus;
use crate::oauth::TokenClock;

/// Non-secret refresh input for one account.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccountRefreshInput {
    account_label: String,
    expires_at_unix_seconds: u64,
}

impl AccountRefreshInput {
    /// Creates a refresh input.
    #[must_use]
    pub fn new(account_label: impl Into<String>, expires_at_unix_seconds: u64) -> Self {
        Self {
            account_label: account_label.into(),
            expires_at_unix_seconds,
        }
    }
}

/// Refresh work decision.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RefreshWorkDecision {
    /// Account should be refreshed by the background worker.
    Refresh {
        /// Redacted/non-secret account label.
        account_label: String,
        /// Token status that caused refresh.
        token_status: OAuthTokenStatus,
    },
    /// Account can be skipped for now.
    Skip {
        /// Redacted/non-secret account label.
        account_label: String,
        /// Token status that caused skip.
        token_status: OAuthTokenStatus,
    },
}

/// Background refresh planner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RefreshWorker {
    clock: TokenClock,
    refresh_window_seconds: u64,
}

impl RefreshWorker {
    /// Creates a refresh worker.
    #[must_use]
    pub const fn new(clock: TokenClock, refresh_window_seconds: u64) -> Self {
        Self {
            clock,
            refresh_window_seconds,
        }
    }

    /// Plans refresh work without reading secret material.
    #[must_use]
    pub fn plan_refreshes(&self, accounts: &[AccountRefreshInput]) -> Vec<RefreshWorkDecision> {
        accounts
            .iter()
            .map(|account| {
                let token_status = self
                    .clock
                    .classify_token(account.expires_at_unix_seconds, self.refresh_window_seconds);
                match token_status {
                    OAuthTokenStatus::Valid { .. } => RefreshWorkDecision::Skip {
                        account_label: account.account_label.clone(),
                        token_status,
                    },
                    OAuthTokenStatus::RefreshNeeded | OAuthTokenStatus::Expired => {
                        RefreshWorkDecision::Refresh {
                            account_label: account.account_label.clone(),
                            token_status,
                        }
                    }
                }
            })
            .collect()
    }
}

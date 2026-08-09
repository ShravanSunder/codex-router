//! Durable Codex session-to-account affinity.

use codex_router_core::ids::AccountId;

/// The account most recently selected for one Codex session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionAccountAffinity {
    session_id: String,
    account_id: AccountId,
    last_seen_unix_seconds: u64,
}

impl SessionAccountAffinity {
    /// Creates a session account affinity row.
    #[must_use]
    pub fn new(
        session_id: impl Into<String>,
        account_id: AccountId,
        last_seen_unix_seconds: u64,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            account_id,
            last_seen_unix_seconds,
        }
    }

    /// Returns the Codex session id.
    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Returns the selected account id.
    #[must_use]
    pub const fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    /// Returns the most recent routed request time.
    #[must_use]
    pub const fn last_seen_unix_seconds(&self) -> u64 {
        self.last_seen_unix_seconds
    }
}

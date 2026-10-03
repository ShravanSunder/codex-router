//! Durable provider session-to-account affinity.

use codex_router_core::ids::AccountId;
use codex_router_core::provider::Provider;

/// The active pin state and persisted version observed before a routing attempt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PinObservation {
    active_account: Option<AccountId>,
    version: u64,
}

impl PinObservation {
    /// Creates an observed pin state for one routing attempt.
    #[must_use]
    pub fn new(active_account: Option<AccountId>, version: u64) -> Self {
        Self {
            active_account,
            version,
        }
    }

    /// Returns the active account at observation time, if one existed.
    #[must_use]
    pub fn active_account(&self) -> Option<&AccountId> {
        self.active_account.as_ref()
    }

    /// Returns the persisted version used to guard the observation.
    #[must_use]
    pub const fn version(&self) -> u64 {
        self.version
    }
}

/// The persisted pin state for one provider session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionAccountAffinity {
    provider: Provider,
    session_id: String,
    account_id: Option<AccountId>,
    pin_version: u64,
    last_seen_unix_seconds: u64,
}

impl SessionAccountAffinity {
    /// Creates an active pin for one provider session.
    #[must_use]
    pub fn new(
        provider: Provider,
        session_id: impl Into<String>,
        account_id: AccountId,
        last_seen_unix_seconds: u64,
    ) -> Self {
        Self::with_pin_state(
            provider,
            session_id,
            Some(account_id),
            0,
            last_seen_unix_seconds,
        )
    }

    /// Creates a versioned pin row, including a released row with no active account.
    #[must_use]
    pub fn with_pin_state(
        provider: Provider,
        session_id: impl Into<String>,
        account_id: Option<AccountId>,
        pin_version: u64,
        last_seen_unix_seconds: u64,
    ) -> Self {
        Self {
            provider,
            session_id: session_id.into(),
            account_id,
            pin_version,
            last_seen_unix_seconds,
        }
    }

    /// Returns the provider identity in the composite key.
    #[must_use]
    pub const fn provider(&self) -> Provider {
        self.provider
    }

    /// Returns the provider session identifier.
    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Returns the active account id, if this row currently owns a pin.
    #[must_use]
    pub const fn account_id(&self) -> Option<&AccountId> {
        self.account_id.as_ref()
    }

    /// Returns the version used for conditional pin ownership.
    #[must_use]
    pub const fn pin_version(&self) -> u64 {
        self.pin_version
    }

    /// Returns the most recent routed request time.
    #[must_use]
    pub const fn last_seen_unix_seconds(&self) -> u64 {
        self.last_seen_unix_seconds
    }
}

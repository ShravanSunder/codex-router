//! Account registry metadata stored outside secret material.

use codex_router_core::ids::AccountId;

/// Router account lifecycle state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccountStatus {
    /// Account may be selected when quota allows it.
    Enabled,
    /// Account is registered but not eligible.
    Disabled,
}

impl AccountStatus {
    /// Serializes status to SQLite.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Enabled => "enabled",
            Self::Disabled => "disabled",
        }
    }

    /// Parses status from SQLite.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "enabled" => Some(Self::Enabled),
            "disabled" => Some(Self::Disabled),
            _ => None,
        }
    }
}

/// Non-secret account metadata.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccountRecord {
    account_id: AccountId,
    label: String,
    status: AccountStatus,
    active_credential_generation: Option<u64>,
}

impl AccountRecord {
    /// Creates account metadata.
    #[must_use]
    pub fn new(account_id: AccountId, label: impl Into<String>, status: AccountStatus) -> Self {
        Self {
            account_id,
            label: label.into(),
            status,
            active_credential_generation: None,
        }
    }

    /// Sets the active credential generation.
    #[must_use]
    pub const fn with_active_credential_generation(
        mut self,
        active_credential_generation: u64,
    ) -> Self {
        self.active_credential_generation = Some(active_credential_generation);
        self
    }

    /// Returns the account id.
    #[must_use]
    pub const fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    /// Returns the display label.
    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Returns the lifecycle status.
    #[must_use]
    pub const fn status(&self) -> AccountStatus {
        self.status
    }

    /// Returns the active credential generation when credentials are usable.
    #[must_use]
    pub const fn active_credential_generation(&self) -> Option<u64> {
        self.active_credential_generation
    }
}

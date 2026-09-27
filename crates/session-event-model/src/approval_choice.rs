use serde::{Deserialize, Serialize};

/// Effect of choosing an option offered by the agent. The option ID remains
/// the agent's own identifier and is carried separately.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ApprovalEffect {
    Allow,
    Decline,
    Abort,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ApprovalScope {
    Once,
    Session,
    Persistent { where_stored: StorageDestination },
}

impl ApprovalScope {
    /// A persistent grant must identify the store that the agent will change.
    pub fn persistent(where_stored: impl Into<String>) -> Result<Self, InvalidStorageDestination> {
        StorageDestination::new(where_stored).map(|where_stored| Self::Persistent { where_stored })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct StorageDestination(String);

impl StorageDestination {
    pub fn new(value: impl Into<String>) -> Result<Self, InvalidStorageDestination> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(InvalidStorageDestination);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for StorageDestination {
    type Error = InvalidStorageDestination;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<StorageDestination> for String {
    fn from(value: StorageDestination) -> Self {
        value.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidStorageDestination;

impl std::fmt::Display for InvalidStorageDestination {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("persistent approval destination is empty")
    }
}

impl std::error::Error for InvalidStorageDestination {}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApprovalChoice {
    pub effect: ApprovalEffect,
    pub scope: ApprovalScope,
}

impl ApprovalChoice {
    #[must_use]
    pub fn new(effect: ApprovalEffect, scope: ApprovalScope) -> Self {
        Self { effect, scope }
    }
}

/// The person or agent designated to answer a session's interactions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionApprover(Identity);

impl SessionApprover {
    pub fn new(requester: &SessionRef, identity: Identity) -> Result<Self, ApproverIsRequester> {
        if matches!(&identity, Identity::Session { session } if session == requester) {
            return Err(ApproverIsRequester);
        }
        Ok(Self(identity))
    }

    #[must_use]
    pub fn identity(&self) -> &Identity {
        &self.0
    }
}

use message_board::{Identity, SessionRef};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApproverIsRequester;

impl std::fmt::Display for ApproverIsRequester {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a session cannot approve its own interaction")
    }
}

impl std::error::Error for ApproverIsRequester {}

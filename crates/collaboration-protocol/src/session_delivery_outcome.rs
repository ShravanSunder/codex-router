//! Client-neutral identities and observed delivery results.
use crate::DeliveryRejection;
use crate::NonEmptyText;
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
#[error("delivery correlation must contain 1 to 4096 UTF-8 bytes without NUL")]
pub struct DeliveryIdentityError;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct DeliveryCorrelationId(String);

impl TryFrom<String> for DeliveryCorrelationId {
    type Error = DeliveryIdentityError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        NonEmptyText::try_from(value.clone()).map_err(|_| DeliveryIdentityError)?;
        Ok(Self(value))
    }
}

impl From<DeliveryCorrelationId> for String {
    fn from(value: DeliveryCorrelationId) -> Self {
        value.0
    }
}

impl DeliveryCorrelationId {
    #[must_use]
    pub fn generate() -> Self {
        Self(Uuid::now_v7().hyphenated().to_string())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl JsonSchema for DeliveryCorrelationId {
    fn schema_name() -> Cow<'static, str> {
        "DeliveryCorrelationId".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type":"string", "minLength":1, "maxLength":4096
        })
    }
}

/// The strongest submission effect the selected client actually evidenced.
#[derive(Clone, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum DeliveryOutcome {
    Started,
    Steered,
    StartedOrSteered,
    Queued,
    PeerMessageWritten,
    NotSubmitted { retryable: bool, reason: String },
    Rejected(DeliveryRejection),
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SessionReachability {
    CodexAppServer,
    ProviderAcp,
    ClaudeCodePeer,
}

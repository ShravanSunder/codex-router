//! Client-neutral identities and observed delivery results.
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use uuid::{Uuid, Variant};

#[derive(Debug, thiserror::Error)]
#[error("delivery identity must be a canonical lowercase RFC UUIDv7")]
pub struct DeliveryIdentityError;

macro_rules! delivery_identity {
    ($($name:ident),+ $(,)?) => {$ (
        #[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl TryFrom<String> for $name {
            type Error = DeliveryIdentityError;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                let parsed = Uuid::parse_str(&value).map_err(|_| DeliveryIdentityError)?;
                if parsed.get_version_num() != 7
                    || parsed.get_variant() != Variant::RFC4122
                    || parsed.hyphenated().to_string() != value
                {
                    return Err(DeliveryIdentityError);
                }
                Ok(Self(value))
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self { value.0 }
        }

        impl $name {
            #[must_use]
            pub fn generate() -> Self { Self(Uuid::now_v7().hyphenated().to_string()) }

            #[must_use]
            pub fn as_str(&self) -> &str { &self.0 }
        }
    )+};
}

delivery_identity!(DeliveryCorrelationId);

impl JsonSchema for DeliveryCorrelationId {
    fn schema_name() -> Cow<'static, str> {
        "DeliveryCorrelationId".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type":"string", "minLength":36, "maxLength":36,
            "pattern":"^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$"
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
    Rejected { reason: String },
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SessionReachability {
    CodexAppServer,
    ProviderAcp,
    ClaudeCodePeer,
    None,
}

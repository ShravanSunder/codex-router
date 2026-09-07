//! Endpoint metadata keeps destination identity separate from protocol channels.
use crate::{CodexGeneration, EndpointRef};
use serde::{Deserialize, Serialize};

/// Bounded public text, distinct from any native session identity.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct NonEmptyText(String);
impl TryFrom<String> for NonEmptyText {
    type Error = &'static str;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.is_empty() || value.len() > 4096 || value.contains('\0') {
            return Err("text requires 1 to 4096 UTF-8 bytes without NUL");
        }
        Ok(Self(value))
    }
}
impl From<NonEmptyText> for String {
    fn from(value: NonEmptyText) -> Self {
        value.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SchemaDigest(String);
impl TryFrom<String> for SchemaDigest {
    type Error = &'static str;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        let digits = value
            .strip_prefix("sha256:")
            .ok_or("invalid schema digest")?;
        if digits.len() != 64
            || !digits
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err("invalid schema digest");
        }
        Ok(Self(value))
    }
}
impl From<SchemaDigest> for String {
    fn from(value: SchemaDigest) -> Self {
        value.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ObservationTimestamp(String);
impl TryFrom<String> for ObservationTimestamp {
    type Error = &'static str;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        if !value.ends_with('Z') || chrono::DateTime::parse_from_rfc3339(&value).is_err() {
            return Err("timestamp must be RFC3339 UTC ending in Z");
        }
        Ok(Self(value))
    }
}
impl From<ObservationTimestamp> for String {
    fn from(value: ObservationTimestamp) -> Self {
        value.0
    }
}

#[derive(schemars::JsonSchema, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "camelCase", deny_unknown_fields)]
pub enum EndpointAvailability {
    #[serde(rename_all = "camelCase")]
    Available {
        observed_at: ObservationTimestamp,
    },
    #[serde(rename_all = "camelCase")]
    Unavailable {
        observed_at: ObservationTimestamp,
        reason: NonEmptyText,
    },
    Unprobed,
}
#[derive(schemars::JsonSchema, Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum NativeCarrier {
    #[serde(rename = "unixWebSocket")]
    UnixWebSocket,
}
#[derive(schemars::JsonSchema, Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum AcpCarrier {
    #[serde(rename = "unixJsonLines")]
    UnixJsonLines,
}

#[derive(schemars::JsonSchema, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum ChannelDescription {
    #[serde(rename_all = "camelCase")]
    NativeCodex {
        transport: NativeCarrier,
        path: NonEmptyText,
        schema_digest: Option<SchemaDigest>,
        generation: Option<CodexGeneration>,
    },
    #[serde(rename_all = "camelCase")]
    Acp {
        transport: AcpCarrier,
        path: NonEmptyText,
        schema_digest: SchemaDigest,
    },
}

#[derive(schemars::JsonSchema, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EndpointDescription {
    pub endpoint: EndpointRef,
    pub label: NonEmptyText,
    pub availability: EndpointAvailability,
    #[serde(deserialize_with = "deserialize_channels")]
    #[schemars(length(min = 1, max = 2))]
    pub channels: Vec<ChannelDescription>,
}
fn deserialize_channels<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<ChannelDescription>, D::Error> {
    let channels = Vec::<ChannelDescription>::deserialize(deserializer)?;
    if !(1..=2).contains(&channels.len()) {
        return Err(serde::de::Error::custom(
            "endpoint requires one or two channels",
        ));
    }
    Ok(channels)
}

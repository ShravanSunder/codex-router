//! Validated endpoint and native session references shared by service and clients.
use serde::{Deserialize, Serialize};

/// Invalid public identity, without including caller-supplied content.
#[derive(Debug, thiserror::Error)]
#[error("invalid {0}")]
pub struct IdentityError(&'static str);

/// Canonical lowercase UUID identity; not a process ID or network locator.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct UuidIdentity(String);

impl TryFrom<String> for UuidIdentity {
    type Error = IdentityError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        let valid = value.len() == 36
            && value.bytes().enumerate().all(|(index, byte)| {
                if matches!(index, 8 | 13 | 18 | 23) {
                    byte == b'-'
                } else {
                    byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
                }
            });
        if valid {
            Ok(Self(value))
        } else {
            Err(IdentityError("UUID"))
        }
    }
}
impl From<UuidIdentity> for String {
    fn from(value: UuidIdentity) -> Self {
        value.0
    }
}

/// Stable name within one logical Host installation.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct EndpointId(String);
impl TryFrom<String> for EndpointId {
    type Error = IdentityError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        let valid = (1..=64).contains(&value.len())
            && value.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
            && value
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
        if valid {
            Ok(Self(value))
        } else {
            Err(IdentityError("endpoint ID"))
        }
    }
}
impl From<EndpointId> for String {
    fn from(value: EndpointId) -> Self {
        value.0
    }
}

/// Bounded opaque session identifier, interpreted only by its endpoint.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SessionId(String);
impl TryFrom<String> for SessionId {
    type Error = IdentityError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        if (1..=4096).contains(&value.len()) && !value.contains('\0') {
            Ok(Self(value))
        } else {
            Err(IdentityError("session ID"))
        }
    }
}
impl From<SessionId> for String {
    fn from(value: SessionId) -> Self {
        value.0
    }
}

/// An endpoint identity independent of the locator used to contact it.
#[derive(
    schemars::JsonSchema, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EndpointRef {
    pub service_id: UuidIdentity,
    pub endpoint_id: EndpointId,
}

/// A native conversation scoped to the endpoint that interprets its ID.
#[derive(
    schemars::JsonSchema, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionRef {
    pub endpoint: EndpointRef,
    pub session_id: SessionId,
}

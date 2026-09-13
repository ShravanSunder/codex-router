use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use uuid::{Uuid, Variant};

#[derive(Debug, thiserror::Error, Clone, Eq, PartialEq)]
#[error("{field} {requirement}")]
pub struct IdentityValidationError {
    pub field: &'static str,
    pub requirement: &'static str,
}

macro_rules! uuid_v7_identity {
    ($($name:ident),+ $(,)?) => {$ (
        #[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl JsonSchema for $name {
            fn schema_name() -> Cow<'static, str> { stringify!($name).into() }
            fn json_schema(_: &mut SchemaGenerator) -> Schema {
                json_schema!({"type":"string","minLength":36,"maxLength":36,
                    "pattern":"^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$"})
            }
        }

        impl TryFrom<String> for $name {
            type Error = IdentityValidationError;
            fn try_from(value: String) -> Result<Self, Self::Error> {
                let parsed = Uuid::parse_str(&value).map_err(|_| IdentityValidationError {
                    field: stringify!($name), requirement: "must be a canonical lowercase RFC UUIDv7",
                })?;
                if parsed.get_version_num() != 7
                    || parsed.get_variant() != Variant::RFC4122
                    || parsed.hyphenated().to_string() != value
                {
                    return Err(IdentityValidationError {
                        field: stringify!($name), requirement: "must be a canonical lowercase RFC UUIDv7",
                    });
                }
                Ok(Self(value))
            }
        }

        impl From<$name> for String { fn from(value: $name) -> Self { value.0 } }
        impl $name {
            #[must_use] pub fn generate() -> Self { Self(Uuid::now_v7().hyphenated().to_string()) }
            #[must_use] pub fn as_str(&self) -> &str { &self.0 }
        }
    )+};
}

uuid_v7_identity!(ProjectId, BoardId, TopicId, MessageId);

macro_rules! validated_identity_string {
    ($name:ident, $field:literal, $min:expr, $max:expr, $validator:expr, $requirement:literal) => {
        #[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);
        impl JsonSchema for $name {
            fn schema_name() -> Cow<'static, str> { stringify!($name).into() }
            fn json_schema(_: &mut SchemaGenerator) -> Schema {
                json_schema!({"type":"string","minLength":$min,"maxLength":$max})
            }
        }
        impl TryFrom<String> for $name {
            type Error = IdentityValidationError;
            fn try_from(value: String) -> Result<Self, Self::Error> {
                if !($min..=$max).contains(&value.len()) || !($validator)(&value) {
                    return Err(IdentityValidationError {
                        field: $field,
                        requirement: $requirement,
                    });
                }
                Ok(Self(value))
            }
        }
        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }
        impl $name {
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}

validated_identity_string!(
    ServiceId,
    "serviceId",
    36,
    36,
    |value: &str| {
        Uuid::parse_str(value).is_ok_and(|parsed| parsed.hyphenated().to_string() == value)
    },
    "must be a canonical lowercase RFC UUID"
);
validated_identity_string!(
    EndpointId,
    "endpointId",
    1,
    64,
    |value: &str| {
        value.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
            && value
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    },
    "must start with a lowercase letter and contain only lowercase letters, digits, or hyphens"
);
validated_identity_string!(
    SessionId,
    "sessionId",
    1,
    4096,
    |value: &str| !value.contains('\0'),
    "must contain 1 to 4096 UTF-8 bytes without NUL"
);
validated_identity_string!(
    HumanId,
    "humanId",
    1,
    4096,
    |value: &str| !value.contains('\0'),
    "must contain 1 to 4096 UTF-8 bytes without NUL"
);

#[derive(
    Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionEndpointRef {
    pub service_id: ServiceId,
    pub endpoint_id: EndpointId,
}

#[derive(
    Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionRef {
    pub endpoint: SessionEndpointRef,
    pub session_id: SessionId,
}

#[derive(
    Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum Identity {
    #[serde(rename_all = "camelCase")]
    Session { session: SessionRef },
    #[serde(rename_all = "camelCase")]
    Human { human_id: HumanId },
}

impl Identity {
    #[must_use]
    pub fn as_human(&self) -> Option<&HumanId> {
        match self {
            Self::Human { human_id } => Some(human_id),
            Self::Session { .. } => None,
        }
    }
}

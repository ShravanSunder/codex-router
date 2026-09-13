use crate::{BoardId, ProjectId, TopicId};
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;

pub const MAX_NAME_BYTES: usize = 256;
pub const MAX_DESCRIPTION_BYTES: usize = 16 * 1024;
pub const MAX_MESSAGE_TEXT_BYTES: usize = 64 * 1024;

#[derive(Debug, thiserror::Error, Clone, Eq, PartialEq)]
#[error("{field} {requirement}")]
pub struct TextValidationError {
    pub field: &'static str,
    pub requirement: &'static str,
}

macro_rules! bounded_text {
    ($name:ident, $field:literal, $min:expr, $max:expr, $trim:expr) => {
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
            type Error = TextValidationError;
            fn try_from(value: String) -> Result<Self, Self::Error> {
                let canonical = if $trim { value.trim().to_owned() } else { value };
                if !($min..=$max).contains(&canonical.len()) {
                    return Err(TextValidationError { field: $field, requirement: "is outside its UTF-8 byte bound" });
                }
                Ok(Self(canonical))
            }
        }
        impl From<$name> for String { fn from(value: $name) -> Self { value.0 } }
        impl $name { #[must_use] pub fn as_str(&self) -> &str { &self.0 } }
    };
}

bounded_text!(ResourceName, "name", 1, MAX_NAME_BYTES, true);
bounded_text!(Description, "description", 0, MAX_DESCRIPTION_BYTES, false);
bounded_text!(MessageText, "text", 1, MAX_MESSAGE_TEXT_BYTES, false);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum BoardState {
    Active,
    Archived,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ThreadState {
    Unresolved,
    Resolved,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Project {
    pub project_id: ProjectId,
    pub name: ResourceName,
    pub description: Description,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Board {
    pub board_id: BoardId,
    pub project_id: ProjectId,
    pub name: ResourceName,
    pub description: Description,
    pub state: BoardState,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Topic {
    pub topic_id: TopicId,
    pub board_id: BoardId,
    pub name: ResourceName,
    pub description: Description,
}

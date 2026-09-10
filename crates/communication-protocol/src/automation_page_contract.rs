//! Language-independent bounded listing contracts; opaque cursors retain service scope.
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(try_from = "u32", into = "u32")]
pub struct PageLimit(u32);
#[derive(Debug, thiserror::Error)]
#[error("page limit must be between 1 and 100")]
pub struct InvalidPageLimit;
impl TryFrom<u32> for PageLimit {
    type Error = InvalidPageLimit;
    fn try_from(value: u32) -> Result<Self, Self::Error> {
        if (1..=100).contains(&value) {
            Ok(Self(value))
        } else {
            Err(InvalidPageLimit)
        }
    }
}
impl From<PageLimit> for u32 {
    fn from(value: PageLimit) -> Self {
        value.0
    }
}
impl JsonSchema for PageLimit {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "PageLimit".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        schemars::json_schema!({"type":"integer","minimum":1,"maximum":100})
    }
}
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationPageRequest {
    #[serde(deserialize_with = "Option::deserialize")]
    pub cursor: Option<String>,
    pub limit: PageLimit,
}
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationPage<TRecord> {
    pub records: Vec<TRecord>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub next_cursor: Option<String>,
}

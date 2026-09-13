use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Serialize};

pub const DEFAULT_PAGE_LIMIT: u32 = 50;
pub const MAX_PAGE_LIMIT: u32 = 100;

#[derive(Debug, thiserror::Error, Clone, Eq, PartialEq)]
#[error("page limit must be between 1 and 100")]
pub struct InvalidPageLimit;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "u32", into = "u32")]
pub struct PageLimit(u32);

impl TryFrom<u32> for PageLimit {
    type Error = InvalidPageLimit;
    fn try_from(value: u32) -> Result<Self, Self::Error> {
        (1..=MAX_PAGE_LIMIT)
            .contains(&value)
            .then_some(Self(value))
            .ok_or(InvalidPageLimit)
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
impl From<PageLimit> for u32 {
    fn from(value: PageLimit) -> Self {
        value.0
    }
}
impl Default for PageLimit {
    fn default() -> Self {
        Self(DEFAULT_PAGE_LIMIT)
    }
}
impl PageLimit {
    #[must_use]
    pub fn get(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PageRequest {
    #[serde(default)]
    pub limit: PageLimit,
    #[serde(default)]
    pub cursor: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Page<TItem> {
    pub records: Vec<TItem>,
    pub next_cursor: Option<String>,
}

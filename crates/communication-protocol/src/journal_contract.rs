//! Shared journal wire contracts without storage dependencies.
use crate::{EndpointRef, LifecycleObservation, UuidIdentity};
use serde::{Deserialize, Serialize};
#[derive(schemars::JsonSchema, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JournalPosition {
    pub journal_id: UuidIdentity,
    #[schemars(range(min = 0, max = 9007199254740991_u64))]
    pub sequence: u64,
}
#[derive(schemars::JsonSchema, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JournalBounds {
    pub journal_id: UuidIdentity,
    #[schemars(range(min = 1, max = 9007199254740991_u64))]
    pub earliest_sequence: u64,
    #[schemars(range(min = 0, max = 9007199254740991_u64))]
    pub last_sequence: u64,
}
#[derive(schemars::JsonSchema, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LifecycleRecord {
    pub journal_id: UuidIdentity,
    #[schemars(range(min = 1, max = 9007199254740991_u64))]
    pub sequence: u64,
    #[schemars(range(min = 1, max = 1))]
    pub schema_version: u8,
    #[serde(flatten)]
    pub observation: LifecycleObservation,
}
#[derive(schemars::JsonSchema, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JournalPage {
    pub bounds: JournalBounds,
    #[schemars(length(max = 100))]
    pub records: Vec<LifecycleRecord>,
    pub next: JournalPosition,
    pub caught_up: bool,
}

#[derive(schemars::JsonSchema, Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "storage", rename_all = "camelCase", deny_unknown_fields)]
pub enum JournalStatus {
    Available { bounds: JournalBounds },
    Unavailable,
}
#[derive(schemars::JsonSchema, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JournalReadParams {
    pub endpoint: EndpointRef,
    pub after: JournalPosition,
    #[schemars(range(min = 1, max = 100))]
    pub page_size: u32,
    #[schemars(range(min = 0, max = 30000))]
    pub wait_milliseconds: u64,
}
#[derive(schemars::JsonSchema, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AddressListParams {
    pub endpoint: EndpointRef,
    #[schemars(range(min = 1, max = 100))]
    pub page_size: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = 1024))]
    pub cursor: Option<String>,
}

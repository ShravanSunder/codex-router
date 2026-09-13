//! Public address snapshots and observer coverage without storage ownership.
use crate::{
    CodexGeneration, EndpointRef, JournalPosition, NativeThreadStatus, ObservationScope,
    ObservationTimestamp, ThreadAddress, UuidIdentity,
};
use serde::{Deserialize, Serialize};
#[derive(schemars::JsonSchema, Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Existence {
    Unknown,
    Observed,
    Deleted,
}
#[derive(schemars::JsonSchema, Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ArchiveState {
    Unknown,
    Archived,
    Unarchived,
}
#[derive(schemars::JsonSchema, Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StatusOrdering {
    Unknown,
    Established,
    Ambiguous,
}
#[derive(schemars::JsonSchema, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LifecycleDisposition {
    pub existence: Existence,
    pub archive: ArchiveState,
    pub last_close: Option<JournalPosition>,
    pub last_change: Option<JournalPosition>,
}
#[derive(schemars::JsonSchema, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AddressEntry {
    pub address: ThreadAddress,
    pub first_observed_at: ObservationTimestamp,
    pub last_observed_at: ObservationTimestamp,
    pub last_observation: JournalPosition,
    pub last_status: Option<NativeThreadStatus>,
    pub status_scope: Option<ObservationScope>,
    pub status_ordering: StatusOrdering,
    pub disposition: LifecycleDisposition,
}
#[derive(schemars::JsonSchema, Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CoverageState {
    Initializing,
    Observing,
    Disconnected,
    StorageUnavailable,
}
#[derive(schemars::JsonSchema, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CoverageView {
    pub endpoint: EndpointRef,
    pub observer_id: Option<UuidIdentity>,
    pub generation: Option<CodexGeneration>,
    pub state: CoverageState,
    pub observed_at: ObservationTimestamp,
}
#[derive(schemars::JsonSchema, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AddressPage {
    pub captured_at: ObservationTimestamp,
    pub coverage: CoverageView,
    pub snapshot_id: UuidIdentity,
    pub watermark: JournalPosition,
    #[schemars(length(max = 100))]
    pub entries: Vec<AddressEntry>,
    #[schemars(length(min = 1, max = 1024))]
    pub next_cursor: Option<String>,
}

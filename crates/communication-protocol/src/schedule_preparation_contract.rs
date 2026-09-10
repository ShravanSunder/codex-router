//! Explicit destination preparation; native addresses are returned by the selected backend.
use crate::{EndpointRef, NonEmptyText, OperationId, ScheduleId, SessionRef};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum DestinationPreparation {
    Fresh {
        endpoint: EndpointRef,
        cwd: String,
    },
    Fork {
        source: SessionRef,
        through_turn_id: NonEmptyText,
        cwd: String,
    },
    Existing {
        target: SessionRef,
        cwd: String,
    },
}
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SchedulePrepareRequest {
    pub operation_id: OperationId,
    pub schedule_id: ScheduleId,
    pub destination: DestinationPreparation,
}

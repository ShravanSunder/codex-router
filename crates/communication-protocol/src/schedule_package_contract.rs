//! Portable package text crosses the Control boundary; filesystem paths remain client-local.
use crate::OperationId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScheduleImportRequest {
    pub operation_id: OperationId,
    pub package_utf8: String,
    pub overwrite: bool,
}

#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScheduleExportResult {
    pub package_utf8: String,
}

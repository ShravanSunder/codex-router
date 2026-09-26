//! The common result of waiting for a caller-owned conversation create.
use crate::{OperationId, SessionRef};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ConversationCreateOutcome {
    Created {
        operation_id: OperationId,
        target: SessionRef,
    },
    Pending {
        operation_id: OperationId,
    },
}

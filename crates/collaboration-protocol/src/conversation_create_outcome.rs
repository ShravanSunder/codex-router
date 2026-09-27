//! The common result of waiting for a caller-owned conversation create.
use crate::{
    AppliedProviderSetting, EffectiveProviderSettings, FailedProviderSetting, OperationId,
    SessionRef,
};
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        effective_settings: Option<EffectiveProviderSettings>,
    },
    CreatedWithoutSettings {
        operation_id: OperationId,
        target: SessionRef,
        applied: Vec<AppliedProviderSetting>,
        failed: Vec<FailedProviderSetting>,
    },
    Pending {
        operation_id: OperationId,
    },
}

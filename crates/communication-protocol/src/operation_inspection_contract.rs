//! Durable operation state is distinct from native work completion and connection-local request IDs.
use crate::{
    AutomationConfiguration, AutomationOperationMethod, ConfigurationFileState,
    NativeEffectEvidence, ObservationTimestamp, OperationId, OperationSuccess,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OperationLocalMutation {
    None,
    Committed,
}
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum OperationEffects {
    Local {
        mutation: OperationLocalMutation,
    },
    Native {
        evidence: Box<NativeEffectEvidence>,
    },
    Configuration {
        file_state: ConfigurationFileState,
        intended: AutomationConfiguration,
    },
}
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperationFailure {
    pub kind: String,
    pub stage: String,
    pub message: String,
    #[serde(deserialize_with = "Option::deserialize")]
    pub field: Option<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub constraint: Option<String>,
    pub next_action: String,
    pub effects: OperationEffects,
}
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum OperationState {
    Admitted {
        effects: OperationEffects,
    },
    InProgress {
        effects: OperationEffects,
    },
    Uncertain {
        effects: OperationEffects,
        explanation: String,
    },
    Succeeded {
        outcome: OperationSuccess,
    },
    Failed {
        error: Box<OperationFailure>,
    },
}
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperationSnapshot {
    pub operation_id: OperationId,
    pub method: AutomationOperationMethod,
    pub resource_id: String,
    pub admitted_at: ObservationTimestamp,
    pub state: OperationState,
}
impl OperationSnapshot {
    #[must_use]
    pub fn has_consistent_outcome(&self) -> bool {
        match &self.state {
            OperationState::Succeeded { outcome } => {
                outcome.method_name() == self.method.as_str()
                    && outcome.resource_id() == self.resource_id
            }
            _ => true,
        }
    }
}
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperationShowRequest {
    pub operation_id: OperationId,
}

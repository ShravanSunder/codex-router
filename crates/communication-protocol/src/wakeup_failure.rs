//! Local reminder failures preserve mutation uncertainty and an explicit next action.
use crate::LocalMutationEvidence;
use agent_automation::{OperationId, WakeupId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum WakeFailureReason {
    InvalidField { field: String, constraint: String },
    OperationConflict,
    AutomationUnavailable,
    ResourceNotFound,
    InvalidRecord,
    LifecycleConflict,
    Overloaded,
}
#[derive(Clone, Copy, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WakeFailureStage {
    Validation,
    Admission,
    Storage,
    Inspection,
}
#[derive(Clone, Copy, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WakeNextAction {
    CorrectRequest,
    InspectOperation,
    VerifyResourceAddress,
    InspectWakeup,
    RetryLater,
}
#[derive(Clone, Debug, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WakeFailure {
    #[serde(flatten)]
    pub reason: WakeFailureReason,
    pub stage: WakeFailureStage,
    pub message: String,
    pub operation_id: Option<OperationId>,
    pub wakeup_id: Option<WakeupId>,
    pub effects: LocalMutationEvidence,
    pub next_action: WakeNextAction,
}

// Serde flatten + deny_unknown_fields cannot decode a tagged payload reliably.
// Decode a closed wire object, then validate the kind-dependent fields explicitly.
impl<'de> Deserialize<'de> for WakeFailure {
    fn deserialize<TDeserializer: serde::Deserializer<'de>>(
        deserializer: TDeserializer,
    ) -> Result<Self, TDeserializer::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct WireFailure {
            kind: String,
            field: Option<String>,
            constraint: Option<String>,
            stage: WakeFailureStage,
            message: String,
            #[serde(deserialize_with = "Option::deserialize")]
            operation_id: Option<OperationId>,
            #[serde(deserialize_with = "Option::deserialize")]
            wakeup_id: Option<WakeupId>,
            effects: LocalMutationEvidence,
            next_action: WakeNextAction,
        }
        let wire = WireFailure::deserialize(deserializer)?;
        let invalid = || serde::de::Error::custom("invalid wake failure kind or field guidance");
        let reason = if wire.kind == "invalidField" {
            WakeFailureReason::InvalidField {
                field: wire
                    .field
                    .filter(|text| !text.is_empty())
                    .ok_or_else(invalid)?,
                constraint: wire
                    .constraint
                    .filter(|text| !text.is_empty())
                    .ok_or_else(invalid)?,
            }
        } else {
            if wire.field.is_some() || wire.constraint.is_some() {
                return Err(invalid());
            }
            match wire.kind.as_str() {
                "operationConflict" => WakeFailureReason::OperationConflict,
                "automationUnavailable" => WakeFailureReason::AutomationUnavailable,
                "resourceNotFound" => WakeFailureReason::ResourceNotFound,
                "invalidRecord" => WakeFailureReason::InvalidRecord,
                "lifecycleConflict" => WakeFailureReason::LifecycleConflict,
                "overloaded" => WakeFailureReason::Overloaded,
                _ => return Err(invalid()),
            }
        };
        Ok(Self {
            reason,
            stage: wire.stage,
            message: wire.message,
            operation_id: wire.operation_id,
            wakeup_id: wire.wakeup_id,
            effects: wire.effects,
            next_action: wire.next_action,
        })
    }
}

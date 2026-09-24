//! A delivery reports its observed outcome and only the client evidence it has.
use crate::{
    DeliveryOutcome, NativeSendAcceptance, NativeSendReceipt, OperationId, SessionReachability,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum DeliveryClientReceipt {
    CodexAppServer(NativeSendReceipt),
    ProviderAcp { operation_id: OperationId },
    ClaudeCodePeer,
}

#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeliveryReceipt {
    pub outcome: DeliveryOutcome,
    pub reachability: Option<SessionReachability>,
    pub client: Option<DeliveryClientReceipt>,
}

impl From<NativeSendReceipt> for DeliveryReceipt {
    fn from(receipt: NativeSendReceipt) -> Self {
        let outcome = match receipt.acceptance {
            NativeSendAcceptance::NativeInputAccepted { .. } => DeliveryOutcome::StartedOrSteered,
            NativeSendAcceptance::QueueAccepted { .. } => DeliveryOutcome::Queued,
            NativeSendAcceptance::SteerAccepted { .. } => DeliveryOutcome::Steered,
        };
        Self {
            outcome,
            reachability: Some(SessionReachability::CodexAppServer),
            client: Some(DeliveryClientReceipt::CodexAppServer(receipt)),
        }
    }
}

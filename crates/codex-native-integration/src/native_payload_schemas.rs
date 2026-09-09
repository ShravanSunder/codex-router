//! Exact operation payload schemas; semantic profile admission remains the Host's responsibility.
use crate::{NativeSchemaBundle, NativeSchemaError, NativeSchemaValidator};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum NativeOperation {
    ReadThread,
    ResumeThread,
    StartThread,
    ForkThread,
    ListTurns,
    ListItems,
    ListLoadedThreads,
    StartTurn,
    SteerTurn,
    InterruptTurn,
    QueueAdd,
}
impl NativeOperation {
    pub(crate) fn contract(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::QueueAdd => (
                "thread/queue/add",
                "ThreadQueueAddParams",
                "ThreadQueueAddResponse",
            ),
            Self::ReadThread => ("thread/read", "ThreadReadParams", "ThreadReadResponse"),
            Self::ResumeThread => (
                "thread/resume",
                "ThreadResumeParams",
                "ThreadResumeResponse",
            ),
            Self::ListTurns => (
                "thread/turns/list",
                "ThreadTurnsListParams",
                "ThreadTurnsListResponse",
            ),
            Self::ListItems => (
                "thread/items/list",
                "ThreadItemsListParams",
                "ThreadItemsListResponse",
            ),
            Self::ForkThread => ("thread/fork", "ThreadForkParams", "ThreadForkResponse"),
            Self::StartThread => ("thread/start", "ThreadStartParams", "ThreadStartResponse"),
            Self::ListLoadedThreads => (
                "thread/loaded/list",
                "ThreadLoadedListParams",
                "ThreadLoadedListResponse",
            ),
            Self::StartTurn => ("turn/start", "TurnStartParams", "TurnStartResponse"),
            Self::SteerTurn => ("turn/steer", "TurnSteerParams", "TurnSteerResponse"),
            Self::InterruptTurn => (
                "turn/interrupt",
                "TurnInterruptParams",
                "TurnInterruptResponse",
            ),
        }
    }
    pub(crate) fn has_effects(self) -> bool {
        !matches!(
            self,
            Self::ReadThread | Self::ListLoadedThreads | Self::ListTurns | Self::ListItems
        )
    }
}

pub struct NativePayloadSchemas {
    schema_digest: String,
    server_messages: Option<(NativeSchemaValidator, NativeSchemaValidator)>,
    operations: BTreeMap<NativeOperation, (NativeSchemaValidator, NativeSchemaValidator)>,
}
impl NativePayloadSchemas {
    /// Fails closed if any typed operation lacks a compilable generated definition.
    pub fn from_bundle(bundle: &NativeSchemaBundle) -> Result<Self, NativeSchemaError> {
        let mut operations = BTreeMap::new();
        for operation in [
            NativeOperation::ReadThread,
            NativeOperation::ResumeThread,
            NativeOperation::StartThread,
            NativeOperation::ListLoadedThreads,
            NativeOperation::StartTurn,
            NativeOperation::SteerTurn,
            NativeOperation::InterruptTurn,
        ] {
            let (_, params, result) = operation.contract();
            operations.insert(
                operation,
                (
                    bundle.validator_for_v2(params)?,
                    bundle.validator_for_v2(result)?,
                ),
            );
        }
        for operation in [
            NativeOperation::QueueAdd,
            NativeOperation::ForkThread,
            NativeOperation::ListTurns,
            NativeOperation::ListItems,
        ] {
            let (_, params_name, result_name) = operation.contract();
            if let (Ok(params), Ok(result)) = (
                bundle.validator_for_v2(params_name),
                bundle.validator_for_v2(result_name),
            ) {
                operations.insert(operation, (params, result));
            }
        }
        let schema_digest = format!(
            "sha256:{}",
            bundle
                .digest()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        );
        let server_messages = match (
            bundle.validator_for_root("ServerRequest"),
            bundle.validator_for_root("ServerNotification"),
        ) {
            (Ok(requests), Ok(notifications)) => Some((requests, notifications)),
            _ => None,
        };
        Ok(Self {
            server_messages,
            operations,
            schema_digest,
        })
    }
    #[must_use]
    pub fn schema_digest(&self) -> &str {
        &self.schema_digest
    }
    #[must_use]
    pub fn supports_server_messages(&self) -> bool {
        self.server_messages.is_some()
    }
    /// Typed adapters fail closed; opaque native relay remains independent of this validator.
    #[must_use]
    pub fn validates_server_message(&self, message: &serde_json::Value) -> bool {
        let Some((requests, notifications)) = &self.server_messages else {
            return false;
        };
        if message.get("id").is_some() {
            requests.is_valid(message)
        } else {
            notifications.is_valid(message)
        }
    }
    #[must_use]
    pub fn supports_operation(&self, operation: NativeOperation) -> bool {
        self.operations.contains_key(&operation)
    }
    pub(crate) fn validators(
        &self,
        operation: NativeOperation,
    ) -> Option<&(NativeSchemaValidator, NativeSchemaValidator)> {
        self.operations.get(&operation)
    }
}

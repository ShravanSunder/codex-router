//! Instruction RPC adapts typed domain storage outcomes; it never touches native threads.
use agent_automation::InstructionDocument;
use automation_storage::{AutomationStore, InstructionUpdate, StorageError};
use communication_protocol::{
    InstructionCreateParams, InstructionFailure, InstructionFailureKind, InstructionId,
    InstructionNextAction, InstructionShowParams, InstructionSnapshot, InstructionStage,
    InstructionUpdateParams, LocalMutationEvidence, LocalMutationState, ObservationTimestamp,
    OperationId,
};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::Mutex;

pub(crate) struct InstructionRequest<'a> {
    pub id: Value,
    pub method: &'a str,
    pub params: Value,
    pub store: Option<&'a Arc<Mutex<AutomationStore>>>,
}
pub(crate) async fn dispatch(request: InstructionRequest<'_>) -> Value {
    let operation_id = request
        .params
        .get("operationId")
        .cloned()
        .and_then(|v| serde_json::from_value::<OperationId>(v).ok());
    let instruction_id = request
        .params
        .get("instructionId")
        .cloned()
        .and_then(|v| serde_json::from_value::<InstructionId>(v).ok());
    let mut context = FailureContext {
        operation_id,
        instruction_id,
        mutation: request.method != "instruction/show",
        current_revision_id: None,
    };
    let Some(store) = request.store else {
        return failure(
            request.id,
            context,
            FailureReason {
                kind: InstructionFailureKind::AutomationUnavailable,
                stage: InstructionStage::Storage,
                message: "Automation storage is unavailable; no instruction mutation was dispatched.",
                mutation: LocalMutationState::None,
            },
        );
    };
    let now = chrono::Utc::now().timestamp_millis();
    let result = match request.method {
        "instruction/create" => {
            match serde_json::from_value::<InstructionCreateParams>(request.params) {
                Ok(params) => {
                    store
                        .lock()
                        .await
                        .create_instruction(&params.operation_id, &params.text, now)
                        .await
                }
                Err(_) => {
                    return invalid(
                        request.id,
                        context,
                        "Provide operationId as UUIDv7 and nonempty text; unknown fields are rejected.",
                    );
                }
            }
        }
        "instruction/update" => {
            match serde_json::from_value::<InstructionUpdateParams>(request.params) {
                Ok(params) => {
                    store
                        .lock()
                        .await
                        .update_instruction(&InstructionUpdate {
                            operation_id: params.operation_id,
                            instruction_id: params.instruction_id,
                            expected_revision_id: params.expected_revision_id,
                            text: params.text,
                            now_ms: now,
                        })
                        .await
                }
                Err(_) => {
                    return invalid(
                        request.id,
                        context,
                        "Provide operationId, instructionId, expectedRevisionId as UUIDv7 and nonempty text; unknown fields are rejected.",
                    );
                }
            }
        }
        "instruction/show" => match serde_json::from_value::<InstructionShowParams>(request.params)
        {
            Ok(params) => {
                store
                    .lock()
                    .await
                    .read_instruction(&params.instruction_id)
                    .await
            }
            Err(_) => {
                return invalid(
                    request.id,
                    context,
                    "Provide instructionId as UUIDv7; unknown fields are rejected.",
                );
            }
        },
        _ => return invalid(request.id, context, "Unsupported instruction operation."),
    };
    match result {
        Ok(document) => match snapshot(document) {
            Ok(snapshot) => json!({"jsonrpc":"2.0","id":request.id,"result":snapshot}),
            Err(_) => {
                let mutation = if context.mutation {
                    LocalMutationState::Committed
                } else {
                    LocalMutationState::None
                };
                failure(
                    request.id,
                    context,
                    FailureReason {
                        kind: InstructionFailureKind::InvalidRecord,
                        stage: InstructionStage::Inspection,
                        message: "Instruction outcome exists but its stored timestamps are invalid; inspect the operation before retrying.",
                        mutation,
                    },
                )
            }
        },
        Err(error) => {
            if let StorageError::RevisionConflict {
                current_revision_id,
            } = &error
            {
                context.current_revision_id = Some(current_revision_id.clone());
            }
            let (kind, message) = match &error {
                StorageError::OperationConflict => (
                    InstructionFailureKind::OperationConflict,
                    "Operation identity belongs to a different request; inspect that operation or choose a new identity for new work.",
                ),
                StorageError::RevisionConflict { .. } => (
                    InstructionFailureKind::RevisionConflict,
                    "Instruction changed; read its current revision before editing.",
                ),
                StorageError::InstructionNotFound => (
                    InstructionFailureKind::ResourceNotFound,
                    "Instruction was not found in this service; verify its identity.",
                ),
                StorageError::Database(_) => (
                    InstructionFailureKind::AutomationUnavailable,
                    "Instruction storage request failed; a mutation may have committed. Inspect or replay the same operation identity, never create a new one blindly.",
                ),
                _ => (
                    InstructionFailureKind::InvalidRecord,
                    "Instruction storage is inconsistent; inspect the stored operation and record.",
                ),
            };
            let effects = if context.mutation && matches!(error, StorageError::Database(_)) {
                LocalMutationState::Unknown
            } else {
                LocalMutationState::None
            };
            failure(
                request.id,
                context,
                FailureReason {
                    kind,
                    stage: InstructionStage::Storage,
                    message,
                    mutation: effects,
                },
            )
        }
    }
}
struct FailureContext {
    operation_id: Option<OperationId>,
    instruction_id: Option<InstructionId>,
    mutation: bool,
    current_revision_id: Option<communication_protocol::RevisionId>,
}
fn invalid(id: Value, context: FailureContext, message: &str) -> Value {
    failure(
        id,
        context,
        FailureReason {
            kind: InstructionFailureKind::InvalidField,
            stage: InstructionStage::Validation,
            message,
            mutation: LocalMutationState::None,
        },
    )
}
struct FailureReason<'a> {
    kind: InstructionFailureKind,
    stage: InstructionStage,
    message: &'a str,
    mutation: LocalMutationState,
}
fn failure(id: Value, context: FailureContext, reason: FailureReason<'_>) -> Value {
    let FailureReason {
        kind,
        stage,
        message,
        mutation,
    } = reason;
    let next_action = match kind {
        InstructionFailureKind::InvalidField => InstructionNextAction::CorrectRequest,
        InstructionFailureKind::RevisionConflict | InstructionFailureKind::ResourceNotFound => {
            InstructionNextAction::InspectInstruction
        }
        InstructionFailureKind::Overloaded => InstructionNextAction::RetryLater,
        _ if context.operation_id.is_some() => InstructionNextAction::InspectOperation,
        _ => InstructionNextAction::RetryLater,
    };
    let data = InstructionFailure {
        kind,
        stage,
        message: message.into(),
        operation_id: context.operation_id,
        instruction_id: context.instruction_id,
        current_revision_id: context.current_revision_id,
        effects: LocalMutationEvidence::Local { mutation },
        next_action,
    };
    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32050,"message":"Instruction operation failed","data":data}})
}
pub(crate) fn overloaded(id: Value) -> Value {
    failure(
        id,
        FailureContext {
            operation_id: None,
            instruction_id: None,
            mutation: false,
            current_revision_id: None,
        },
        FailureReason {
            kind: InstructionFailureKind::Overloaded,
            stage: InstructionStage::Admission,
            message: "Control request capacity exceeded; no instruction mutation was dispatched.",
            mutation: LocalMutationState::None,
        },
    )
}
pub(crate) fn snapshot(document: InstructionDocument) -> Result<InstructionSnapshot, ()> {
    fn time(value: i64) -> Result<ObservationTimestamp, ()> {
        chrono::DateTime::<chrono::Utc>::from_timestamp_millis(value)
            .ok_or(())?
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
            .try_into()
            .map_err(|_| ())
    }
    Ok(InstructionSnapshot {
        instruction_id: document.instruction_id,
        revision_id: document.revision_id,
        text: document.text,
        created_at: time(document.created_at_ms)?,
        updated_at: time(document.updated_at_ms)?,
    })
}

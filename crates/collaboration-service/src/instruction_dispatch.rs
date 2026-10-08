//! Instruction Control dispatch over the typed automation operations; it never touches native
//! threads.
use crate::ServiceIdentity;
use crate::collaboration_application::{
    AutomationOperations, InstructionContext, InstructionFailureReason, instruction_failure,
};
use collaboration_protocol::{
    InstructionCreateParams, InstructionFailure, InstructionFailureKind, InstructionId,
    InstructionShowParams, InstructionStage, InstructionUpdateParams, LocalMutationState,
    OperationId,
};
use serde_json::{Value, json};

pub(crate) async fn dispatch(
    id: Value,
    method: &str,
    params: Value,
    identity: &ServiceIdentity,
) -> Value {
    let context = InstructionContext {
        operation_id: params
            .get("operationId")
            .cloned()
            .and_then(|v| serde_json::from_value::<OperationId>(v).ok()),
        instruction_id: params
            .get("instructionId")
            .cloned()
            .and_then(|v| serde_json::from_value::<InstructionId>(v).ok()),
        mutation: method != "instruction/show",
        current_revision_id: None,
    };
    let automation = AutomationOperations::new(identity);
    let result = match method {
        "instruction/create" => {
            let Ok(params) = serde_json::from_value::<InstructionCreateParams>(params) else {
                return invalid(
                    id,
                    context,
                    "Provide operationId as UUIDv7 and nonempty text; unknown fields are rejected.",
                );
            };
            automation.instruction_create(params).await
        }
        "instruction/update" => {
            let Ok(params) = serde_json::from_value::<InstructionUpdateParams>(params) else {
                return invalid(
                    id,
                    context,
                    "Provide operationId, instructionId, expectedRevisionId as UUIDv7 and nonempty text; unknown fields are rejected.",
                );
            };
            automation.instruction_update(params).await
        }
        "instruction/show" => {
            let Ok(params) = serde_json::from_value::<InstructionShowParams>(params) else {
                return invalid(
                    id,
                    context,
                    "Provide instructionId as UUIDv7; unknown fields are rejected.",
                );
            };
            automation.instruction_show(params).await
        }
        _ => return invalid(id, context, "Unsupported instruction operation."),
    };
    match result {
        Ok(snapshot) => json!({"jsonrpc":"2.0","id":id,"result":snapshot}),
        Err(data) => failure_response(id, data),
    }
}
fn invalid(id: Value, context: InstructionContext, message: &str) -> Value {
    failure_response(
        id,
        instruction_failure(
            context,
            InstructionFailureReason {
                kind: InstructionFailureKind::InvalidField,
                stage: InstructionStage::Validation,
                message,
                mutation: LocalMutationState::None,
            },
        ),
    )
}
fn failure_response(id: Value, data: InstructionFailure) -> Value {
    crate::control_connection::rejection_response(id, &data)
}
pub(crate) fn overloaded(id: Value) -> Value {
    failure_response(
        id,
        instruction_failure(
            InstructionContext {
                operation_id: None,
                instruction_id: None,
                mutation: false,
                current_revision_id: None,
            },
            InstructionFailureReason {
                kind: InstructionFailureKind::Overloaded,
                stage: InstructionStage::Admission,
                message: "Control request capacity exceeded; no instruction mutation was dispatched.",
                mutation: LocalMutationState::None,
            },
        ),
    )
}

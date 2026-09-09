//! Public Run inspection and explicit same-Run summary recovery; no automatic native resend.
use automation_storage::{
    AutomationStore, StorageError, SummaryRecoveryAction, SummaryRecoveryRequest,
};
use communication_protocol::{
    CodexGeneration, EndpointRef, LocalMutationEvidence, LocalMutationState, NativeSendReceipt,
    RunFailure, RunFailureKind, RunFailureStage, RunNextAction, RunRecoveryRequest, RunShowRequest,
    SessionRef,
};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::Mutex;
pub(crate) struct RunRequest<'a> {
    pub id: Value,
    pub method: &'a str,
    pub params: Value,
    pub store: Option<&'a Arc<Mutex<AutomationStore>>>,
}
pub(crate) async fn dispatch(request: RunRequest<'_>) -> Value {
    let mut context = RunFailure {
        kind: RunFailureKind::InvalidField,
        stage: RunFailureStage::Validation,
        message: "Provide exact Run and operation identities using the published request fields."
            .into(),
        operation_id: request
            .params
            .get("operationId")
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok()),
        run_id: request
            .params
            .get("runId")
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok()),
        effects: LocalMutationEvidence::Local {
            mutation: LocalMutationState::None,
        },
        next_action: RunNextAction::CorrectRequest,
    };
    let Some(store) = request.store else {
        context.kind = RunFailureKind::AutomationUnavailable;
        context.message = "Automation storage unavailable; no Run mutation dispatched.".into();
        context.next_action = RunNextAction::RetryLater;
        return failure(request.id, context);
    };
    let result = match request.method {
        "run/show" => {
            let params = match serde_json::from_value::<RunShowRequest>(request.params) {
                Ok(params) => params,
                Err(_) => return failure(request.id, context),
            };
            store
                .lock()
                .await
                .read_run::<SessionRef, EndpointRef, CodexGeneration, NativeSendReceipt>(
                    &params.run_id,
                )
                .await
        }
        "run/summaryRetry" | "run/summarySkip" => {
            let params = match serde_json::from_value::<RunRecoveryRequest>(request.params) {
                Ok(params) => params,
                Err(_) => return failure(request.id, context),
            };
            store
                .lock()
                .await
                .recover_summary::<SessionRef, EndpointRef, CodexGeneration, NativeSendReceipt>(
                    &SummaryRecoveryRequest {
                        operation_id: params.operation_id,
                        run_id: params.run_id,
                        action: if request.method == "run/summaryRetry" {
                            SummaryRecoveryAction::Retry {
                                timeout_seconds: 900,
                            }
                        } else {
                            SummaryRecoveryAction::Skip
                        },
                        now_ms: chrono::Utc::now().timestamp_millis(),
                    },
                )
                .await
        }
        _ => return failure(request.id, context),
    };
    match result {
        Ok(record) => match crate::run_projection::snapshot(record) {
            Ok(result) => json!({"jsonrpc":"2.0","id":request.id,"result":result}),
            Err(()) => {
                context.kind = RunFailureKind::InvalidRecord;
                context.stage = RunFailureStage::Inspection;
                context.message =
                    "Stored Run cannot be projected consistently; inspect it before retrying."
                        .into();
                context.next_action = RunNextAction::InspectRun;
                if request.method != "run/show" {
                    context.effects = LocalMutationEvidence::Local {
                        mutation: LocalMutationState::Committed,
                    };
                }
                failure(request.id, context)
            }
        },
        Err(error) => {
            context.stage = if request.method == "run/show" {
                RunFailureStage::Inspection
            } else {
                RunFailureStage::Recovery
            };
            context.kind = match error {
                StorageError::OperationConflict => RunFailureKind::OperationConflict,
                StorageError::Database(_) => RunFailureKind::AutomationUnavailable,
                _ if request.method == "run/show" => RunFailureKind::ResourceNotFound,
                _ => RunFailureKind::RecoveryNotAllowed,
            };
            context.message="Run recovery requires a blocked/required summary, unchanged attempt identity and confirmed cessation or proven non-submission. Inspect current Run and attempt before retry or skip.".into();
            if matches!(error, StorageError::Database(_)) && request.method != "run/show" {
                context.effects = LocalMutationEvidence::Local {
                    mutation: LocalMutationState::Unknown,
                };
                context.next_action = RunNextAction::InspectOperation;
            } else {
                context.next_action = RunNextAction::InspectRun;
            }
            failure(request.id, context)
        }
    }
}
fn failure(id: Value, data: RunFailure) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32050,"message":"Run request failed","data":data}})
}

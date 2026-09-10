//! Preparation commits intent before native allocation and commits the returned binding only once.
use crate::native_thread_preparation::{self, NativePreparationInput};
use automation_storage::{
    AutomationStore, ExternalAdmission, ExternalAdmissionResult, PreparationFailureDisposition,
    PreparationFailureRecord, PreparationIntent, PreparedThread, ScheduleInspection,
};
use communication_protocol::{
    EndpointRef, NativeEffectEvidence, PreparationEffect, ScheduleEffects, ScheduleFailure,
    ScheduleFailureKind, ScheduleFailureStage, ScheduleNextAction, SchedulePrepareRequest,
    SessionRef, UuidIdentity,
};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::Mutex;
pub(crate) struct PreparationRequest<'a> {
    pub id: Value,
    pub params: Value,
    pub configuration: &'a crate::AutomationConfigurationHandle,
    pub service_id: &'a UuidIdentity,
    pub backend: Option<&'a crate::NativeControlBackend>,
    pub store: Option<&'a Arc<Mutex<AutomationStore>>>,
}
pub(crate) async fn dispatch(request: PreparationRequest<'_>) -> Value {
    let params = match serde_json::from_value::<SchedulePrepareRequest>(request.params) {
        Ok(params) => params,
        Err(_) => {
            return json!({"jsonrpc":"2.0","id":request.id,"error":{"code":-32602,"message":"Provide operationId, scheduleId and an explicit fresh/fork/existing destination."}});
        }
    };
    let effects = native_thread_preparation::initial_effects(&params.destination);
    let Some(store) = request.store else {
        return reject(
            request.id,
            &params,
            ScheduleFailureKind::AutomationUnavailable,
            "Automation storage unavailable; no preparation dispatched.",
            effects,
        );
    };
    if !std::path::Path::new(native_thread_preparation::cwd(&params.destination)).is_absolute()
        || &native_thread_preparation::endpoint(&params.destination).service_id
            != request.service_id
    {
        return reject(
            request.id,
            &params,
            ScheduleFailureKind::InvalidField,
            "Use an absolute workspace and an endpoint in this service.",
            effects,
        );
    }
    let inspected = store
        .lock()
        .await
        .inspect_schedule::<SessionRef, EndpointRef>(&params.schedule_id)
        .await;
    let inspected = match inspected {
        Ok(inspected) => inspected,
        Err(_) => {
            return reject(
                request.id,
                &params,
                ScheduleFailureKind::ResourceNotFound,
                "Schedule could not be inspected; no native operation dispatched.",
                effects,
            );
        }
    };
    if matches!(
        inspected.record.definition.destination,
        agent_automation::ExecutionDestination::FreshEachRun { .. }
    ) {
        return reject(
            request.id,
            &params,
            ScheduleFailureKind::InvalidField,
            "Execution mode is fixed at creation. A fresh-per-run schedule cannot own one prepared thread; create a new reuse-mode schedule. No native operation was dispatched.",
            effects,
        );
    }
    let canonical = match serde_json::to_vec(&(&params.schedule_id, &params.destination)) {
        Ok(value) => value,
        Err(_) => {
            return reject(
                request.id,
                &params,
                ScheduleFailureKind::InvalidField,
                "Preparation request cannot be encoded.",
                effects,
            );
        }
    };
    let admitted = store
        .lock()
        .await
        .admit_schedule_preparation::<_, ScheduleInspection<SessionRef, EndpointRef>>(
            &ExternalAdmission {
                operation_id: params.operation_id.clone(),
                schedule_id: params.schedule_id.clone(),
                canonical_request: canonical,
                evidence: effects.clone(),
                now_ms: chrono::Utc::now().timestamp_millis(),
            },
        )
        .await;
    match admitted {
        Ok(ExternalAdmissionResult::Existing(record)) => {
            if let Some(result) = record.result {
                return match crate::schedule_projection::snapshot(result) {
                    Ok(result) => json!({"jsonrpc":"2.0","id":request.id,"result":result}),
                    Err(()) => reject(
                        request.id,
                        &params,
                        ScheduleFailureKind::InvalidRecord,
                        "Stored preparation result is inconsistent; inspect operation.",
                        record.evidence,
                    ),
                };
            }
            if let Some(failure) = record.failure {
                return json!({"jsonrpc":"2.0","id":request.id,"error":{"code":-32050,"message":"Retained preparation outcome","data":failure}});
            }
            return reject(
                request.id,
                &params,
                ScheduleFailureKind::OutcomeUnknown,
                "Preparation was already admitted; inspect this exact operation rather than creating another native thread.",
                record.evidence,
            );
        }
        Ok(ExternalAdmissionResult::New) => {}
        Err(_) => {
            return reject(
                request.id,
                &params,
                ScheduleFailureKind::OperationConflict,
                "Preparation identity conflicts or another preparation remains unresolved; inspect the operation.",
                effects,
            );
        }
    }
    let backend = request.backend.filter(|backend| {
        backend.endpoint == *native_thread_preparation::endpoint(&params.destination)
    });
    let admission = backend.and_then(|backend| backend.gate.acquire().ok());
    let Some(admission) = admission else {
        return finish_failure(
            store,
            request.id,
            &params,
            ScheduleFailureKind::UnsupportedCapability,
            "Selected native endpoint is unavailable; no allocation dispatched.",
            effects,
            false,
        )
        .await;
    };
    let required = match params.destination {
        communication_protocol::DestinationPreparation::Fresh { .. } => {
            codex_native_integration::NativeOperation::StartThread
        }
        communication_protocol::DestinationPreparation::Fork { .. } => {
            codex_native_integration::NativeOperation::ForkThread
        }
        communication_protocol::DestinationPreparation::Existing { .. } => {
            codex_native_integration::NativeOperation::ReadThread
        }
    };
    if admission
        .schemas()
        .is_none_or(|schemas| !schemas.supports_operation(required))
    {
        return finish_failure(
            store,
            request.id,
            &params,
            ScheduleFailureKind::UnsupportedCapability,
            "Native schema does not support the requested preparation operation.",
            effects,
            false,
        )
        .await;
    }
    let instruction = store
        .lock()
        .await
        .read_instruction(&inspected.record.definition.instruction_id)
        .await;
    let instruction = match instruction {
        Ok(instruction) => instruction,
        Err(_) => {
            return finish_failure(
                store,
                request.id,
                &params,
                ScheduleFailureKind::ResourceNotFound,
                "Instruction document unavailable; no native allocation dispatched.",
                effects,
                false,
            )
            .await;
        }
    };
    let mut text = instruction.text.as_str().to_owned();
    if let agent_automation::ContinuityInput::ImportedSummary { text: summary, .. } =
        &inspected.record.imported_continuity
    {
        text.push_str("\n\nPrior scheduled-work summary (context, not proof):\n");
        text.push_str(summary);
    }
    let mut intent = effects;
    intent.generation = Some(admission.generation().clone());
    if !matches!(
        params.destination,
        communication_protocol::DestinationPreparation::Existing { .. }
    ) {
        intent.allocation = PreparationEffect::Unknown;
    }
    let configuration_lease = request.configuration.admission_lease().await;
    if configuration_lease.configuration().is_none() {
        return finish_failure(
            store,
            request.id,
            &params,
            ScheduleFailureKind::AutomationUnavailable,
            "Configuration reconciliation is pending; native preparation was not dispatched.",
            intent,
            false,
        )
        .await;
    }
    match store
        .lock()
        .await
        .record_preparation_intent(&PreparationIntent {
            operation_id: params.operation_id.clone(),
            schedule_id: params.schedule_id.clone(),
            evidence: intent.clone(),
        })
        .await
    {
        Ok(true) => {}
        _ => {
            return reject(
                request.id,
                &params,
                ScheduleFailureKind::OutcomeUnknown,
                "Preparation intent could not be established; inspect the operation before resending.",
                intent,
            );
        }
    }
    drop(configuration_lease);
    let prepared = native_thread_preparation::prepare(NativePreparationInput {
        admission: &admission,
        destination: &params.destination,
        instruction_text: &text,
    })
    .await;
    let (target, effects) = match prepared {
        Ok(result) => result,
        Err(error) => {
            return finish_failure(
                store,
                request.id,
                &params,
                if error.uncertain {
                    ScheduleFailureKind::OutcomeUnknown
                } else {
                    ScheduleFailureKind::UnsupportedCapability
                },
                error.explanation,
                *error.effects,
                error.uncertain,
            )
            .await;
        }
    };
    let result = store
        .lock()
        .await
        .complete_thread_preparation::<_, EndpointRef, _>(PreparedThread {
            operation_id: params.operation_id.clone(),
            schedule_id: params.schedule_id.clone(),
            expected_change_id: inspected.record.change_id,
            target: target.clone(),
            service_id: String::from(target.endpoint.service_id),
            endpoint_id: String::from(target.endpoint.endpoint_id),
            thread_id: String::from(target.session_id),
            cwd: native_thread_preparation::cwd(&params.destination).into(),
            evidence: effects.clone(),
            now_ms: chrono::Utc::now().timestamp_millis(),
        })
        .await;
    match result {
        Ok(result)=>match crate::schedule_projection::snapshot(result){Ok(result)=>json!({"jsonrpc":"2.0","id":request.id,"result":result}),Err(())=>reject(request.id,&params,ScheduleFailureKind::OutcomeUnknown,"Preparation committed but its response could not be projected; inspect operation.",effects)},
        Err(automation_storage::StorageError::ThreadOwnershipConflict { .. }) => finish_failure(
            store, request.id, &params, ScheduleFailureKind::OwnershipConflict,
            "The thread belongs to another schedule. Select a different thread; no binding was committed for this schedule.",
            effects, false,
        ).await,
        Err(_)=>finish_failure(store,request.id,&params,ScheduleFailureKind::OutcomeUnknown,"Native preparation completed but binding commit was not established; inspect retained target and operation before retrying.",effects,true).await,
    }
}
fn error(
    params: &SchedulePrepareRequest,
    kind: ScheduleFailureKind,
    message: &str,
    effects: NativeEffectEvidence,
) -> ScheduleFailure {
    ScheduleFailure {
        kind,
        stage: ScheduleFailureStage::Preparation,
        message: message.into(),
        operation_id: Some(params.operation_id.clone()),
        schedule_id: Some(params.schedule_id.clone()),
        current_change_id: None,
        field: matches!(kind, ScheduleFailureKind::InvalidField).then(|| "destination".into()),
        constraint: matches!(kind, ScheduleFailureKind::InvalidField).then(|| message.into()),
        details: communication_protocol::ScheduleFailureDetails::None,
        effects: ScheduleEffects::Native { evidence: effects },
        next_action: match kind {
            ScheduleFailureKind::OwnershipConflict => ScheduleNextAction::SelectDifferentThread,
            ScheduleFailureKind::InvalidField => ScheduleNextAction::CorrectRequest,
            _ => ScheduleNextAction::InspectOperation,
        },
    }
}
fn reject(
    id: Value,
    params: &SchedulePrepareRequest,
    kind: ScheduleFailureKind,
    message: &str,
    effects: NativeEffectEvidence,
) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32050,"message":"Schedule preparation failed","data":error(params,kind,message,effects)}})
}
async fn finish_failure(
    store: &Arc<Mutex<AutomationStore>>,
    id: Value,
    params: &SchedulePrepareRequest,
    kind: ScheduleFailureKind,
    message: &str,
    effects: NativeEffectEvidence,
    uncertain: bool,
) -> Value {
    let failure = error(params, kind, message, effects.clone());
    let recorded = store
        .lock()
        .await
        .record_preparation_failure(PreparationFailureRecord {
            operation_id: params.operation_id.clone(),
            schedule_id: params.schedule_id.clone(),
            disposition: if uncertain {
                PreparationFailureDisposition::Uncertain
            } else {
                PreparationFailureDisposition::Failed
            },
            evidence: effects.clone(),
            error: &failure,
        })
        .await;
    if recorded.is_err() {
        return reject(
            id,
            params,
            ScheduleFailureKind::OutcomeUnknown,
            "Preparation evidence could not be committed; inspect the original operation before retrying.",
            effects,
        );
    }
    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32050,"message":"Schedule preparation failed","data":failure}})
}

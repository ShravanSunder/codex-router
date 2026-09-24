//! Explicit preparation validates the request and persists the selected route's effects.
use crate::{
    PreparationEvidenceSink, ScheduleDestination, SchedulePreparationOutcome,
    SchedulePreparationRequest, ScheduledRunExecution,
};
use agent_automation::RouteEffectEvidence;
use automation_storage::{
    AutomationStore, ExternalAdmission, ExternalAdmissionResult, PreparationFailureDisposition,
    PreparationFailureRecord, ScheduleInspection,
};
use collaboration_protocol::{
    CodexGeneration, DestinationPreparation, EndpointRef, ScheduleEffects, ScheduleFailure,
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
    pub execution: Option<&'a Arc<dyn ScheduledRunExecution>>,
    pub store: Option<&'a Arc<Mutex<AutomationStore>>>,
}
pub(crate) async fn dispatch(request: PreparationRequest<'_>) -> Value {
    let params = match serde_json::from_value::<SchedulePrepareRequest>(request.params) {
        Ok(params) => params,
        Err(_) => {
            return json!({"jsonrpc":"2.0","id":request.id,"error":{"code":-32602,"message":"Provide operationId, scheduleId and an explicit fresh/fork/existing destination."}});
        }
    };
    let destination = match &params.destination {
        DestinationPreparation::Existing { target, .. } => ScheduleDestination::Existing {
            target: target.clone(),
        },
        DestinationPreparation::Fresh { endpoint, .. } => ScheduleDestination::Fresh {
            endpoint: endpoint.clone(),
        },
        DestinationPreparation::Fork {
            source,
            through_turn_id,
            ..
        } => ScheduleDestination::Fork {
            source: source.clone(),
            through_turn_id: through_turn_id.clone(),
        },
    };
    let selected_effects = match request.execution {
        Some(execution) => execution
            .initial_evidence(&destination)
            .await
            .ok()
            .filter(|evidence| evidence.is_valid()),
        None => None,
    };
    let Some(store) = request.store else {
        return reject_preselection(
            request.id,
            &params,
            ScheduleFailureKind::AutomationUnavailable,
            "Automation storage unavailable; no preparation dispatched.",
            selected_effects.as_ref(),
        );
    };
    if !std::path::Path::new(destination_cwd(&params.destination)).is_absolute()
        || &destination_endpoint(&params.destination).service_id != request.service_id
    {
        return reject_preselection(
            request.id,
            &params,
            ScheduleFailureKind::InvalidField,
            "Use an absolute workspace and an endpoint in this service.",
            selected_effects.as_ref(),
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
            return reject_preselection(
                request.id,
                &params,
                ScheduleFailureKind::ResourceNotFound,
                "Schedule could not be inspected; no client operation dispatched.",
                selected_effects.as_ref(),
            );
        }
    };
    let valid_choice = |value: Option<&String>| {
        value.is_some_and(|value| {
            !value.trim().is_empty() && !value.chars().any(char::is_whitespace)
        })
    };
    if !valid_choice(inspected.record.definition.effort.as_ref()) {
        return reject_choice_field(request.id, &params, "effort", selected_effects.as_ref());
    }
    if matches!(
        params.destination,
        collaboration_protocol::DestinationPreparation::Fresh { .. }
            | collaboration_protocol::DestinationPreparation::Fork { .. }
    ) && !valid_choice(inspected.record.definition.model.as_ref())
    {
        return reject_choice_field(request.id, &params, "model", selected_effects.as_ref());
    }
    if inspected.record.definition.destination.execution_mode()
        == agent_automation::ExecutionMode::FreshEachRun
    {
        return reject_preselection(
            request.id,
            &params,
            ScheduleFailureKind::InvalidField,
            "Execution mode is fixed at creation. Use schedule update to configure a freshEachRun endpoint and workspace; schedule prepare creates a reuse-thread binding and cannot change mode. No client operation was dispatched.",
            selected_effects.as_ref(),
        );
    }
    let Some(effects) = selected_effects else {
        return reject_without_route(
            request.id,
            &params,
            "No scheduled preparation route is available for this destination.",
        );
    };
    let Some(execution) = request.execution else {
        return reject_without_route(
            request.id,
            &params,
            "Scheduled preparation route unavailable.",
        );
    };
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
                let failure =
                    match crate::schedule_preparation_evidence_sink::upgrade_stored_failure(failure)
                    {
                        Ok(failure) => failure,
                        Err(_) => {
                            return reject(
                                request.id,
                                &params,
                                ScheduleFailureKind::InvalidRecord,
                                "Stored preparation failure is invalid; inspect the operation.",
                                record.evidence,
                            );
                        }
                    };
                return json!({"jsonrpc":"2.0","id":request.id,"error":{"code":-32050,"message":"Retained preparation outcome","data":failure}});
            }
            return reject(
                request.id,
                &params,
                ScheduleFailureKind::OutcomeUnknown,
                "Preparation was already admitted; inspect this exact operation rather than creating another session.",
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
                "Instruction document unavailable; no client allocation dispatched.",
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
    let sink = crate::schedule_preparation_evidence_sink::StoredPreparationEvidenceSink {
        store: Arc::clone(store),
        configuration: request.configuration.clone(),
        operation_id: params.operation_id.clone(),
        schedule_id: params.schedule_id.clone(),
        expected_change_id: inspected.record.change_id,
        working_directory: destination_cwd(&params.destination).to_owned(),
    };
    let result = execution
        .prepare_destination(
            SchedulePreparationRequest {
                operation_id: params.operation_id.clone(),
                schedule_id: params.schedule_id.clone(),
                expected_change_id: sink.expected_change_id.clone(),
                destination: params.destination.clone(),
                instruction_text: text,
                model: inspected.record.definition.model.clone(),
                effort: inspected
                    .record
                    .definition
                    .effort
                    .clone()
                    .unwrap_or_default(),
            },
            &sink,
        )
        .await;
    match result {
        Ok(SchedulePreparationOutcome::Prepared(prepared)) => {
            let prepared_effects = prepared.evidence.clone();
            if crate::schedule_preparation_evidence_sink::project_evidence(&prepared_effects).is_err() {
                return reject(request.id, &params, ScheduleFailureKind::OutcomeUnknown,
                    "Prepared route evidence could not be projected; inspect the operation.", effects);
            }
            let effects = prepared_effects;
            match sink.record_prepared(&prepared).await {
                Ok(result) => match crate::schedule_projection::snapshot(result) {
                    Ok(result) => json!({"jsonrpc":"2.0","id":request.id,"result":result}),
                    Err(()) => reject(
                        request.id,
                        &params,
                        ScheduleFailureKind::OutcomeUnknown,
                        "Preparation committed but its response could not be projected; inspect operation.",
                        effects,
                    ),
                },
                Err(crate::DeliveryContractError::PreparationOwnershipConflict) => finish_failure(
                    store, request.id, &params, ScheduleFailureKind::OwnershipConflict,
                    "The thread belongs to another schedule. Select a different thread; no binding was committed for this schedule.",
                    effects, false,
                ).await,
                Err(crate::DeliveryContractError::PreparationChangeConflict) => finish_failure(
                    store, request.id, &params, ScheduleFailureKind::ChangeConflict,
                    "The schedule changed during preparation; no binding was committed. Inspect the current schedule and retained route effects before preparing again.",
                    effects, false,
                ).await,
                Err(_) => finish_failure(
                    store, request.id, &params, ScheduleFailureKind::OutcomeUnknown,
                    "Preparation completed but binding commit was not established; inspect retained target and operation before retrying.",
                    effects, true,
                ).await,
            }
        }
        Ok(SchedulePreparationOutcome::Failed(failure)) => {
            let failed_effects = failure.evidence.clone();
            if crate::schedule_preparation_evidence_sink::project_evidence(&failed_effects).is_err() {
                return reject(request.id, &params, ScheduleFailureKind::OutcomeUnknown,
                    "Preparation route evidence could not be projected; inspect the operation.", effects);
            }
            let effects = failed_effects;
            let public_failure = error(&params, failure.kind, &failure.explanation, effects.clone());
            if sink
                .record_failure(&public_failure, failure.evidence, failure.uncertain)
                .await
                .is_err()
            {
                return reject(
                    request.id,
                    &params,
                    ScheduleFailureKind::OutcomeUnknown,
                    "Preparation evidence could not be committed; inspect the original operation before retrying.",
                    effects,
                );
            }
            json!({"jsonrpc":"2.0","id":request.id,"error":{"code":-32050,"message":"Schedule preparation failed","data":public_failure}})
        }
        Err(_) => finish_failure(
            store,
            request.id,
            &params,
            ScheduleFailureKind::OutcomeUnknown,
            "Preparation route failed after admission; inspect the original operation before retrying.",
            effects,
            true,
        )
        .await,
    }
}
fn destination_endpoint(destination: &DestinationPreparation) -> &EndpointRef {
    match destination {
        DestinationPreparation::Existing { target, .. } => &target.endpoint,
        DestinationPreparation::Fresh { endpoint, .. } => endpoint,
        DestinationPreparation::Fork { source, .. } => &source.endpoint,
    }
}
fn destination_cwd(destination: &DestinationPreparation) -> &str {
    match destination {
        DestinationPreparation::Existing { cwd, .. }
        | DestinationPreparation::Fresh { cwd, .. }
        | DestinationPreparation::Fork { cwd, .. } => cwd,
    }
}
fn reject_choice_field(
    id: Value,
    params: &SchedulePrepareRequest,
    field: &str,
    effects: Option<&RouteEffectEvidence<SessionRef, CodexGeneration>>,
) -> Value {
    let failure = ScheduleFailure {
        kind: ScheduleFailureKind::InvalidField,
        stage: ScheduleFailureStage::Preparation,
        message: format!("Schedule preparation requires {field}."),
        operation_id: Some(params.operation_id.clone()),
        schedule_id: Some(params.schedule_id.clone()),
        current_change_id: None,
        field: Some(field.into()),
        constraint: Some("is required and must not contain whitespace".into()),
        details: collaboration_protocol::ScheduleFailureDetails::None,
        effects: effects
            .map(public_effects)
            .unwrap_or(ScheduleEffects::Local {
                mutation: collaboration_protocol::LocalMutationState::None,
            }),
        next_action: ScheduleNextAction::CorrectRequest,
    };
    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32050,"message":"Schedule preparation failed","data":failure}})
}
fn reject_preselection(
    id: Value,
    params: &SchedulePrepareRequest,
    kind: ScheduleFailureKind,
    message: &str,
    effects: Option<&RouteEffectEvidence<SessionRef, CodexGeneration>>,
) -> Value {
    if let Some(effects) = effects {
        return reject(id, params, kind, message, effects.clone());
    }
    let failure = ScheduleFailure {
        kind,
        stage: ScheduleFailureStage::Preparation,
        message: message.into(),
        operation_id: Some(params.operation_id.clone()),
        schedule_id: Some(params.schedule_id.clone()),
        current_change_id: None,
        field: matches!(kind, ScheduleFailureKind::InvalidField).then(|| "destination".into()),
        constraint: matches!(kind, ScheduleFailureKind::InvalidField).then(|| message.into()),
        details: collaboration_protocol::ScheduleFailureDetails::None,
        effects: ScheduleEffects::Local {
            mutation: collaboration_protocol::LocalMutationState::None,
        },
        next_action: if matches!(kind, ScheduleFailureKind::InvalidField) {
            ScheduleNextAction::CorrectRequest
        } else {
            ScheduleNextAction::InspectOperation
        },
    };
    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32050,"message":"Schedule preparation failed","data":failure}})
}
fn error(
    params: &SchedulePrepareRequest,
    kind: ScheduleFailureKind,
    message: &str,
    effects: RouteEffectEvidence<SessionRef, CodexGeneration>,
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
        details: collaboration_protocol::ScheduleFailureDetails::None,
        effects: public_effects(&effects),
        next_action: match kind {
            ScheduleFailureKind::OwnershipConflict => ScheduleNextAction::SelectDifferentThread,
            ScheduleFailureKind::ChangeConflict => ScheduleNextAction::InspectSchedule,
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
    effects: RouteEffectEvidence<SessionRef, CodexGeneration>,
) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32050,"message":"Schedule preparation failed","data":error(params,kind,message,effects)}})
}
async fn finish_failure(
    store: &Arc<Mutex<AutomationStore>>,
    id: Value,
    params: &SchedulePrepareRequest,
    kind: ScheduleFailureKind,
    message: &str,
    effects: RouteEffectEvidence<SessionRef, CodexGeneration>,
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

fn public_effects(effects: &RouteEffectEvidence<SessionRef, CodexGeneration>) -> ScheduleEffects {
    match crate::schedule_preparation_evidence_sink::project_evidence(effects) {
        Ok(evidence) => ScheduleEffects::Route { evidence },
        Err(_) => ScheduleEffects::Local {
            mutation: collaboration_protocol::LocalMutationState::None,
        },
    }
}

fn reject_without_route(id: Value, params: &SchedulePrepareRequest, message: &str) -> Value {
    let failure = ScheduleFailure {
        kind: ScheduleFailureKind::AutomationUnavailable,
        stage: ScheduleFailureStage::Preparation,
        message: message.into(),
        operation_id: Some(params.operation_id.clone()),
        schedule_id: Some(params.schedule_id.clone()),
        current_change_id: None,
        field: None,
        constraint: None,
        details: collaboration_protocol::ScheduleFailureDetails::None,
        effects: ScheduleEffects::Local {
            mutation: collaboration_protocol::LocalMutationState::None,
        },
        next_action: ScheduleNextAction::InspectEndpointCapabilities,
    };
    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32050,"message":"Schedule preparation failed","data":failure}})
}

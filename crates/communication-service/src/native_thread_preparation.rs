//! Exact native preparation calls use the admitted schema and never submit a turn.
use crate::NativeAdmission;
use codex_native_integration::{NativeConnectionError, NativeOperation, NativeProtocolConnection};
use communication_protocol::{
    CessationEvidence, DestinationPreparation, EndpointRef, NativeEffectEvidence,
    PreparationEffect, SessionRef, SubmissionEffect,
};
use serde_json::{Value, json};
pub(crate) struct NativePreparationInput<'a> {
    pub admission: &'a NativeAdmission,
    pub destination: &'a DestinationPreparation,
    pub instruction_text: &'a str,
}
pub(crate) struct NativePreparationFailure {
    pub explanation: &'static str,
    pub effects: Box<NativeEffectEvidence>,
    pub uncertain: bool,
}
pub(crate) fn initial_effects(destination: &DestinationPreparation) -> NativeEffectEvidence {
    let target = match destination {
        DestinationPreparation::Existing { target, .. } => Some(target.clone()),
        _ => None,
    };
    NativeEffectEvidence {
        target,
        generation: None,
        client_user_message_id: None,
        native_turn_id: None,
        native_submission_id: None,
        allocation: PreparationEffect::NotRequested,
        resume: PreparationEffect::NotRequested,
        submission: SubmissionEffect::NotDispatched,
        cessation: CessationEvidence::NotApplicable,
    }
}
pub(crate) fn endpoint(destination: &DestinationPreparation) -> &EndpointRef {
    match destination {
        DestinationPreparation::Fresh { endpoint, .. } => endpoint,
        DestinationPreparation::Fork { source, .. } => &source.endpoint,
        DestinationPreparation::Existing { target, .. } => &target.endpoint,
    }
}
pub(crate) fn cwd(destination: &DestinationPreparation) -> &str {
    match destination {
        DestinationPreparation::Fresh { cwd, .. }
        | DestinationPreparation::Fork { cwd, .. }
        | DestinationPreparation::Existing { cwd, .. } => cwd,
    }
}
pub(crate) async fn prepare(
    input: NativePreparationInput<'_>,
) -> Result<(SessionRef, NativeEffectEvidence), NativePreparationFailure> {
    let mut effects = initial_effects(input.destination);
    effects.generation = Some(input.admission.generation().clone());
    let allocation = !matches!(input.destination, DestinationPreparation::Existing { .. });
    let (operation, params) = match input.destination {
        DestinationPreparation::Fresh { cwd, .. } => (
            NativeOperation::StartThread,
            json!({"cwd":cwd,"developerInstructions":input.instruction_text}),
        ),
        DestinationPreparation::Fork {
            source,
            through_turn_id,
            cwd,
        } => (
            NativeOperation::ForkThread,
            json!({"threadId":String::from(source.session_id.clone()),"lastTurnId":String::from(through_turn_id.clone()),"cwd":cwd,"developerInstructions":input.instruction_text}),
        ),
        DestinationPreparation::Existing { target, .. } => (
            NativeOperation::ReadThread,
            json!({"threadId":String::from(target.session_id.clone()),"includeTurns":false}),
        ),
    };
    let Some(schemas) = input.admission.schemas() else {
        return Err(NativePreparationFailure {
            explanation: "Native preparation schema unavailable; no native mutation dispatched.",
            effects: Box::new(effects),
            uncertain: false,
        });
    };
    let retired = input.admission.retirement();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    let connection=tokio::time::timeout_at(deadline,async{tokio::select!{biased;_=retired.cancelled()=>Err(NativeConnectionError::Unavailable),result=NativeProtocolConnection::connect(input.admission.backend_path())=>result}}).await.unwrap_or(Err(NativeConnectionError::Unavailable));
    let mut connection = match connection {
        Ok(connection) => connection,
        Err(_) => {
            return Err(NativePreparationFailure {
                explanation: "Native connection unavailable; no native mutation dispatched.",
                effects: Box::new(effects),
                uncertain: false,
            });
        }
    };
    if allocation {
        effects.allocation = PreparationEffect::Unknown;
    }
    let result=tokio::time::timeout_at(deadline,async{tokio::select!{biased;result=connection.request_validated(&schemas,operation,params)=>result,_=retired.cancelled()=>Err(NativeConnectionError::OutcomeUnknown)}}).await.unwrap_or(Err(NativeConnectionError::OutcomeUnknown));
    let response = match result {
        Ok(response) => response,
        Err(error) => {
            let uncertain = allocation
                && !matches!(
                    error,
                    NativeConnectionError::Rejected { .. }
                        | NativeConnectionError::InvalidInput
                        | NativeConnectionError::Unavailable
                );
            if allocation {
                effects.allocation = if uncertain {
                    PreparationEffect::Unknown
                } else if matches!(error, NativeConnectionError::Rejected { .. }) {
                    PreparationEffect::Rejected
                } else {
                    PreparationEffect::NotDispatched
                };
            }
            return Err(NativePreparationFailure {
                explanation: if uncertain {
                    "Native allocation outcome is unknown; inspect the operation before any retry."
                } else {
                    "Native preparation rejected or unavailable; inspect the target and exported capabilities."
                },
                effects: Box::new(effects),
                uncertain,
            });
        }
    };
    let thread = response.get("thread");
    let id = thread
        .and_then(|thread| thread.get("id"))
        .and_then(Value::as_str);
    let actual_cwd = response
        .get("cwd")
        .or_else(|| thread.and_then(|thread| thread.get("cwd")))
        .and_then(Value::as_str);
    let target = id
        .and_then(|id| id.to_owned().try_into().ok())
        .map(|session_id| SessionRef {
            endpoint: endpoint(input.destination).clone(),
            session_id,
        });
    if allocation {
        effects.allocation = PreparationEffect::Accepted;
    }
    effects.target = target.clone();
    let matches_existing = match (input.destination, &target) {
        (
            DestinationPreparation::Existing {
                target: expected, ..
            },
            Some(actual),
        ) => expected == actual,
        (DestinationPreparation::Existing { .. }, None) => false,
        _ => true,
    };
    if actual_cwd != Some(cwd(input.destination)) || !matches_existing || target.is_none() {
        return Err(NativePreparationFailure {
            explanation: "Native response did not establish the requested identity/workspace; inspect retained effects before retrying.",
            effects: Box::new(effects),
            uncertain: allocation,
        });
    }
    Ok((
        target.ok_or_else(|| NativePreparationFailure {
            explanation: "Native target missing",
            effects: Box::new(effects.clone()),
            uncertain: allocation,
        })?,
        effects,
    ))
}

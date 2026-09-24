//! Explicit preparation persists route evidence and the resulting schedule binding.
use crate::{DeliveryContractError, DeliveryFuture, PreparationEvidenceSink, PreparedTarget};
use agent_automation::{RouteEffectEvidence, ScheduleId};
use automation_storage::{
    AutomationStore, PreparationFailureDisposition, PreparationFailureRecord, PreparationIntent,
    PreparedThread, ScheduleInspection, StorageError, StoredOperationState,
};
use collaboration_protocol::{
    ChangeId, CodexGeneration, DeliveryRouteEvidence, EndpointRef, OperationId, ScheduleFailure,
    SessionRef,
};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::Mutex;

pub(crate) struct StoredPreparationEvidenceSink {
    pub store: Arc<Mutex<AutomationStore>>,
    pub configuration: crate::AutomationConfigurationHandle,
    pub operation_id: OperationId,
    pub schedule_id: ScheduleId,
    pub expected_change_id: ChangeId,
    pub working_directory: String,
}

pub(crate) fn project_evidence(
    evidence: &RouteEffectEvidence<SessionRef, CodexGeneration>,
) -> Result<DeliveryRouteEvidence, DeliveryContractError> {
    if !evidence.is_valid() {
        return Err(DeliveryContractError::InvalidEvidence);
    }
    crate::delivery_route_projection::project(evidence)
        .map_err(|_| DeliveryContractError::InvalidEvidence)
}

pub(crate) fn decode_stored_evidence(
    value: Value,
) -> Result<RouteEffectEvidence<SessionRef, CodexGeneration>, StorageError> {
    if value.get("kind").is_some() {
        serde_json::from_value(value).map_err(|_| StorageError::InvalidRecord)
    } else {
        serde_json::from_value(value)
            .map(RouteEffectEvidence::CodexAppServer)
            .map_err(|_| StorageError::InvalidRecord)
    }
}

pub(crate) fn project_stored_evidence(value: Value) -> Result<DeliveryRouteEvidence, StorageError> {
    let evidence = decode_stored_evidence(value)?;
    project_evidence(&evidence).map_err(|_| StorageError::InvalidRecord)
}

pub(crate) fn upgrade_stored_failure(mut value: Value) -> Result<Value, StorageError> {
    let effects = value
        .get_mut("effects")
        .ok_or(StorageError::InvalidRecord)?;
    if effects.get("kind").and_then(Value::as_str) == Some("native") {
        let legacy = effects
            .get("evidence")
            .cloned()
            .ok_or(StorageError::InvalidRecord)?;
        let projected = project_stored_evidence(legacy)?;
        *effects = json!({"kind":"route","evidence":projected});
    }
    Ok(value)
}

fn validate_prepared_target(prepared: &PreparedTarget) -> Result<(), DeliveryContractError> {
    if !prepared.evidence.is_valid() {
        return Err(DeliveryContractError::InvalidEvidence);
    }
    let matches_target = match &prepared.evidence {
        RouteEffectEvidence::CodexAppServer(native) => {
            native.target.as_ref() == Some(&prepared.target)
        }
        RouteEffectEvidence::ProviderAcp(provider) => {
            provider.target.as_ref() == Some(&prepared.target)
        }
        RouteEffectEvidence::ClaudeCodePeer(peer) => {
            peer.session_id.as_str() == String::from(prepared.target.session_id.clone())
        }
    };
    if matches_target {
        Ok(())
    } else {
        Err(DeliveryContractError::InvalidEvidence)
    }
}

fn same_preparation_identity(
    before: &RouteEffectEvidence<SessionRef, CodexGeneration>,
    after: &RouteEffectEvidence<SessionRef, CodexGeneration>,
) -> bool {
    match (before, after) {
        (
            RouteEffectEvidence::CodexAppServer(before),
            RouteEffectEvidence::CodexAppServer(after),
        ) => {
            before.generation == after.generation
                && before.client_user_message_id == after.client_user_message_id
                && before
                    .target
                    .as_ref()
                    .is_none_or(|target| after.target.as_ref() == Some(target))
        }
        (RouteEffectEvidence::ProviderAcp(before), RouteEffectEvidence::ProviderAcp(after)) => {
            before.generation == after.generation
                && before.binding == after.binding
                && before.attempt_id == after.attempt_id
                && before
                    .target
                    .as_ref()
                    .is_none_or(|target| after.target.as_ref() == Some(target))
        }
        _ => before.same_client_identity(after),
    }
}

impl PreparationEvidenceSink for StoredPreparationEvidenceSink {
    fn record_intent(
        &self,
        evidence: RouteEffectEvidence<SessionRef, CodexGeneration>,
    ) -> DeliveryFuture<'_, bool> {
        Box::pin(async move {
            if !evidence.is_valid() {
                return Err(DeliveryContractError::InvalidEvidence);
            }
            let lease = self.configuration.admission_lease().await;
            if lease.configuration().is_none() {
                return Err(DeliveryContractError::ClientOperation);
            }
            self.store
                .lock()
                .await
                .record_preparation_intent(&PreparationIntent {
                    operation_id: self.operation_id.clone(),
                    schedule_id: self.schedule_id.clone(),
                    evidence,
                })
                .await
                .map_err(|_| DeliveryContractError::EvidencePersistence)
        })
    }

    fn record_prepared<'a>(
        &'a self,
        prepared: &'a PreparedTarget,
    ) -> DeliveryFuture<'a, ScheduleInspection<SessionRef, EndpointRef>> {
        Box::pin(async move {
            validate_prepared_target(prepared)?;
            let mut store = self.store.lock().await;
            let prior = store
                .read_operation(&self.operation_id)
                .await
                .map_err(|_| DeliveryContractError::EvidencePersistence)?;
            let StoredOperationState::InProgress { effects } = prior.state else {
                return Err(DeliveryContractError::InvalidEvidence);
            };
            let prior = decode_stored_evidence(effects)
                .map_err(|_| DeliveryContractError::InvalidEvidence)?;
            if !same_preparation_identity(&prior, &prepared.evidence) {
                return Err(DeliveryContractError::InvalidEvidence);
            }
            store
                .complete_thread_preparation::<_, EndpointRef, _>(PreparedThread {
                    operation_id: self.operation_id.clone(),
                    schedule_id: self.schedule_id.clone(),
                    expected_change_id: self.expected_change_id.clone(),
                    target: prepared.target.clone(),
                    service_id: String::from(prepared.target.endpoint.service_id.clone()),
                    endpoint_id: String::from(prepared.target.endpoint.endpoint_id.clone()),
                    thread_id: String::from(prepared.target.session_id.clone()),
                    cwd: self.working_directory.clone(),
                    evidence: prepared.evidence.clone(),
                    now_ms: chrono::Utc::now().timestamp_millis(),
                })
                .await
                .map_err(|error| match error {
                    StorageError::ThreadOwnershipConflict { .. } => {
                        DeliveryContractError::PreparationOwnershipConflict
                    }
                    StorageError::ScheduleChangeConflict => {
                        DeliveryContractError::PreparationChangeConflict
                    }
                    _ => DeliveryContractError::EvidencePersistence,
                })
        })
    }

    fn record_failure<'a>(
        &'a self,
        failure: &'a ScheduleFailure,
        evidence: RouteEffectEvidence<SessionRef, CodexGeneration>,
        uncertain: bool,
    ) -> DeliveryFuture<'a, ()> {
        Box::pin(async move {
            if !evidence.is_valid() {
                return Err(DeliveryContractError::InvalidEvidence);
            }
            self.store
                .lock()
                .await
                .record_preparation_failure(PreparationFailureRecord {
                    operation_id: self.operation_id.clone(),
                    schedule_id: self.schedule_id.clone(),
                    disposition: if uncertain {
                        PreparationFailureDisposition::Uncertain
                    } else {
                        PreparationFailureDisposition::Failed
                    },
                    evidence,
                    error: failure,
                })
                .await
                .map_err(|_| DeliveryContractError::EvidencePersistence)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use collaboration_protocol::{DestinationPreparation, PreparationEffect};
    use serde_json::json;

    #[test]
    fn legacy_native_preparation_evidence_projects_as_codex_route()
    -> Result<(), Box<dyn std::error::Error>> {
        let endpoint: EndpointRef = serde_json::from_value(json!({
            "serviceId":"00000000-0000-4000-8000-000000000001",
            "endpointId":"codex-local"
        }))?;
        let native =
            crate::native_thread_preparation::initial_effects(&DestinationPreparation::Fresh {
                endpoint,
                cwd: "/fixture".into(),
            });
        let stored = serde_json::to_value(&native)?;
        let projected = project_stored_evidence(stored.clone())?;
        if !matches!(projected, DeliveryRouteEvidence::CodexAppServer(evidence)
            if matches!(evidence.allocation, PreparationEffect::NotRequested))
        {
            return Err("legacy native preparation evidence lost allocation state".into());
        }
        let old_failure = json!({"effects":{"kind":"native","evidence":stored}});
        let upgraded = upgrade_stored_failure(old_failure)?;
        if upgraded["effects"]["kind"] != "route"
            || upgraded["effects"]["evidence"]["kind"] != "codexAppServer"
        {
            return Err("legacy failure evidence did not project as a Codex route".into());
        }
        Ok(())
    }
}

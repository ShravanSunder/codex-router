//! Run-owned evidence persistence keeps every client side effect behind a durable intent.
use crate::{
    AutomationConfigurationHandle, DeliveryContractError, DeliveryFuture, RunEvidenceDisposition,
    RunEvidenceSink,
};
use agent_automation::{
    PeerWriteEffect, PreparationEffect, RouteEffectEvidence, RunId, ScheduleId, SubmissionEffect,
};
use automation_storage::{
    AutomationStore, RunDispatchIntent, RunPreparationIntent, RunPreparedTarget, RunStopIdentity,
    RunUncertainty, ThreadBindingClaim,
};
use collaboration_protocol::{CodexGeneration, EndpointRef, SessionRef};
use std::sync::Arc;
use tokio::sync::Mutex;

pub(crate) struct StoredRunEvidenceSink {
    store: Arc<Mutex<AutomationStore>>,
    configuration: AutomationConfigurationHandle,
    run_id: RunId,
    schedule_id: ScheduleId,
    latest: Mutex<Option<RouteEffectEvidence<SessionRef, CodexGeneration>>>,
}

impl StoredRunEvidenceSink {
    pub(crate) fn new(
        store: Arc<Mutex<AutomationStore>>,
        configuration: AutomationConfigurationHandle,
        run_id: RunId,
        schedule_id: ScheduleId,
        initial: Option<RouteEffectEvidence<SessionRef, CodexGeneration>>,
    ) -> Self {
        Self {
            store,
            configuration,
            run_id,
            schedule_id,
            latest: Mutex::new(initial),
        }
    }

    pub(crate) async fn latest(&self) -> Option<RouteEffectEvidence<SessionRef, CodexGeneration>> {
        self.latest.lock().await.clone()
    }

    async fn begin_dispatch(
        &self,
        evidence: RouteEffectEvidence<SessionRef, CodexGeneration>,
    ) -> Result<RunEvidenceDisposition, DeliveryContractError> {
        let lease = self.configuration.admission_lease().await;
        let Some(configuration) = lease.configuration() else {
            return Ok(RunEvidenceDisposition::AdmissionRefused);
        };
        let admitted = self
            .store
            .lock()
            .await
            .begin_run_dispatch::<_, EndpointRef, _, crate::stored_run_receipt::StoredRunReceipt>(
                RunDispatchIntent {
                    run_id: self.run_id.clone(),
                    effects: evidence,
                    configured_timeout_seconds: u32::from(configuration.execution_timeout_seconds),
                    now_ms: chrono::Utc::now().timestamp_millis(),
                },
            )
            .await
            .map_err(|_| DeliveryContractError::EvidencePersistence)?;
        Ok(RunEvidenceDisposition::Recorded {
            timing: admitted.timing,
        })
    }
}

impl RunEvidenceSink for StoredRunEvidenceSink {
    fn record(
        &self,
        evidence: RouteEffectEvidence<SessionRef, CodexGeneration>,
    ) -> DeliveryFuture<'_, RunEvidenceDisposition> {
        Box::pin(async move {
            let disposition = match &evidence {
                RouteEffectEvidence::ProviderAcp(provider)
                    if provider.submission == SubmissionEffect::Dispatching
                        && self.latest.lock().await.is_some() =>
                {
                    self.begin_dispatch(evidence.clone()).await?
                }
                RouteEffectEvidence::ClaudeCodePeer(peer)
                    if peer.write == PeerWriteEffect::Dispatching
                        && self.latest.lock().await.is_some() =>
                {
                    self.begin_dispatch(evidence.clone()).await?
                }
                RouteEffectEvidence::CodexAppServer(native)
                    if native.submission == SubmissionEffect::Dispatching =>
                {
                    self.begin_dispatch(evidence.clone()).await?
                }
                RouteEffectEvidence::CodexAppServer(native)
                    if native.resume == PreparationEffect::Unknown && native.target.is_some() =>
                {
                    self.store
                        .lock()
                        .await
                        .retain_run_uncertainty::<_, EndpointRef, _, crate::stored_run_receipt::StoredRunReceipt>(
                            RunUncertainty {
                                run_id: self.run_id.clone(),
                                effects: native.clone(),
                            },
                        )
                        .await
                        .map_err(|_| DeliveryContractError::EvidencePersistence)?;
                    RunEvidenceDisposition::Recorded { timing: None }
                }
                RouteEffectEvidence::CodexAppServer(native)
                    if native.target.is_some()
                        && native.submission == SubmissionEffect::NotDispatched
                        && self.latest.lock().await.is_some() =>
                {
                    let target = native
                        .target
                        .as_ref()
                        .ok_or(DeliveryContractError::InvalidEvidence)?;
                    let recorded = self
                        .store
                        .lock()
                        .await
                        .record_run_target::<_, EndpointRef, _, crate::stored_run_receipt::StoredRunReceipt>(
                            RunPreparedTarget {
                                run_id: self.run_id.clone(),
                                effects: native.clone(),
                                binding: ThreadBindingClaim {
                                    schedule_id: self.schedule_id.clone(),
                                    service_id: String::from(target.endpoint.service_id.clone()),
                                    endpoint_id: String::from(target.endpoint.endpoint_id.clone()),
                                    thread_id: String::from(target.session_id.clone()),
                                    now_ms: chrono::Utc::now().timestamp_millis(),
                                },
                            },
                        )
                        .await
                        .map_err(|_| DeliveryContractError::EvidencePersistence)?;
                    if recorded {
                        RunEvidenceDisposition::Recorded { timing: None }
                    } else {
                        RunEvidenceDisposition::AdmissionRefused
                    }
                }
                RouteEffectEvidence::CodexAppServer(native)
                    if native.submission == SubmissionEffect::NotDispatched
                        && self.latest.lock().await.is_none() =>
                {
                    let recorded = self
                        .store
                        .lock()
                        .await
                        .begin_run_preparation::<_, EndpointRef, _, crate::stored_run_receipt::StoredRunReceipt>(
                            RunPreparationIntent {
                                run_id: self.run_id.clone(),
                                effects: evidence.clone(),
                            },
                        )
                        .await
                        .map_err(|_| DeliveryContractError::EvidencePersistence)?;
                    if recorded {
                        RunEvidenceDisposition::Recorded { timing: None }
                    } else {
                        RunEvidenceDisposition::AdmissionRefused
                    }
                }
                RouteEffectEvidence::CodexAppServer(_) => {
                    RunEvidenceDisposition::Recorded { timing: None }
                }
                RouteEffectEvidence::ProviderAcp(_) | RouteEffectEvidence::ClaudeCodePeer(_) => {
                    if self.latest.lock().await.is_none() {
                        let recorded = self
                            .store
                            .lock()
                            .await
                            .begin_run_preparation::<_, EndpointRef, _, crate::stored_run_receipt::StoredRunReceipt>(
                                RunPreparationIntent {
                                    run_id: self.run_id.clone(),
                                    effects: evidence.clone(),
                                },
                            )
                            .await
                            .map_err(|_| DeliveryContractError::EvidencePersistence)?;
                        if !recorded {
                            return Ok(RunEvidenceDisposition::AdmissionRefused);
                        }
                    }
                    RunEvidenceDisposition::Recorded { timing: None }
                }
            };
            *self.latest.lock().await = Some(evidence);
            Ok(disposition)
        })
    }

    fn record_stop_intent(&self) -> DeliveryFuture<'_, RunEvidenceDisposition> {
        Box::pin(async move {
            let recorded = self
                .latest()
                .await
                .ok_or(DeliveryContractError::InvalidEvidence)?;
            let identity = match recorded {
                RouteEffectEvidence::CodexAppServer(native) => RunStopIdentity::NativeTurn(
                    native
                        .native_turn_id
                        .ok_or(DeliveryContractError::InvalidEvidence)?,
                ),
                RouteEffectEvidence::ProviderAcp(provider) => {
                    RunStopIdentity::ProviderOperation(provider.attempt_id)
                }
                RouteEffectEvidence::ClaudeCodePeer(_) => {
                    return Ok(RunEvidenceDisposition::AdmissionRefused);
                }
            };
            let written = self
                .store
                .lock()
                .await
                .begin_run_stop::<SessionRef, EndpointRef, CodexGeneration, crate::stored_run_receipt::StoredRunReceipt>(
                    &self.run_id,
                    identity,
                )
                .await
                .map_err(|_| DeliveryContractError::EvidencePersistence)?;
            if written {
                #[cfg(test)]
                crate::scheduled_run_worker::worker_timeout_checkpoint("stop-intent");
                Ok(RunEvidenceDisposition::Recorded { timing: None })
            } else {
                Ok(RunEvidenceDisposition::AdmissionRefused)
            }
        })
    }
}

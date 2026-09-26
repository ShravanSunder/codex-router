//! Native scheduled-run client operations behind the injected execution route.
use crate::{
    DeliveryContractError, DeliveryFuture, FreshSessionRequest, NativeControlBackend,
    PreparationEvidenceSink, PreparedTarget, RunEvidenceDisposition, RunEvidenceSink,
    RunObservationContext, RunReconciliation, RunSettlement, RunSubmission, RunSummarySource,
    ScheduleCapability, ScheduleDestination, SchedulePreparationFailure,
    SchedulePreparationOutcome, SchedulePreparationRequest, ScheduleSupport, ScheduledRunExecution,
    ScheduledRunRoute, ScheduledRunSubmission, SettlementEvidence, StopRequestOutcome,
};
use agent_automation::{NativeEffectEvidence, PreparationEffect, RouteEffectEvidence};
use codex_native_integration::{NativeOperation, NativeProtocolConnection};
use collaboration_protocol::{
    DestinationPreparation, EndpointRef, ScheduleFailureKind, SessionRef,
};
use serde_json::{Value, json};

pub struct CodexAppServerScheduledRuns {
    backend: NativeControlBackend,
}

fn native_effects_into_automation(
    effects: collaboration_protocol::NativeEffectEvidence,
) -> Result<
    NativeEffectEvidence<SessionRef, collaboration_protocol::CodexGeneration>,
    DeliveryContractError,
> {
    serde_json::from_value(
        serde_json::to_value(effects).map_err(|_| DeliveryContractError::InvalidEvidence)?,
    )
    .map_err(|_| DeliveryContractError::InvalidEvidence)
}

impl CodexAppServerScheduledRuns {
    #[must_use]
    pub fn new(backend: NativeControlBackend) -> Self {
        Self { backend }
    }

    async fn prepare_native(
        &self,
        destination: DestinationPreparation,
        instruction_text: &str,
        model: Option<&str>,
        effort: &str,
        sink: &dyn RunEvidenceSink,
    ) -> Result<PreparedTarget, DeliveryContractError> {
        let admission = self
            .backend
            .gate
            .acquire()
            .map_err(|_| DeliveryContractError::ClientOperation)?;
        let mut intent = crate::native_thread_preparation::initial_automation_effects(&destination);
        intent.generation = Some(admission.generation().clone());
        if matches!(destination, DestinationPreparation::Fresh { .. }) {
            intent.allocation = PreparationEffect::Unknown;
        }
        if !matches!(
            sink.record(RouteEffectEvidence::CodexAppServer(intent))
                .await?,
            RunEvidenceDisposition::Recorded { .. }
        ) {
            return Err(DeliveryContractError::EvidencePersistence);
        }
        let prepared = crate::native_thread_preparation::prepare(
            crate::native_thread_preparation::NativePreparationInput {
                admission: &admission,
                destination: &destination,
                instruction_text,
                model,
                effort,
            },
        )
        .await;
        match prepared {
            Ok((target, effects)) => {
                let evidence: NativeEffectEvidence<
                    SessionRef,
                    collaboration_protocol::CodexGeneration,
                > = serde_json::from_value(
                    serde_json::to_value(effects)
                        .map_err(|_| DeliveryContractError::InvalidEvidence)?,
                )
                .map_err(|_| DeliveryContractError::InvalidEvidence)?;
                sink.record(RouteEffectEvidence::CodexAppServer(evidence.clone()))
                    .await?;
                Ok(PreparedTarget {
                    target,
                    evidence: RouteEffectEvidence::CodexAppServer(evidence),
                })
            }
            Err(failure) => {
                let evidence: NativeEffectEvidence<
                    SessionRef,
                    collaboration_protocol::CodexGeneration,
                > = serde_json::from_value(
                    serde_json::to_value(failure.effects)
                        .map_err(|_| DeliveryContractError::InvalidEvidence)?,
                )
                .map_err(|_| DeliveryContractError::InvalidEvidence)?;
                sink.record(RouteEffectEvidence::CodexAppServer(evidence))
                    .await?;
                Err(DeliveryContractError::ClientOperation)
            }
        }
    }

    async fn observe_native(
        &self,
        context: &RunObservationContext,
    ) -> Result<RunSettlement, DeliveryContractError> {
        let RouteEffectEvidence::CodexAppServer(native) = &context.recorded else {
            return Err(DeliveryContractError::InvalidEvidence);
        };
        let (Some(target), Some(turn_id)) = (&native.target, &native.native_turn_id) else {
            return Ok(RunSettlement::Pending);
        };
        if target.endpoint != self.backend.endpoint {
            return Err(DeliveryContractError::InvalidEvidence);
        }
        let admission = self
            .backend
            .gate
            .acquire()
            .map_err(|_| DeliveryContractError::ClientOperation)?;
        let retired = admission.retirement();
        let observed = tokio::select! {
            biased;
            _ = retired.cancelled() => return Ok(RunSettlement::Pending),
            observed = tokio::time::timeout(
                std::time::Duration::from_secs(20),
                crate::scheduled_native_observation::read_turn_and_choice(&admission, target, turn_id),
            ) => observed,
        };
        let Ok(Ok(Some(observed_turn))) = observed else {
            return Ok(RunSettlement::Pending);
        };
        if observed_turn.effort.as_deref()
            != context.inputs.execution_configuration.effort.as_deref()
            || (matches!(
                context.inputs.execution_configuration.destination,
                agent_automation::ExecutionDestination::FreshEachRun { .. }
            ) && observed_turn.model.as_deref()
                != context.inputs.execution_configuration.model.as_deref())
        {
            return Err(DeliveryContractError::InvalidEvidence);
        }
        if retired.is_cancelled() {
            return Ok(RunSettlement::Pending);
        }
        match observed_turn.turn.get("status").and_then(Value::as_str) {
            Some("completed") => Ok(RunSettlement::Completed {
                summary_source: Some(RunSummarySource::NativeTurn {
                    turn: crate::NativeTurnRef {
                        target: target.clone(),
                        turn_id: turn_id
                            .to_owned()
                            .try_into()
                            .map_err(|_| DeliveryContractError::InvalidEvidence)?,
                    },
                }),
            }),
            Some("failed") => Ok(RunSettlement::Failed {
                reason: "Native turn failed; inspect its recorded output.".into(),
            }),
            Some("interrupted") => Ok(RunSettlement::Interrupted),
            _ => Ok(RunSettlement::Pending),
        }
    }
}

impl ScheduledRunExecution for CodexAppServerScheduledRuns {
    fn prepare_destination<'a>(
        &'a self,
        request: SchedulePreparationRequest,
        sink: &'a dyn PreparationEvidenceSink,
    ) -> DeliveryFuture<'a, SchedulePreparationOutcome> {
        Box::pin(async move {
            let initial = crate::native_thread_preparation::initial_effects(&request.destination);
            let initial =
                RouteEffectEvidence::CodexAppServer(native_effects_into_automation(initial)?);
            let endpoint = crate::native_thread_preparation::endpoint(&request.destination);
            if *endpoint != self.backend.endpoint {
                return Ok(SchedulePreparationOutcome::Failed(
                    SchedulePreparationFailure {
                        kind: ScheduleFailureKind::UnsupportedCapability,
                        explanation:
                            "Selected native endpoint is unavailable; no allocation dispatched."
                                .into(),
                        evidence: initial,
                        uncertain: false,
                    },
                ));
            }
            let Ok(admission) = self.backend.gate.acquire() else {
                return Ok(SchedulePreparationOutcome::Failed(
                    SchedulePreparationFailure {
                        kind: ScheduleFailureKind::UnsupportedCapability,
                        explanation:
                            "Selected native endpoint is unavailable; no allocation dispatched."
                                .into(),
                        evidence: initial,
                        uncertain: false,
                    },
                ));
            };
            let required = match &request.destination {
                DestinationPreparation::Fresh { .. } => NativeOperation::StartThread,
                DestinationPreparation::Fork { .. } => NativeOperation::ForkThread,
                DestinationPreparation::Existing { .. } => NativeOperation::ReadThread,
            };
            if admission
                .schemas()
                .is_none_or(|schemas| !schemas.supports_operation(required))
            {
                return Ok(SchedulePreparationOutcome::Failed(
                    SchedulePreparationFailure {
                        kind: ScheduleFailureKind::UnsupportedCapability,
                        explanation:
                            "Native schema does not support the requested preparation operation."
                                .into(),
                        evidence: initial,
                        uncertain: false,
                    },
                ));
            }
            let mut intent =
                crate::native_thread_preparation::initial_effects(&request.destination);
            intent.generation = Some(admission.generation().clone());
            if !matches!(
                &request.destination,
                DestinationPreparation::Existing { .. }
            ) {
                intent.allocation = collaboration_protocol::PreparationEffect::Unknown;
            }
            let intent =
                RouteEffectEvidence::CodexAppServer(native_effects_into_automation(intent)?);
            let recorded = match sink.record_intent(intent.clone()).await {
                Ok(recorded) => recorded,
                Err(DeliveryContractError::ClientOperation) => {
                    return Ok(SchedulePreparationOutcome::Failed(SchedulePreparationFailure {
                        kind: ScheduleFailureKind::AutomationUnavailable,
                        explanation: "Configuration reconciliation is pending; native preparation was not dispatched.".into(),
                        evidence: intent,
                        uncertain: false,
                    }));
                }
                Err(error) => return Err(error),
            };
            if !recorded {
                return Ok(SchedulePreparationOutcome::Failed(SchedulePreparationFailure {
                    kind: ScheduleFailureKind::OutcomeUnknown,
                    explanation: "Preparation intent could not be established; inspect the operation before resending.".into(),
                    evidence: intent,
                    uncertain: true,
                }));
            }
            let result = crate::native_thread_preparation::prepare(
                crate::native_thread_preparation::NativePreparationInput {
                    admission: &admission,
                    destination: &request.destination,
                    instruction_text: &request.instruction_text,
                    model: request.model.as_deref(),
                    effort: &request.effort,
                },
            )
            .await;
            Ok(match result {
                Ok((target, effects)) => SchedulePreparationOutcome::Prepared(PreparedTarget {
                    target,
                    evidence: RouteEffectEvidence::CodexAppServer(native_effects_into_automation(
                        effects,
                    )?),
                }),
                Err(failure) => SchedulePreparationOutcome::Failed(SchedulePreparationFailure {
                    kind: if failure.uncertain {
                        ScheduleFailureKind::OutcomeUnknown
                    } else {
                        ScheduleFailureKind::UnsupportedCapability
                    },
                    explanation: failure.explanation.into(),
                    evidence: RouteEffectEvidence::CodexAppServer(native_effects_into_automation(
                        *failure.effects,
                    )?),
                    uncertain: failure.uncertain,
                }),
            })
        })
    }

    fn initial_evidence(
        &self,
        destination: &ScheduleDestination,
    ) -> DeliveryFuture<'_, RouteEffectEvidence<SessionRef, collaboration_protocol::CodexGeneration>>
    {
        let destination = destination.clone();
        Box::pin(async move {
            let preparation = match destination {
                ScheduleDestination::Existing { target } => DestinationPreparation::Existing {
                    target,
                    cwd: String::new(),
                },
                ScheduleDestination::Fresh { endpoint } => DestinationPreparation::Fresh {
                    endpoint,
                    cwd: String::new(),
                },
                ScheduleDestination::Fork {
                    source,
                    through_turn_id,
                } => DestinationPreparation::Fork {
                    source,
                    through_turn_id,
                    cwd: String::new(),
                },
            };
            let mut native =
                crate::native_thread_preparation::initial_automation_effects(&preparation);
            native.target = None;
            Ok(RouteEffectEvidence::CodexAppServer(native))
        })
    }

    fn support(&self, destination: &ScheduleDestination) -> DeliveryFuture<'_, ScheduleSupport> {
        let endpoint = match destination {
            ScheduleDestination::Existing { target } => &target.endpoint,
            ScheduleDestination::Fresh { endpoint } => endpoint,
            ScheduleDestination::Fork { source, .. } => &source.endpoint,
        };
        let mine = endpoint == &self.backend.endpoint;
        let destination = destination.clone();
        Box::pin(async move {
            let available = if mine {
                self.backend.gate.acquire().ok()
            } else {
                None
            };
            let schemas = available.as_ref().and_then(crate::NativeAdmission::schemas);
            let mut required = vec![
                (NativeOperation::ReadThread, ScheduleCapability::ReadTarget),
                (NativeOperation::StartTurn, ScheduleCapability::StartRun),
                (NativeOperation::InterruptTurn, ScheduleCapability::StopRun),
            ];
            required.push(match destination {
                ScheduleDestination::Existing { .. } => (
                    NativeOperation::ResumeThread,
                    ScheduleCapability::ReadTarget,
                ),
                ScheduleDestination::Fresh { .. } => (
                    NativeOperation::StartThread,
                    ScheduleCapability::CreateSession,
                ),
                ScheduleDestination::Fork { .. } => {
                    (NativeOperation::ForkThread, ScheduleCapability::ForkSession)
                }
            });
            let mut missing = Vec::new();
            for (operation, capability) in required {
                if schemas
                    .as_ref()
                    .is_none_or(|schemas| !schemas.supports_operation(operation))
                    && !missing.contains(&capability)
                {
                    missing.push(capability);
                }
            }
            Ok(if missing.is_empty() {
                ScheduleSupport::Supported {
                    settlement: SettlementEvidence::TurnCompletion,
                }
            } else {
                ScheduleSupport::Unsupported { missing }
            })
        })
    }

    fn prepare_existing_target<'a>(
        &'a self,
        target: &SessionRef,
        sink: &'a dyn RunEvidenceSink,
    ) -> DeliveryFuture<'a, PreparedTarget> {
        let target = target.clone();
        Box::pin(async move {
            self.prepare_native(
                DestinationPreparation::Existing {
                    target,
                    cwd: String::new(),
                },
                "",
                None,
                "",
                sink,
            )
            .await
        })
    }

    fn prepare_fresh_session<'a>(
        &'a self,
        request: FreshSessionRequest,
        sink: &'a dyn RunEvidenceSink,
    ) -> DeliveryFuture<'a, PreparedTarget> {
        Box::pin(async move {
            self.prepare_native(
                DestinationPreparation::Fresh {
                    endpoint: request.endpoint,
                    cwd: request.working_directory,
                },
                request.message.as_str(),
                request.inputs.execution_configuration.model.as_deref(),
                request
                    .inputs
                    .execution_configuration
                    .effort
                    .as_deref()
                    .unwrap_or_default(),
                sink,
            )
            .await
        })
    }

    fn submit_run<'a>(
        &'a self,
        run: ScheduledRunSubmission,
        sink: &'a dyn RunEvidenceSink,
    ) -> DeliveryFuture<'a, RunSubmission> {
        Box::pin(async move {
            crate::scheduled_native_dispatch::dispatch(&self.backend, run, sink).await
        })
    }

    fn observe_settlement(
        &self,
        context: RunObservationContext,
    ) -> DeliveryFuture<'_, RunSettlement> {
        Box::pin(async move { self.observe_native(&context).await })
    }

    fn summary_source(
        &self,
        context: RunObservationContext,
    ) -> DeliveryFuture<'_, RunSummarySource> {
        Box::pin(async move {
            let RouteEffectEvidence::CodexAppServer(native) = context.recorded else {
                return Err(DeliveryContractError::InvalidEvidence);
            };
            let (Some(target), Some(turn_id)) = (native.target, native.native_turn_id) else {
                return Err(DeliveryContractError::InvalidEvidence);
            };
            Ok(RunSummarySource::NativeTurn {
                turn: crate::NativeTurnRef {
                    target,
                    turn_id: turn_id
                        .try_into()
                        .map_err(|_| DeliveryContractError::InvalidEvidence)?,
                },
            })
        })
    }

    fn request_stop<'a>(
        &'a self,
        context: RunObservationContext,
        sink: &'a dyn RunEvidenceSink,
    ) -> DeliveryFuture<'a, StopRequestOutcome> {
        Box::pin(async move {
            let RouteEffectEvidence::CodexAppServer(native) = context.recorded else {
                return Err(DeliveryContractError::InvalidEvidence);
            };
            let (Some(target), Some(turn_id)) = (native.target, native.native_turn_id) else {
                return Ok(StopRequestOutcome::Unsupported);
            };
            let admission = self
                .backend
                .gate
                .acquire()
                .map_err(|_| DeliveryContractError::ClientOperation)?;
            let schemas = admission
                .schemas()
                .ok_or(DeliveryContractError::ClientOperation)?;
            let Ok(mut connection) =
                NativeProtocolConnection::connect(admission.backend_path()).await
            else {
                return Ok(StopRequestOutcome::Unknown);
            };
            if matches!(
                sink.record_stop_intent().await?,
                RunEvidenceDisposition::AdmissionRefused
            ) {
                return Ok(StopRequestOutcome::Unsupported);
            }
            let result = connection
                .request_validated(
                    &schemas,
                    NativeOperation::InterruptTurn,
                    json!({
                        "threadId":String::from(target.session_id),"turnId":turn_id
                    }),
                )
                .await;
            #[cfg(test)]
            crate::scheduled_run_worker::worker_timeout_checkpoint("interrupt-response");
            Ok(if result.is_ok() {
                StopRequestOutcome::Requested
            } else {
                StopRequestOutcome::Unknown
            })
        })
    }

    fn reconcile_run(
        &self,
        context: RunObservationContext,
    ) -> DeliveryFuture<'_, RunReconciliation> {
        Box::pin(async move {
            let settlement = self.observe_native(&context).await?;
            Ok(if matches!(settlement, RunSettlement::Pending) {
                RunReconciliation::StillUnknown
            } else {
                RunReconciliation::Settled { settlement }
            })
        })
    }
}

impl ScheduledRunRoute for CodexAppServerScheduledRuns {
    fn supports_endpoint(&self, endpoint: &EndpointRef) -> bool {
        endpoint == &self.backend.endpoint
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{RunEvidenceDisposition, RunEvidenceSink, ScheduledRunExecution};
    use agent_automation::{CapturedRunInputs, RouteEffectEvidence, RunId, RunPhase};
    use collaboration_protocol::CodexGeneration;
    use serde_json::json;
    use std::{
        collections::BTreeMap,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
    };

    struct CountingStopSink {
        writes: AtomicUsize,
    }

    impl RunEvidenceSink for CountingStopSink {
        fn record(
            &self,
            _: RouteEffectEvidence<SessionRef, CodexGeneration>,
        ) -> DeliveryFuture<'_, RunEvidenceDisposition> {
            Box::pin(async { Ok(RunEvidenceDisposition::Recorded { timing: None }) })
        }

        fn record_stop_intent(&self) -> DeliveryFuture<'_, RunEvidenceDisposition> {
            self.writes.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(RunEvidenceDisposition::Recorded { timing: None }) })
        }
    }

    #[tokio::test]
    async fn unavailable_stop_connection_never_calls_intent_sink()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let service = "00000000-0000-4000-8000-000000000001";
        let target: SessionRef = serde_json::from_value(json!({
            "endpoint":{"serviceId":service,"endpointId":"codex-local"},
            "sessionId":"worker-thread"
        }))?;
        let generation: CodexGeneration =
            serde_json::from_value(json!({"serviceEpoch":service,"generation":1}))?;
        let mut definitions = serde_json::Map::new();
        for name in [
            "ThreadRead",
            "ThreadTurnsList",
            "ThreadResume",
            "ThreadStart",
            "ThreadLoadedList",
            "TurnStart",
            "TurnSteer",
            "TurnInterrupt",
        ] {
            definitions.insert(format!("{name}Params"), json!({"type":"object"}));
            definitions.insert(format!("{name}Response"), json!({"type":"object"}));
        }
        let bundle =
            codex_native_integration::NativeSchemaBundle::from_documents(BTreeMap::from([(
                "codex_app_server_protocol.schemas.json".to_owned(),
                serde_json::to_vec(&json!({"definitions":{"v2":definitions}}))?,
            )]))?;
        let schemas = Arc::new(codex_native_integration::NativePayloadSchemas::from_bundle(
            &bundle,
        )?);
        let gate = crate::NativeGenerationGate::default();
        let missing_socket = std::env::temp_dir().join(format!(
            "absent-stop-{}.sock",
            agent_automation::OperationId::generate().as_str()
        ));
        gate.activate(generation.clone(), missing_socket, Some(schemas))?;
        let route = CodexAppServerScheduledRuns::new(NativeControlBackend {
            endpoint: target.endpoint.clone(),
            gate,
            codex_home: std::env::temp_dir(),
        });
        let recorded = serde_json::from_value(json!({
            "kind":"codexAppServer","target":target,"generation":generation,
            "clientUserMessageId":"run-correlation","nativeTurnId":"worker-turn",
            "nativeSubmissionId":null,"allocation":"notRequested","resume":"notRequested",
            "submission":"accepted","cessation":"unconfirmed"
        }))?;
        let inputs: CapturedRunInputs<SessionRef, EndpointRef> = serde_json::from_value(json!({
            "scheduleChangeId":agent_automation::ChangeId::generate(),
            "instructionRevisionId":agent_automation::RevisionId::generate(),
            "instructionText":"Check work",
            "continuity":{"kind":"none"},
            "executionConfiguration":{
                "destination":{"kind":"ownedThread","target":target,"cwd":"/tmp"},
                "executionTimeoutSeconds":120,"model":"fixture-model","effort":"medium"
            }
        }))?;
        let sink = CountingStopSink {
            writes: AtomicUsize::new(0),
        };
        let result = route
            .request_stop(
                RunObservationContext {
                    run_id: RunId::generate(),
                    phase: RunPhase::Executing,
                    recorded,
                    inputs,
                },
                &sink,
            )
            .await?;
        if !matches!(result, StopRequestOutcome::Unknown) || sink.writes.load(Ordering::SeqCst) != 0
        {
            return Err("unreachable client persisted a stop intent".into());
        }
        Ok(())
    }
}

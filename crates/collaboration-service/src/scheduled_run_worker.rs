//! One bounded scheduled-run step through the injected execution route.
use crate::{
    DeliveryContractError, DeliveryPrecondition, NativeControlBackend, RunObservationContext,
    RunSubmission, ScheduleDestination, ScheduledRunExecution,
};
use agent_automation::{
    ExecutionDestination, PreparationEffect, RouteEffectEvidence, RunId, RunPhase, ScheduleId,
};
use automation_storage::{
    AutomationStore, PushRecordDraft, RunSubmissionOutcome, RunSubmissionResult, RunUncertainty,
    ScheduleRunPushUpdate, StorageError,
};
use collaboration_protocol::{
    CodexGeneration, DeliveryNextAction, DeliveryOutcome, DeliveryReceipt, DeliveryRejection,
    DeliveryRejectionReason, DestinationPreparation, EndpointRef, MachineId, PushDeliveryState,
    PushHeaderFacts, PushId, PushKind, PushLineInput, PushOrigin, PushRecord, RouterLink,
    RouterOriginRef, RunExecution, SessionReachability, SessionRef, render_push_line,
};
use std::sync::Arc;
use tokio::sync::Mutex;

#[cfg(test)]
#[path = "scheduled_input_validation_tests.rs"]
mod input_validation_tests;
#[cfg(test)]
#[path = "scheduled_run_retry_tests.rs"]
mod retry_tests;
#[cfg(test)]
#[path = "scheduled_run_route_tests.rs"]
mod route_tests;
#[cfg(test)]
#[path = "worker_timeout_crash_tests.rs"]
mod timeout_crash_tests;

#[cfg(test)]
pub(crate) fn worker_timeout_checkpoint(stage: &str) {
    timeout_crash_tests::checkpoint(stage);
}

type StoredRun = agent_automation::RunRecord<
    SessionRef,
    EndpointRef,
    CodexGeneration,
    crate::stored_run_receipt::StoredRunReceipt,
>;

type SchedulePushReceipt = (PushDeliveryState, DeliveryReceipt);
type SchedulePushSettlementContext = Option<(PushId, RouterOriginRef, Option<SchedulePushReceipt>)>;

enum SchedulePushPreparation {
    Ready {
        prepared: crate::layer_zero::PreparedPush,
        origin_router_ref: RouterOriginRef,
    },
    Recover(Box<PushRecord>),
}

#[derive(Clone)]
pub(crate) struct ScheduledRunWorker {
    pub store: Arc<Mutex<AutomationStore>>,
    pub execution: Arc<dyn ScheduledRunExecution>,
    pub backend: Option<NativeControlBackend>,
    pub configuration: crate::AutomationConfigurationHandle,
    pub machine_identity: crate::MachineIdentity,
}

impl ScheduledRunWorker {
    pub async fn step(&self, id: RunId) -> Result<(), StorageError> {
        let record = self
            .store
            .lock()
            .await
            .read_run::<SessionRef, EndpointRef, CodexGeneration, crate::stored_run_receipt::StoredRunReceipt>(&id)
            .await?;
        if matches!(
            record.phase,
            RunPhase::SummaryRequired | RunPhase::SummaryRunning | RunPhase::SummaryBlocked
        ) {
            return self.step_summary(record).await;
        }
        match record.phase {
            RunPhase::Preparing => self.step_preparation(record).await,
            RunPhase::Executing | RunPhase::Stopping | RunPhase::Uncertain => {
                self.step_observation(record).await
            }
            _ => Ok(()),
        }
    }

    async fn step_summary(&self, record: StoredRun) -> Result<(), StorageError> {
        let Some(backend) = &self.backend else {
            return Ok(());
        };
        let Ok(admission) = backend.gate.acquire() else {
            return Ok(());
        };
        let seconds = if record.phase == RunPhase::SummaryRequired {
            let lease = self.configuration.admission_lease().await;
            let Some(configuration) = lease.configuration() else {
                return Ok(());
            };
            u32::from(configuration.summary_timeout_seconds)
        } else {
            record
                .summary_attempt
                .as_ref()
                .ok_or(StorageError::InvalidRecord)?
                .effective_timeout_seconds
        };
        let needs_source = record.summary_attempt.as_ref().is_some_and(|attempt| {
            attempt.phase == agent_automation::SummaryPhase::Preparing && attempt.target.is_some()
        });
        let source = if needs_source {
            let context = RunObservationContext {
                run_id: record.run_id.clone(),
                phase: record.phase,
                recorded: record
                    .evidence
                    .route
                    .clone()
                    .ok_or(StorageError::InvalidRecord)?,
                inputs: record.inputs.clone().ok_or(StorageError::InvalidRecord)?,
            };
            match self.execution.summary_source(context).await {
                Ok(source) => Some(source),
                Err(DeliveryContractError::ClientOperation) => None,
                Err(_) => return Err(StorageError::InvalidRecord),
            }
        } else {
            None
        };
        crate::summary_native_worker::step(crate::summary_native_worker::SummaryStep {
            work: crate::summary_native_worker::SummaryWork::Advance,
            store: &self.store,
            admission: &admission,
            summary_endpoint: backend.endpoint.clone(),
            source,
            record,
            timeout_seconds: seconds,
        })
        .await
    }

    async fn step_observation(&self, record: StoredRun) -> Result<(), StorageError> {
        let Some(recorded) = record.evidence.route.clone() else {
            return Err(StorageError::InvalidRecord);
        };
        let inputs = record.inputs.clone().ok_or(StorageError::InvalidRecord)?;
        let context = RunObservationContext {
            run_id: record.run_id.clone(),
            phase: record.phase,
            recorded: recorded.clone(),
            inputs,
        };
        let settlement = match self.execution.observe_settlement(context).await {
            Ok(settlement) => settlement,
            Err(DeliveryContractError::ClientOperation) => return Ok(()),
            Err(_) => return Err(StorageError::InvalidRecord),
        };
        if crate::run_reconciliation::persist_settlement(&self.store, &record, settlement).await? {
            return Ok(());
        }
        if record.phase == RunPhase::Stopping
            || !record.evidence.timing.as_ref().is_some_and(|timing| {
                timing.deadline_at_ms <= chrono::Utc::now().timestamp_millis()
            })
        {
            return Ok(());
        }
        let sink = crate::scheduled_run_evidence_sink::StoredRunEvidenceSink::new(
            Arc::clone(&self.store),
            self.configuration.clone(),
            record.run_id.clone(),
            record.schedule_id.clone(),
            Some(recorded.clone()),
        );
        let context = RunObservationContext {
            run_id: record.run_id,
            phase: record.phase,
            recorded,
            inputs: record.inputs.ok_or(StorageError::InvalidRecord)?,
        };
        match self.execution.request_stop(context, &sink).await {
            Ok(_) | Err(DeliveryContractError::ClientOperation) => {}
            Err(_) => return Err(StorageError::InvalidRecord),
        }
        Ok(())
    }

    async fn step_preparation(&self, record: StoredRun) -> Result<(), StorageError> {
        let id = record.run_id.clone();
        let inputs = record.inputs.clone().ok_or(StorageError::InvalidRecord)?;
        if record.evidence.timing.is_some()
            || record
                .evidence
                .route
                .as_ref()
                .is_some_and(route_preparation_unknown)
        {
            if record.evidence.timing.is_some()
                && self.recover_started_schedule_push(&record).await?
            {
                return Ok(());
            }
            if let Some(RouteEffectEvidence::CodexAppServer(mut native)) = record.evidence.route {
                native.submission = agent_automation::SubmissionEffect::Unknown;
                self.store
                    .lock()
                    .await
                    .retain_run_uncertainty::<SessionRef, EndpointRef, CodexGeneration, crate::stored_run_receipt::StoredRunReceipt>(
                        RunUncertainty { run_id: id, effects: native },
                    )
                    .await?;
            }
            return Ok(());
        }
        let selected_evidence = if let Some(recorded) = record.evidence.route.clone() {
            recorded
        } else {
            let destination = match preparation_destination(&inputs)? {
                DestinationPreparation::Existing { target, .. } => {
                    ScheduleDestination::Existing { target }
                }
                DestinationPreparation::Fresh { endpoint, .. } => {
                    ScheduleDestination::Fresh { endpoint }
                }
                DestinationPreparation::Fork { .. } => return Err(StorageError::InvalidRecord),
            };
            match self.execution.initial_evidence(&destination).await {
                Ok(evidence) => evidence,
                Err(DeliveryContractError::ClientOperation) => return Ok(()),
                Err(_) => return Err(StorageError::InvalidRecord),
            }
        };
        let selected_target = route_target(&selected_evidence).or_else(|| captured_target(&inputs));
        let text = match render_instructions(
            &inputs,
            record.schedule_id.as_str(),
            id.as_str(),
            selected_target.as_ref(),
        ) {
            Ok(text) => text,
            Err(_) => {
                self.store.lock().await.fail_run_preparation::<SessionRef, EndpointRef, CodexGeneration, crate::stored_run_receipt::StoredRunReceipt>(
                    automation_storage::RunPreparationFailure {
                        run_id: id,
                        effects: selected_evidence,
                        explanation: "Combined instructions and continuity exceed the supported request size; no input was submitted; any earlier preparation effects remain recorded. Shorten instructions or continuity for future runs.".into(),
                        now_ms: chrono::Utc::now().timestamp_millis(),
                    },
                ).await?;
                return Ok(());
            }
        };
        if record.evidence.route.is_none() {
            return self.prepare_target(record, inputs, text).await;
        }
        let recorded = record
            .evidence
            .route
            .clone()
            .ok_or(StorageError::InvalidRecord)?;
        let Some(target) = route_target(&recorded).or_else(|| captured_target(&inputs)) else {
            return self.prepare_target(record, inputs, text).await;
        };
        let sink = crate::scheduled_run_evidence_sink::StoredRunEvidenceSink::new(
            Arc::clone(&self.store),
            self.configuration.clone(),
            id.clone(),
            record.schedule_id.clone(),
            Some(recorded.clone()),
        );
        let mut schedule_push_identity: SchedulePushSettlementContext = None;
        let payload = match &inputs.execution_configuration.destination {
            ExecutionDestination::OwnedThread { .. } => {
                let schedule_push = self
                    .store_schedule_push(&id, &record.schedule_id, &target, &inputs)
                    .await?;
                match schedule_push {
                    SchedulePushPreparation::Ready {
                        prepared,
                        origin_router_ref,
                    } => {
                        self.store
                            .lock()
                            .await
                            .mark_push_attempted(&prepared.push_id)
                            .await?;
                        schedule_push_identity =
                            Some((prepared.push_id.clone(), origin_router_ref, None));
                        crate::scheduled_run_contract::ScheduledRunPayload::Existing { prepared }
                    }
                    SchedulePushPreparation::Recover(push) => {
                        if push.delivery_state == PushDeliveryState::Attempted {
                            self.recover_schedule_push_attempt(&record, *push).await?;
                        }
                        return Ok(());
                    }
                }
            }
            ExecutionDestination::FreshEachRun { .. } => {
                crate::scheduled_run_contract::ScheduledRunPayload::Fresh {
                    task_input: text.try_into().map_err(|_| StorageError::InvalidRecord)?,
                }
            }
            ExecutionDestination::Unprepared | ExecutionDestination::FreshEachRunUnprepared => {
                return Err(StorageError::InvalidRecord);
            }
        };
        let submission = self
            .execution
            .submit_run(
                crate::ScheduledRunSubmission {
                    run_id: id.clone(),
                    target,
                    payload,
                    precondition: DeliveryPrecondition::Unpinned,
                    inputs,
                    recorded: recorded.clone(),
                },
                &sink,
            )
            .await;
        let latest = sink.latest().await.unwrap_or(recorded);
        let submitted = match submission {
            Ok(submitted) => submitted,
            Err(DeliveryContractError::ClientOperation) if schedule_push_identity.is_some() => {
                crate::RunSubmission::Unknown
            }
            Err(DeliveryContractError::ClientOperation) => return Ok(()),
            Err(_) => return Err(StorageError::InvalidRecord),
        };
        self.persist_submission(id, latest, submitted, schedule_push_identity)
            .await
    }

    async fn recover_started_schedule_push(
        &self,
        run_record: &StoredRun,
    ) -> Result<bool, StorageError> {
        let origin_router_ref =
            schedule_run_origin_reference(&run_record.schedule_id, &run_record.run_id);
        let mut push_record = self
            .store
            .lock()
            .await
            .get_push_record_by_origin_reference(&origin_router_ref)
            .await?;
        let Some(mut push_record) = push_record.take() else {
            return Ok(false);
        };
        if push_record.delivery_state == PushDeliveryState::Pending {
            push_record = self
                .store
                .lock()
                .await
                .mark_push_attempted(&push_record.push_id)
                .await?;
        }
        if push_record.delivery_state == PushDeliveryState::Attempted {
            self.recover_schedule_push_attempt(run_record, push_record)
                .await?;
        }
        Ok(true)
    }

    async fn store_schedule_push(
        &self,
        run_id: &RunId,
        schedule_id: &agent_automation::ScheduleId,
        target: &SessionRef,
        inputs: &agent_automation::CapturedRunInputs<SessionRef, EndpointRef>,
    ) -> Result<SchedulePushPreparation, StorageError> {
        let origin_router_ref = schedule_run_origin_reference(schedule_id, run_id);
        let encoded_origin_reference = origin_router_ref
            .canonical_string()
            .map_err(|_| StorageError::InvalidRecord)?;
        if let Some(existing) = self
            .store
            .lock()
            .await
            .get_push_record_by_origin_reference(&origin_router_ref)
            .await?
        {
            if existing.kind != PushKind::ScheduleRun
                || existing.target != *target
                || existing.origin_router_ref.as_deref() != Some(encoded_origin_reference.as_str())
                || existing.body.as_deref() != Some(inputs.instruction_text.as_str())
                || !matches!(
                    &existing.header_facts,
                    PushHeaderFacts::ScheduleRun {
                        schedule_id: stored_schedule,
                        run_id: stored_run,
                    } if stored_schedule == schedule_id && stored_run == run_id
                )
            {
                return Err(StorageError::InvalidRecord);
            }
            if existing.delivery_state != PushDeliveryState::Pending {
                return Ok(SchedulePushPreparation::Recover(Box::new(existing)));
            }
            let prepared = self.prepared_schedule_push(&existing)?;
            return Ok(SchedulePushPreparation::Ready {
                prepared,
                origin_router_ref,
            });
        }
        let push_id = PushId::try_from(uuid::Uuid::now_v7().to_string())
            .map_err(|_| StorageError::InvalidRecord)?;
        let body = inputs.instruction_text.as_str().to_owned();
        let draft = PushRecordDraft {
            mode: None,
            guard: None,
            push_id: push_id.clone(),
            kind: PushKind::ScheduleRun,
            origin: PushOrigin::Router(PushKind::ScheduleRun),
            origin_router_ref: Some(encoded_origin_reference),
            target: target.clone(),
            reply_to_push_id: None,
            header_facts: PushHeaderFacts::ScheduleRun {
                schedule_id: schedule_id.clone(),
                run_id: run_id.clone(),
            },
            body: Some(body),
            activity: None,
            created_at: chrono::Utc::now(),
        };
        let record = self.store.lock().await.insert_push_record(draft).await?;
        let prepared = self.prepared_schedule_push(&record)?;
        Ok(SchedulePushPreparation::Ready {
            prepared,
            origin_router_ref,
        })
    }

    fn prepared_schedule_push(
        &self,
        record: &PushRecord,
    ) -> Result<crate::layer_zero::PreparedPush, StorageError> {
        let line = render_push_line(&PushLineInput {
            link: RouterLink::new(
                MachineId::from(self.machine_identity.service_id().clone()),
                record.push_id.clone(),
            ),
            machine_label: self.machine_identity.machine_label().clone(),
            origin: record.origin.clone(),
            header_facts: record.header_facts.clone(),
            body: record.body.clone(),
        })
        .map_err(|_| StorageError::InvalidRecord)?
        .try_into()
        .map_err(|_| StorageError::InvalidRecord)?;
        Ok(crate::layer_zero::PreparedPush {
            push_id: record.push_id.clone(),
            line,
            load_policy: crate::LoadPolicy::MayLoad,
        })
    }

    async fn recover_schedule_push_attempt(
        &self,
        run_record: &StoredRun,
        push_record: PushRecord,
    ) -> Result<(), StorageError> {
        if push_record.delivery_state != PushDeliveryState::Attempted {
            return Err(StorageError::InvalidRecord);
        }
        let origin_router_ref = push_record
            .origin_router_ref
            .as_deref()
            .ok_or(StorageError::InvalidRecord)?;
        let expected_origin_reference =
            schedule_run_origin_reference(&run_record.schedule_id, &run_record.run_id);
        let expected_origin_ref = expected_origin_reference
            .canonical_string()
            .map_err(|_| StorageError::InvalidRecord)?;
        if origin_router_ref != expected_origin_ref {
            return Err(StorageError::InvalidRecord);
        }
        let effects = run_record
            .evidence
            .route
            .clone()
            .ok_or(StorageError::InvalidRecord)?;
        if route_submission_rejected(&effects) {
            return self
                .persist_submission(
                    run_record.run_id.clone(),
                    effects,
                    crate::RunSubmission::Rejected(DeliveryRejection {
                        reason: DeliveryRejectionReason::Unknown,
                        next_action: DeliveryNextAction::InspectTarget,
                        client_code: None,
                        detail: Some(
                            "Scheduled start was known rejected before its receipt was persisted."
                                .into(),
                        ),
                        claims: None,

                        }),
                    Some((
                        push_record.push_id,
                        expected_origin_reference,
                        Some((
                            PushDeliveryState::Pending,
                            DeliveryReceipt {
                                outcome: DeliveryOutcome::Rejected(DeliveryRejection {
                                    reason: DeliveryRejectionReason::Unknown,
                                    next_action: DeliveryNextAction::InspectTarget,
                                    client_code: None,
                                    detail: Some(
                                        "Scheduled start was known rejected before its receipt was persisted."
                                            .into(),
                                    ),
                                    claims: None,

                                    }),
                                reachability: None,
                                client: None,
                            },
                        )),
                    )),
                )
                .await;
        }
        let (push_state, receipt) = recovered_schedule_push_outcome(&effects);
        self.persist_submission(
            run_record.run_id.clone(),
            effects,
            crate::RunSubmission::Unknown,
            Some((
                push_record.push_id,
                expected_origin_reference,
                Some((push_state, receipt)),
            )),
        )
        .await
    }

    async fn prepare_target(
        &self,
        record: StoredRun,
        inputs: agent_automation::CapturedRunInputs<SessionRef, EndpointRef>,
        text: String,
    ) -> Result<(), StorageError> {
        let destination = preparation_destination(&inputs)?;
        let sink = crate::scheduled_run_evidence_sink::StoredRunEvidenceSink::new(
            Arc::clone(&self.store),
            self.configuration.clone(),
            record.run_id.clone(),
            record.schedule_id.clone(),
            None,
        );
        let prepared = match destination {
            DestinationPreparation::Existing { target, cwd } => {
                self.execution
                    .prepare_existing_target(&target, &cwd, &sink)
                    .await
            }
            DestinationPreparation::Fresh { endpoint, cwd } => {
                self.execution
                    .prepare_fresh_session(
                        crate::FreshSessionRequest {
                            endpoint,
                            working_directory: cwd,
                            message: text.try_into().map_err(|_| StorageError::InvalidRecord)?,
                            inputs,
                        },
                        &sink,
                    )
                    .await
            }
            DestinationPreparation::Fork { .. } => return Err(StorageError::InvalidRecord),
        };
        match prepared {
            Ok(_) => Ok(()),
            Err(DeliveryContractError::ClientOperation) => {
                let Some(evidence) = sink.latest().await else {
                    return Ok(());
                };
                if let RouteEffectEvidence::CodexAppServer(native) = &evidence
                    && route_preparation_unknown(&evidence)
                {
                    self.store.lock().await.retain_run_uncertainty::<SessionRef, EndpointRef, CodexGeneration, crate::stored_run_receipt::StoredRunReceipt>(
                        RunUncertainty { run_id: record.run_id, effects: native.clone() },
                    ).await?;
                } else {
                    self.store.lock().await.fail_run_preparation::<SessionRef, EndpointRef, CodexGeneration, crate::stored_run_receipt::StoredRunReceipt>(
                        automation_storage::RunPreparationFailure {
                            run_id: record.run_id,
                            effects: evidence,
                            explanation: "Route preparation could not confirm its target; no worker input was submitted.".into(),
                            now_ms: chrono::Utc::now().timestamp_millis(),
                        },
                    ).await?;
                }
                Ok(())
            }
            Err(_) => Err(StorageError::InvalidRecord),
        }
    }

    async fn persist_submission(
        &self,
        id: RunId,
        effects: RouteEffectEvidence<SessionRef, CodexGeneration>,
        submission: RunSubmission,
        schedule_push: SchedulePushSettlementContext,
    ) -> Result<(), StorageError> {
        let (outcome, push_receipt, push_state) = match submission {
            RunSubmission::NotStartedBusy => {
                if let Some((push_id, _, _)) = schedule_push {
                    self.store
                        .lock()
                        .await
                        .restore_push_pending_after_not_started(&push_id)
                        .await?;
                }
                return Ok(());
            }
            RunSubmission::Started(acceptance) => {
                let run_receipt = crate::stored_run_receipt::StoredRunReceipt::Current(
                    acceptance.receipt.clone(),
                );
                let push_receipt = acceptance.receipt;
                let outcome = match acceptance.execution {
                    RunExecution::CodexAppServer(native) => RunSubmissionOutcome::Accepted {
                        turn_id: native.turn_id,
                        receipt: run_receipt,
                    },
                    RunExecution::ProviderAcp { .. } => RunSubmissionOutcome::ProviderAdmitted {
                        receipt: run_receipt,
                    },
                    RunExecution::ClaudeCodePeer { written_at, .. } => {
                        let timestamp =
                            chrono::DateTime::parse_from_rfc3339(&String::from(written_at))
                                .map_err(|_| StorageError::InvalidRecord)?
                                .timestamp_millis();
                        RunSubmissionOutcome::PeerWritten {
                            written_at_ms: timestamp,
                            receipt: run_receipt,
                        }
                    }
                };
                (outcome, push_receipt, PushDeliveryState::Delivered)
            }
            RunSubmission::Rejected(rejection) => {
                let receipt = DeliveryReceipt {
                    outcome: DeliveryOutcome::Rejected(rejection.clone()),
                    reachability: None,
                    client: None,
                };
                (
                    RunSubmissionOutcome::Rejected {
                        explanation: rejection
                            .detail
                            .unwrap_or_else(|| format!("{:?}", rejection.reason)),
                    },
                    receipt,
                    PushDeliveryState::Pending,
                )
            }
            RunSubmission::Unknown => (
                RunSubmissionOutcome::Unknown {
                    explanation: "Scheduled start outcome unknown; no automatic resend.".into(),
                },
                DeliveryReceipt {
                    outcome: DeliveryOutcome::Unknown,
                    reachability: None,
                    client: None,
                },
                PushDeliveryState::OutcomeUnknown,
            ),
        };
        let result = RunSubmissionResult {
            run_id: id,
            effects,
            outcome,
        };
        let push_update = schedule_push.map(|(push_id, origin_router_ref, recovered_outcome)| {
            ScheduleRunPushUpdate {
                push_id,
                origin_router_ref,
                delivery_state: recovered_outcome
                    .as_ref()
                    .map_or(push_state, |(state, _)| *state),
                receipt: recovered_outcome.map_or(push_receipt, |(_, receipt)| receipt),
                settled_at: chrono::Utc::now(),
            }
        });
        let mut delay = std::time::Duration::from_millis(25);
        loop {
            let write = {
                let mut store = self.store.lock().await;
                match &push_update {
                    Some(push_update) => {
                        store
                            .record_run_submission_with_push::<_, EndpointRef, _, _>(
                                result.clone(),
                                push_update.clone(),
                            )
                            .await
                    }
                    None => {
                        store
                            .record_run_submission::<_, EndpointRef, _, _>(result.clone())
                            .await
                    }
                }
            };
            match write {
                Ok(()) => return Ok(()),
                Err(StorageError::Database(_)) if push_update.is_some() => {
                    tokio::time::sleep(delay).await;
                    delay = delay
                        .saturating_mul(2)
                        .min(std::time::Duration::from_secs(1));
                }
                Err(error) => return Err(error),
            }
        }
    }
}

fn schedule_run_origin_reference(schedule_id: &ScheduleId, run_id: &RunId) -> RouterOriginRef {
    RouterOriginRef::ScheduleRun {
        schedule_id: schedule_id.clone(),
        run_id: run_id.clone(),
    }
}

fn route_submission_rejected(evidence: &RouteEffectEvidence<SessionRef, CodexGeneration>) -> bool {
    matches!(evidence,
        RouteEffectEvidence::CodexAppServer(native)
            if native.submission == agent_automation::SubmissionEffect::Rejected
    ) || matches!(evidence,
        RouteEffectEvidence::ProviderAcp(provider)
            if provider.submission == agent_automation::SubmissionEffect::Rejected
    )
}

fn recovered_schedule_push_outcome(
    evidence: &RouteEffectEvidence<SessionRef, CodexGeneration>,
) -> (PushDeliveryState, DeliveryReceipt) {
    let (state, outcome, reachability) = match evidence {
        RouteEffectEvidence::CodexAppServer(native)
            if native.submission == agent_automation::SubmissionEffect::Accepted =>
        {
            (
                PushDeliveryState::Delivered,
                DeliveryOutcome::Started,
                Some(SessionReachability::CodexAppServer),
            )
        }
        RouteEffectEvidence::ProviderAcp(provider)
            if provider.submission == agent_automation::SubmissionEffect::Accepted =>
        {
            (
                PushDeliveryState::Delivered,
                DeliveryOutcome::Started,
                Some(SessionReachability::ProviderAcp),
            )
        }
        RouteEffectEvidence::ClaudeCodePeer(peer)
            if peer.write == agent_automation::PeerWriteEffect::Written =>
        {
            (
                PushDeliveryState::Delivered,
                DeliveryOutcome::PeerMessageWritten,
                Some(SessionReachability::ClaudeCodePeer),
            )
        }
        _ => (
            PushDeliveryState::OutcomeUnknown,
            DeliveryOutcome::Unknown,
            None,
        ),
    };
    (
        state,
        DeliveryReceipt {
            outcome,
            reachability,
            client: None,
        },
    )
}

fn route_preparation_unknown(evidence: &RouteEffectEvidence<SessionRef, CodexGeneration>) -> bool {
    matches!(evidence,
        RouteEffectEvidence::CodexAppServer(native)
            if native.allocation == PreparationEffect::Unknown
                || native.resume == PreparationEffect::Unknown)
}

fn route_target(evidence: &RouteEffectEvidence<SessionRef, CodexGeneration>) -> Option<SessionRef> {
    match evidence {
        RouteEffectEvidence::CodexAppServer(native) => native.target.clone(),
        RouteEffectEvidence::ProviderAcp(provider) => Some(provider.target.clone()),
        RouteEffectEvidence::ClaudeCodePeer(_) => None,
    }
}

fn captured_target(
    inputs: &agent_automation::CapturedRunInputs<SessionRef, EndpointRef>,
) -> Option<SessionRef> {
    match &inputs.execution_configuration.destination {
        ExecutionDestination::OwnedThread { target, .. } => Some(target.clone()),
        _ => None,
    }
}

fn preparation_destination(
    inputs: &agent_automation::CapturedRunInputs<SessionRef, EndpointRef>,
) -> Result<DestinationPreparation, StorageError> {
    match &inputs.execution_configuration.destination {
        ExecutionDestination::Unprepared | ExecutionDestination::FreshEachRunUnprepared => {
            Err(StorageError::InvalidRecord)
        }
        ExecutionDestination::OwnedThread { target, cwd } => Ok(DestinationPreparation::Existing {
            target: target.clone(),
            cwd: cwd.clone(),
        }),
        ExecutionDestination::FreshEachRun { endpoint, cwd } => Ok(DestinationPreparation::Fresh {
            endpoint: endpoint.clone(),
            cwd: cwd.clone(),
        }),
    }
}
fn render_instructions(
    inputs: &agent_automation::CapturedRunInputs<SessionRef, EndpointRef>,
    schedule_id: &str,
    run_id: &str,
    target: Option<&SessionRef>,
) -> Result<String, StorageError> {
    let mut text = format!(
        "Agent communication\nSelf-declared sender: local scheduled automation {schedule_id}\nIntended recipient: {}\nRun: {run_id}\n\n{}",
        target
            .map(serde_json::to_string)
            .transpose()
            .map_err(|_| StorageError::InvalidRecord)?
            .unwrap_or_else(|| "new execution thread".into()),
        inputs.instruction_text.as_str()
    );
    // Continued threads already have their context. Only new execution threads need continuity text.
    if target.is_none()
        && matches!(
            inputs.execution_configuration.destination,
            ExecutionDestination::FreshEachRun { .. }
        )
    {
        match &inputs.continuity {
            agent_automation::ContinuityInput::LocalSummary { text: summary, .. }
            | agent_automation::ContinuityInput::ImportedSummary { text: summary, .. } => {
                text.push_str("\n\nPrevious run summary (context, not proof):\n");
                text.push_str(summary);
            }
            _ => {}
        }
    }
    if text.len() > collaboration_protocol::MAX_CONTROL_FRAME_BYTES {
        return Err(StorageError::InvalidRecord);
    }
    Ok(text)
}

#[cfg(test)]
mod scheduled_instruction_title_tests {
    use collaboration_protocol::{PushKind, parse_push_line_header};

    #[test]
    fn scheduled_push_title_comes_from_the_header_not_preview_text() {
        let line = "🗓 Router schedule 123 @test · run 456 · \"Review the weekly summary.\" · router://00000000-0000-4000-8000-000000000001/push/018f47d2-24d5-7a68-b9ec-6f759c394599";
        let header = parse_push_line_header(line).expect("scheduled push header");

        assert_eq!(header.kind, PushKind::ScheduleRun);
        assert_eq!(header.title, "🗓 Router schedule 123 @test · run 456");
    }
}

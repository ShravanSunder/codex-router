//! Submission outcomes retain actual native acceptance and exact turn identity, never inferred success.
use crate::{AutomationStore, StorageError};
use agent_automation::{
    PeerWriteEffect, ProviderSettlementEffect, RouteEffectEvidence, RunId, SubmissionEffect,
    WorkerOutcome,
};
use chrono::{DateTime, Utc};
use collaboration_protocol::{
    DeliveryOutcome, DeliveryReceipt, PushDeliveryState, PushId, PushKind, RouterOriginRef,
};
use serde::{Serialize, de::DeserializeOwned};
use sqlx::Connection;
#[derive(Clone)]
pub enum RunSubmissionOutcome<TReceipt> {
    Accepted {
        turn_id: String,
        receipt: TReceipt,
    },
    ProviderAdmitted {
        receipt: TReceipt,
    },
    PeerWritten {
        written_at_ms: i64,
        receipt: TReceipt,
    },
    Rejected {
        explanation: String,
    },
    Unknown {
        explanation: String,
    },
}
#[derive(Clone)]
pub struct RunSubmissionResult<TTarget, TGeneration, TReceipt> {
    pub run_id: RunId,
    pub effects: RouteEffectEvidence<TTarget, TGeneration>,
    pub outcome: RunSubmissionOutcome<TReceipt>,
}

#[derive(Clone)]
pub struct ScheduleRunPushUpdate {
    pub push_id: PushId,
    pub origin_router_ref: RouterOriginRef,
    pub delivery_state: PushDeliveryState,
    pub receipt: DeliveryReceipt,
    pub settled_at: DateTime<Utc>,
}

impl AutomationStore {
    pub async fn record_run_submission<
        TTarget: PartialEq + Serialize + DeserializeOwned,
        TEndpoint: DeserializeOwned,
        TGeneration: PartialEq + Serialize + DeserializeOwned,
        TReceipt: Serialize + DeserializeOwned,
    >(
        &mut self,
        request: RunSubmissionResult<TTarget, TGeneration, TReceipt>,
    ) -> Result<(), StorageError> {
        self.record_run_submission_inner::<TTarget, TEndpoint, TGeneration, TReceipt>(request, None)
            .await
    }

    pub async fn record_run_submission_with_push<
        TTarget: PartialEq + Serialize + DeserializeOwned,
        TEndpoint: DeserializeOwned,
        TGeneration: PartialEq + Serialize + DeserializeOwned,
        TReceipt: Serialize + DeserializeOwned,
    >(
        &mut self,
        request: RunSubmissionResult<TTarget, TGeneration, TReceipt>,
        push_update: ScheduleRunPushUpdate,
    ) -> Result<(), StorageError> {
        self.record_run_submission_inner::<TTarget, TEndpoint, TGeneration, TReceipt>(
            request,
            Some(push_update),
        )
        .await
    }

    async fn record_run_submission_inner<
        TTarget: PartialEq + Serialize + DeserializeOwned,
        TEndpoint: DeserializeOwned,
        TGeneration: PartialEq + Serialize + DeserializeOwned,
        TReceipt: Serialize + DeserializeOwned,
    >(
        &mut self,
        request: RunSubmissionResult<TTarget, TGeneration, TReceipt>,
        push_update: Option<ScheduleRunPushUpdate>,
    ) -> Result<(), StorageError> {
        let mut transaction = self.connection.begin_with("BEGIN IMMEDIATE").await?;
        let mut record =
            crate::run_inspection::read_current::<TTarget, TEndpoint, TGeneration, TReceipt>(
                &mut transaction,
                &request.run_id,
            )
            .await?;
        let recording_no_execution_outcome = push_update.is_some()
            && matches!(
                &request.outcome,
                RunSubmissionOutcome::Unknown { .. } | RunSubmissionOutcome::Rejected { .. }
            );
        if record.phase != agent_automation::RunPhase::Preparing
            || (record.evidence.timing.is_none() && !recording_no_execution_outcome)
        {
            return Err(StorageError::InvalidRecord);
        }
        let selected = record
            .evidence
            .route
            .as_ref()
            .ok_or(StorageError::InvalidRecord)?;
        if !selected.same_client_identity(&request.effects) {
            return Err(StorageError::InvalidRecord);
        }
        let push_state = push_update.as_ref().map(|update| update.delivery_state);
        if let Some(push_state) = push_state {
            let route_reports_acceptance = route_reports_acceptance(&request.effects);
            let outcome_matches_push_state = match (&request.outcome, push_state) {
                (
                    RunSubmissionOutcome::Accepted { .. }
                    | RunSubmissionOutcome::ProviderAdmitted { .. }
                    | RunSubmissionOutcome::PeerWritten { .. },
                    PushDeliveryState::Delivered,
                ) => true,
                (RunSubmissionOutcome::Unknown { .. }, PushDeliveryState::OutcomeUnknown) => true,
                (RunSubmissionOutcome::Unknown { .. }, PushDeliveryState::Delivered) => {
                    route_reports_acceptance
                }
                (RunSubmissionOutcome::Rejected { .. }, PushDeliveryState::Pending) => true,
                _ => false,
            };
            if !outcome_matches_push_state {
                return Err(StorageError::InvalidRecord);
            }
        }
        let (phase, turn_id, outcome, completed_at_ms) = match request.outcome {
            RunSubmissionOutcome::Accepted { turn_id, receipt } => {
                let (
                    RouteEffectEvidence::CodexAppServer(recorded_native),
                    RouteEffectEvidence::CodexAppServer(reported_native),
                ) = (selected, &request.effects)
                else {
                    return Err(StorageError::InvalidRecord);
                };
                if turn_id.is_empty()
                    || reported_native.submission != SubmissionEffect::Accepted
                    || reported_native.native_turn_id.as_deref() != Some(turn_id.as_str())
                    || serde_json::to_value(&reported_native.target)
                        .map_err(|_| StorageError::InvalidRecord)?
                        != serde_json::to_value(&recorded_native.target)
                            .map_err(|_| StorageError::InvalidRecord)?
                    || serde_json::to_value(&reported_native.generation)
                        .map_err(|_| StorageError::InvalidRecord)?
                        != serde_json::to_value(&recorded_native.generation)
                            .map_err(|_| StorageError::InvalidRecord)?
                    || reported_native.client_user_message_id
                        != recorded_native.client_user_message_id
                {
                    return Err(StorageError::InvalidRecord);
                }
                record.evidence.acceptance = Some(receipt);
                ("executing", Some(turn_id), None, None)
            }
            RunSubmissionOutcome::ProviderAdmitted { receipt } => {
                let (
                    RouteEffectEvidence::ProviderAcp(before),
                    RouteEffectEvidence::ProviderAcp(after),
                ) = (selected, &request.effects)
                else {
                    return Err(StorageError::InvalidRecord);
                };
                if before.target != after.target
                    || before.generation != after.generation
                    || before.binding != after.binding
                    || before.attempt_id != after.attempt_id
                    || after.submission != SubmissionEffect::Accepted
                    || after.settlement != ProviderSettlementEffect::NotObserved
                {
                    return Err(StorageError::InvalidRecord);
                }
                record.evidence.acceptance = Some(receipt);
                ("executing", None, None, None)
            }
            RunSubmissionOutcome::PeerWritten {
                written_at_ms,
                receipt,
            } => {
                let (
                    RouteEffectEvidence::ClaudeCodePeer(before),
                    RouteEffectEvidence::ClaudeCodePeer(after),
                ) = (selected, &request.effects)
                else {
                    return Err(StorageError::InvalidRecord);
                };
                let dispatch_started_at_ms = record
                    .evidence
                    .timing
                    .as_ref()
                    .ok_or(StorageError::InvalidRecord)?
                    .dispatch_started_at_ms;
                if written_at_ms < dispatch_started_at_ms
                    || before.session_id != after.session_id
                    || before.process_id != after.process_id
                    || after.write != PeerWriteEffect::Written
                {
                    return Err(StorageError::InvalidRecord);
                }
                record.evidence.acceptance = Some(receipt);
                (
                    "finished",
                    None,
                    Some(WorkerOutcome::PeerMessageWritten {
                        explanation: "Peer message written; receiver completion was not observed."
                            .into(),
                    }),
                    Some(written_at_ms),
                )
            }
            RunSubmissionOutcome::Rejected { explanation: _ } => {
                let known_none = match &request.effects {
                    RouteEffectEvidence::CodexAppServer(native) => matches!(
                        native.submission,
                        SubmissionEffect::Rejected | SubmissionEffect::NotDispatched
                    ),
                    RouteEffectEvidence::ProviderAcp(provider) => matches!(
                        provider.submission,
                        SubmissionEffect::Rejected | SubmissionEffect::NotDispatched
                    ),
                    RouteEffectEvidence::ClaudeCodePeer(peer) => {
                        peer.write == PeerWriteEffect::NotDispatched
                    }
                };
                if !known_none {
                    return Err(StorageError::InvalidRecord);
                }
                record.evidence.timing = None;
                record.evidence.acceptance = None;
                ("preparing", None, None, None)
            }
            RunSubmissionOutcome::Unknown { explanation: _ } => {
                let turn_id = match &request.effects {
                    RouteEffectEvidence::CodexAppServer(native) => native.native_turn_id.clone(),
                    RouteEffectEvidence::ProviderAcp(_)
                    | RouteEffectEvidence::ClaudeCodePeer(_) => None,
                };
                ("uncertain", turn_id, None, None)
            }
        };
        record.evidence.route = Some(request.effects);
        let evidence =
            serde_json::to_string(&record.evidence).map_err(|_| StorageError::InvalidRecord)?;
        let timing = record.evidence.timing.as_ref();
        if let Some(push_update) = push_update {
            let push_state = push_state.ok_or(StorageError::InvalidRecord)?;
            let expected_origin_reference = RouterOriginRef::ScheduleRun {
                schedule_id: record.schedule_id.clone(),
                run_id: record.run_id.clone(),
            };
            if push_update.origin_router_ref != expected_origin_reference {
                return Err(StorageError::InvalidRecord);
            }
            let expected_origin_ref = expected_origin_reference
                .canonical_string()
                .map_err(|_| StorageError::InvalidRecord)?;
            let push_row = sqlx::query_as!(
                crate::push_record_rows::PushRecordRow,
                "SELECT push_id,kind,origin_kind,origin_service_id,origin_endpoint_id,origin_session_id,origin_router_ref,target_service_id,target_endpoint_id,target_session_id,reply_to_push_id,header_facts_json,body,ranges_json,delivery_state,last_outcome_json,created_at,settled_at,read_at FROM router_pushes WHERE push_id=?",
                push_update.push_id.as_str()
            )
            .fetch_optional(&mut *transaction)
            .await?
            .ok_or(StorageError::PushNotFound)?;
            let push = push_row.into_record()?;
            if push.kind != PushKind::ScheduleRun
                || push.origin_router_ref.as_deref() != Some(expected_origin_ref.as_str())
                || push.delivery_state != PushDeliveryState::Attempted
            {
                return Err(StorageError::InvalidRecord);
            }
            let receipt_matches_state = match push_state {
                PushDeliveryState::Delivered => matches!(
                    push_update.receipt.outcome,
                    DeliveryOutcome::Started
                        | DeliveryOutcome::Steered
                        | DeliveryOutcome::StartedOrSteered
                        | DeliveryOutcome::Queued
                        | DeliveryOutcome::PeerMessageWritten
                ),
                PushDeliveryState::OutcomeUnknown => {
                    push_update.receipt.outcome == DeliveryOutcome::Unknown
                }
                PushDeliveryState::Pending => {
                    matches!(push_update.receipt.outcome, DeliveryOutcome::Rejected(_))
                }
                PushDeliveryState::Attempted
                | PushDeliveryState::Held
                | PushDeliveryState::Rejected => false,
            };
            if !receipt_matches_state {
                return Err(StorageError::InvalidRecord);
            }
            let outcome_json = serde_json::to_string(&push_update.receipt)
                .map_err(|_| StorageError::InvalidRecord)?;
            let settled_at = push_update
                .settled_at
                .to_rfc3339_opts(chrono::SecondsFormat::Nanos, true);
            let result = sqlx::query!(
                "UPDATE router_pushes SET delivery_state=?,last_outcome_json=?,settled_at=? WHERE push_id=? AND origin_kind='router' AND origin_router_ref=? AND delivery_state='attempted'",
                crate::push_record_rows::serialize_delivery_state(push_state),
                outcome_json,
                settled_at,
                push_update.push_id.as_str(),
                expected_origin_ref
            )
            .execute(&mut *transaction)
            .await?;
            if result.rows_affected() != 1 {
                return Err(StorageError::InvalidRecord);
            }
        }
        sqlx::query("UPDATE workflow_runs SET run_status=?,native_turn_id=?,execution_evidence_json=?,worker_outcome_json=?,execution_started_at_ms=?,execution_deadline_at_ms=?,effective_timeout_seconds=?,completed_at_ms=? WHERE run_id=? AND run_status='preparing'")
            .bind(phase).bind(turn_id).bind(evidence).bind(outcome.map(|outcome|serde_json::to_string(&outcome)).transpose().map_err(|_|StorageError::InvalidRecord)?)
            .bind(timing.map(|value| value.dispatch_started_at_ms))
            .bind(timing.map(|value| value.deadline_at_ms))
            .bind(timing.map(|value| i64::from(value.effective_timeout_seconds)))
            .bind(completed_at_ms)
            .bind(request.run_id.as_str()).execute(&mut *transaction).await?;
        transaction.commit().await?;
        Ok(())
    }
}

fn route_reports_acceptance<TTarget, TGeneration>(
    effects: &RouteEffectEvidence<TTarget, TGeneration>,
) -> bool {
    match effects {
        RouteEffectEvidence::CodexAppServer(native) => {
            native.submission == SubmissionEffect::Accepted
        }
        RouteEffectEvidence::ProviderAcp(provider) => {
            provider.submission == SubmissionEffect::Accepted
        }
        RouteEffectEvidence::ClaudeCodePeer(peer) => peer.write == PeerWriteEffect::Written,
    }
}

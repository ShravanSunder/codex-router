//! Store-first subscription records and their prepared Layer-0 delivery.
use super::SubscriptionClock;
use crate::{
    AttemptEvidenceSink, AttemptReconciliation, AttemptReconciliationContext, DeliveryFuture,
    DeliveryPrecondition, LoadPolicy, MachineIdentity, SessionMessageDelivery,
    layer_zero::{DeliveryRequest, PreparedPush},
};
use agent_automation::{AttemptId, RouteEffectEvidence};
use automation_storage::AutomationStore;
use collaboration_protocol::{
    CodexGeneration, DeliveryCorrelationId, DeliveryOutcome, DeliveryReceipt, MachineId,
    MessageContent, MessageDelivery, MessageText, PushActivityRange, PushActivitySnapshot,
    PushHeaderFacts, PushId, PushKind, PushLineInput, PushOrigin, PushRecordDraft, RouterLink,
    RouterOriginRef, SessionReachability, SessionRef, render_push_line,
};
use message_board::{
    BoardError, Identity, SubscriptionBatch, SubscriptionGeneration, SubscriptionScope,
};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

pub(super) struct SubscriptionPushStore {
    pub(super) store: Arc<Mutex<AutomationStore>>,
    pub(super) delivery: Arc<dyn SessionMessageDelivery>,
    pub(super) machine: MachineIdentity,
    pub(super) clock: Arc<dyn SubscriptionClock>,
    pub(super) shutdown: CancellationToken,
    #[cfg(test)]
    observations: tokio::sync::broadcast::Sender<super::subscription_service::OwnerObservation>,
}

pub(super) struct SubscriptionPushStoreProps {
    pub store: Arc<Mutex<AutomationStore>>,
    pub delivery: Arc<dyn SessionMessageDelivery>,
    pub machine: MachineIdentity,
    pub clock: Arc<dyn SubscriptionClock>,
    pub shutdown: CancellationToken,
    #[cfg(test)]
    pub observations: tokio::sync::broadcast::Sender<super::subscription_service::OwnerObservation>,
}

pub(super) struct SubscriptionPushReceipt {
    pub receipt: DeliveryReceipt,
    pub evidence: serde_json::Value,
}

impl SubscriptionPushStore {
    pub(super) fn new(props: SubscriptionPushStoreProps) -> Self {
        let SubscriptionPushStoreProps {
            store,
            delivery,
            machine,
            clock,
            shutdown,
            #[cfg(test)]
            observations,
        } = props;
        Self {
            store,
            delivery,
            machine,
            clock,
            shutdown,
            #[cfg(test)]
            observations,
        }
    }

    pub(super) async fn activity(
        &self,
        target: &SessionRef,
        batch: &SubscriptionBatch,
        load_policy: LoadPolicy,
    ) -> Result<PreparedPush, BoardError> {
        let root_count =
            u8::try_from(batch.roots.len()).map_err(|_| BoardError::board_unavailable())?;
        let message_count = batch
            .roots
            .iter()
            .try_fold(0_u64, |count, root| count.checked_add(root.message_count))
            .ok_or_else(BoardError::board_unavailable)?;
        let header_facts = PushHeaderFacts::SubscriptionActivity {
            root_count,
            message_count,
            held_since: batch.held_since.map(|held| held.to_rfc3339()),
            thread_resolved: batch.draining,
        };
        let activity = PushActivitySnapshot {
            ranges: batch
                .roots
                .iter()
                .map(|root| PushActivityRange {
                    root_message_id: root.root_id.clone(),
                    from_activity_sequence: root.from_sequence,
                    through_activity_sequence: root.through_sequence,
                })
                .collect(),
            held: batch.held,
            draining: batch.draining,
        };
        let origin_router_ref = RouterOriginRef::SubscriptionActivity {
            target: target.clone(),
            batch_id: batch.batch_id.clone(),
        };
        let draft = self.draft(
            target,
            PushKind::SubscriptionActivity,
            origin_router_ref,
            header_facts,
            None,
            Some(activity),
        )?;
        self.store_prepared(draft, load_policy).await
    }

    pub(super) async fn expiry(
        &self,
        target: &SessionRef,
        scope: &SubscriptionScope,
        subscription_generation: SubscriptionGeneration,
    ) -> Result<PreparedPush, BoardError> {
        let (kind, id) = scope.kind_and_id();
        let argument = match scope {
            SubscriptionScope::Thread { .. } => "--root-message-id",
            SubscriptionScope::Topic { .. } => "--topic-id",
        };
        let draft = self.draft(
            target,
            PushKind::SubscriptionExpiry,
            RouterOriginRef::SubscriptionExpiry {
                target: target.clone(),
                scope: scope.clone(),
                subscription_generation,
            },
            PushHeaderFacts::SubscriptionExpiry {
                scope: format!("{kind} {id}"),
            },
            Some(format!(
                "Renew with: agent-collaboration board thread subscribe {argument} {id}"
            )),
            None,
        )?;
        self.store_prepared(draft, LoadPolicy::LoadedOnly).await
    }

    fn draft(
        &self,
        target: &SessionRef,
        kind: PushKind,
        origin_router_ref: RouterOriginRef,
        header_facts: PushHeaderFacts,
        body: Option<String>,
        activity: Option<PushActivitySnapshot>,
    ) -> Result<PushRecordDraft, BoardError> {
        Ok(PushRecordDraft {
            push_id: PushId::try_from(uuid::Uuid::now_v7().to_string())
                .map_err(|_| BoardError::board_unavailable())?,
            kind,
            origin: PushOrigin::Router(kind),
            origin_router_ref: Some(
                origin_router_ref
                    .canonical_string()
                    .map_err(|_| BoardError::board_unavailable())?,
            ),
            target: target.clone(),
            mode: None,
            guard: None,
            reply_to_push_id: None,
            header_facts,
            body,
            activity,
            created_at: self.clock.now(),
        })
    }

    async fn store_prepared(
        &self,
        draft: PushRecordDraft,
        load_policy: LoadPolicy,
    ) -> Result<PreparedPush, BoardError> {
        let record = draft
            .clone()
            .into_pending()
            .map_err(|_| BoardError::board_unavailable())?;
        let line = render_push_line(&PushLineInput {
            link: RouterLink::new(
                MachineId::from(self.machine.service_id().clone()),
                record.push_id.clone(),
            ),
            machine_label: self.machine.machine_label().clone(),
            origin: record.origin,
            header_facts: record.header_facts,
            // Expiry stores the renew command for show; subscription lines never preview it.
            body: None,
        })
        .map_err(|_| BoardError::board_unavailable())?;
        let line = MessageText::try_from(line).map_err(|_| BoardError::board_unavailable())?;
        self.store
            .lock()
            .await
            .insert_push_record(draft)
            .await
            .map_err(|_| BoardError::board_unavailable())?;
        Ok(PreparedPush {
            push_id: record.push_id,
            line,
            load_policy,
        })
    }

    pub(super) async fn mark_attempted(&self, prepared: &PreparedPush) -> Result<(), BoardError> {
        self.store
            .lock()
            .await
            .mark_push_attempted(&prepared.push_id)
            .await
            .map_err(|_| BoardError::board_unavailable())?;
        Ok(())
    }

    pub(super) async fn deliver(
        &self,
        target: &SessionRef,
        prepared: PreparedPush,
    ) -> Result<SubscriptionPushReceipt, BoardError> {
        self.mark_attempted(&prepared).await?;
        let correlation = DeliveryCorrelationId::try_from(prepared.push_id.as_str().to_owned())
            .map_err(|_| BoardError::board_unavailable())?;
        let request = DeliveryRequest {
            payload: prepared.clone(),
            target: target.clone(),
            mode: MessageDelivery::Auto,
            precondition: DeliveryPrecondition::Unpinned,
            correlation,
            attempt: AttemptId::generate(),
        };
        let sink = CapturedAttemptEvidence::default();
        let mut receipt = tokio::select! {
            () = self.shutdown.cancelled() => return Err(BoardError::board_unavailable()),
            receipt = self.delivery.deliver_prepared(request, &sink) => receipt.unwrap_or_else(|_| unknown_receipt()),
        };
        if prepared.load_policy == LoadPolicy::LoadedOnly
            && receipt.reachability == Some(SessionReachability::ProviderAcp)
            && receipt.outcome == DeliveryOutcome::Queued
        {
            receipt = self.reconcile_queued(target, &prepared, &sink).await;
        }
        self.record_receipt(&prepared, &receipt, true).await?;
        let evidence = serde_json::json!({ "receipt": receipt, "routeEvidence": sink.recorded.lock().await.clone() });
        Ok(SubscriptionPushReceipt { receipt, evidence })
    }

    async fn reconcile_queued(
        &self,
        target: &SessionRef,
        prepared: &PreparedPush,
        sink: &CapturedAttemptEvidence,
    ) -> DeliveryReceipt {
        let Some(recorded) = sink.recorded.lock().await.clone() else {
            return unknown_receipt();
        };
        let context = AttemptReconciliationContext {
            target: target.clone(),
            message: MessageContent::Router {
                text: prepared.line.clone(),
            },
            prepared_push_id: Some(prepared.push_id.clone()),
            mode: MessageDelivery::Auto,
            recorded,
        };
        match self.delivery.reconcile_attempt(context).await {
            Ok(AttemptReconciliation::Accepted(receipt)) => *receipt,
            Ok(AttemptReconciliation::KnownNotSubmitted) => DeliveryReceipt {
                outcome: DeliveryOutcome::NotSubmitted {
                    retryable: true,
                    reason: "queued input was not submitted".to_owned(),
                },
                reachability: Some(SessionReachability::ProviderAcp),
                client: None,
            },
            Ok(AttemptReconciliation::StillUnknown) | Err(_) => unknown_receipt(),
        }
    }

    /// After an effect, retry only its evidence write. The owning actor stays serialized.
    pub(super) async fn record_receipt(
        &self,
        prepared: &PreparedPush,
        receipt: &DeliveryReceipt,
        allow_hold: bool,
    ) -> Result<(), BoardError> {
        let mut delay = std::time::Duration::from_secs(1);
        loop {
            let result = {
                let mut store = self.store.lock().await;
                if allow_hold
                    && matches!(
                        receipt.outcome,
                        DeliveryOutcome::NotSubmitted {
                            retryable: true,
                            ..
                        }
                    )
                {
                    store
                        .hold_push_record(&prepared.push_id, receipt.clone())
                        .await
                } else {
                    store
                        .settle_push_record(&prepared.push_id, receipt.clone(), self.clock.now())
                        .await
                }
            };
            if result.is_ok() {
                return Ok(());
            }
            #[cfg(test)]
            let _ = self
                .observations
                .send(super::subscription_service::OwnerObservation::PushSettlementRetry);
            tracing::warn!(
                push_id = prepared.push_id.as_str(),
                "retrying stored push settlement without redelivery"
            );
            let deadline = self
                .clock
                .monotonic_now()
                .checked_add(delay)
                .ok_or_else(BoardError::board_unavailable)?;
            tokio::select! { () = self.shutdown.cancelled() => return Err(BoardError::board_unavailable()), () = self.clock.sleep_until(deadline) => {} }
            delay = delay
                .saturating_mul(2)
                .min(std::time::Duration::from_secs(30));
        }
    }
}

#[derive(Default)]
struct CapturedAttemptEvidence {
    recorded: Mutex<Option<RouteEffectEvidence<SessionRef, CodexGeneration>>>,
}
impl AttemptEvidenceSink for CapturedAttemptEvidence {
    fn record(
        &self,
        evidence: RouteEffectEvidence<SessionRef, CodexGeneration>,
    ) -> DeliveryFuture<'_, ()> {
        Box::pin(async move {
            *self.recorded.lock().await = Some(evidence);
            Ok(())
        })
    }
}

fn unknown_receipt() -> DeliveryReceipt {
    DeliveryReceipt {
        outcome: DeliveryOutcome::Unknown,
        reachability: None,
        client: None,
    }
}

pub(super) fn target_session(reader: &Identity) -> Result<SessionRef, BoardError> {
    let Identity::Session { session } = reader else {
        return Err(BoardError::invalid_field(
            "reader",
            "deliver-mode subscription requires a session Reader",
        ));
    };
    serde_json::from_value(
        serde_json::to_value(session).map_err(|_| BoardError::board_unavailable())?,
    )
    .map_err(|_| BoardError::invalid_field("reader", "must contain a valid SessionRef"))
}

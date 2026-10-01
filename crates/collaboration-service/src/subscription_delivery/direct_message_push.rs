//! Durable direct-message work carried by the same target actor as subscription notices.
use super::subscription_push::SubscriptionPushStore;
use crate::{
    DeliveryPrecondition, LoadPolicy,
    layer_zero::{DeliveryRequest, PreparedPush},
    session_delivery_contract::UnstoredAttemptEvidenceSink,
};
use collaboration_protocol::{
    AttemptId, DeliveryCorrelationId, DeliveryOutcome, DeliveryReceipt, MachineId, MessageDelivery,
    MessageText, PushId, PushKind, PushLineInput, PushRecord, RouterLink, SessionRef,
    render_push_line,
};
use message_board::BoardError;

impl SubscriptionPushStore {
    pub(super) async fn restore_direct_messages(&self) -> Result<Vec<SessionRef>, BoardError> {
        let mut store = self.store.lock().await;
        store
            .settle_interrupted_direct_messages(self.clock.now())
            .await
            .map_err(|_| BoardError::board_unavailable())?;
        store
            .direct_message_recovery_targets()
            .await
            .map_err(|_| BoardError::board_unavailable())
    }

    pub(super) async fn direct_messages(
        &self,
        target: &SessionRef,
    ) -> Result<Vec<PushRecord>, BoardError> {
        self.store
            .lock()
            .await
            .list_unsent_direct_messages(target)
            .await
            .map_err(|_| BoardError::board_unavailable())
    }

    pub(super) async fn direct_message(&self, push_id: &PushId) -> Result<PushRecord, BoardError> {
        self.store
            .lock()
            .await
            .get_push_record(push_id)
            .await
            .map_err(|_| BoardError::board_unavailable())?
            .ok_or_else(BoardError::board_unavailable)
    }

    pub(super) async fn hold_direct_message(
        &self,
        record: &PushRecord,
    ) -> Result<PushRecord, BoardError> {
        self.store
            .lock()
            .await
            .hold_unsent_direct_message(
                &record.push_id,
                DeliveryReceipt {
                    outcome: DeliveryOutcome::NotSubmitted {
                        retryable: true,
                        reason: "target is not running; direct message held".to_owned(),
                    },
                    reachability: None,
                    client: None,
                },
            )
            .await
            .map_err(|_| BoardError::board_unavailable())
    }

    pub(super) async fn reject_direct_message(
        &self,
        record: &PushRecord,
    ) -> Result<PushRecord, BoardError> {
        let (reason, detail) = if record.guard.is_some() {
            (
                collaboration_protocol::DeliveryRejectionReason::StaleGeneration,
                "guarded direct message requires a running target generation",
            )
        } else {
            (
                collaboration_protocol::DeliveryRejectionReason::SteerUnsupported,
                "steer direct message requires a running target",
            )
        };
        let receipt = DeliveryReceipt {
            outcome: DeliveryOutcome::Rejected(collaboration_protocol::DeliveryRejection {
                reason,
                next_action: collaboration_protocol::DeliveryNextAction::InspectTarget,
                client_code: None,
                detail: Some(detail.to_owned()),
                claims: None,
            }),
            reachability: None,
            client: None,
        };
        self.store
            .lock()
            .await
            .reject_unsent_direct_message(&record.push_id, receipt, self.clock.now())
            .await
            .map_err(|_| BoardError::board_unavailable())
    }

    pub(super) async fn deliver_direct_message(
        &self,
        record: &PushRecord,
    ) -> Result<PushRecord, BoardError> {
        if record.kind != PushKind::DirectMessage {
            return Err(BoardError::board_unavailable());
        }
        let mode = record.mode.ok_or_else(BoardError::board_unavailable)?;
        let line = render_push_line(&PushLineInput {
            link: RouterLink::new(
                MachineId::from(self.machine.service_id().clone()),
                record.push_id.clone(),
            ),
            machine_label: self.machine.machine_label().clone(),
            origin: record.origin.clone(),
            header_facts: record.header_facts.clone(),
            body: record.body.clone(),
        })
        .map_err(|_| BoardError::board_unavailable())?;
        let prepared = PreparedPush {
            push_id: record.push_id.clone(),
            line: MessageText::try_from(line).map_err(|_| BoardError::board_unavailable())?,
            load_policy: LoadPolicy::LoadedOnly,
        };
        let correlation = DeliveryCorrelationId::try_from(record.push_id.as_str().to_owned())
            .map_err(|_| BoardError::board_unavailable())?;
        let request = DeliveryRequest {
            payload: prepared.clone(),
            target: record.target.clone(),
            mode,
            precondition: record
                .guard
                .clone()
                .map_or(DeliveryPrecondition::Unpinned, |expected| {
                    DeliveryPrecondition::EndpointGeneration { expected }
                }),
            correlation,
            attempt: AttemptId::generate(),
        };
        self.mark_attempted(&prepared).await?;
        let receipt = tokio::select! {
            () = self.shutdown.cancelled() => return Err(BoardError::board_unavailable()),
            receipt = self.delivery.deliver(request, &UnstoredAttemptEvidenceSink) => receipt.unwrap_or(DeliveryReceipt { outcome: DeliveryOutcome::Unknown, reachability: None, client: None }),
        };
        let allow_hold = mode != MessageDelivery::Steer && record.guard.is_none();
        self.record_receipt(&prepared, &receipt, allow_hold).await?;
        self.direct_message(&record.push_id).await
    }
}

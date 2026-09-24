//! Background Thread Listen delivery through the injected session delivery seam.
use crate::{
    DeliveryPrecondition, DeliveryRequest, SessionMessageDelivery,
    session_delivery_contract::UnstoredAttemptEvidenceSink,
};
use collaboration_protocol::{DeliveryOutcome, MessageContent, MessageDelivery, SessionRef};
use message_board::{BatchSink, BatchSinkFailure, ListenDeliveryRecord, ThreadListenBatchSet};
use std::sync::Arc;

#[derive(Clone)]
pub(crate) struct SessionDeliverySink {
    pub(crate) delivery: Arc<dyn SessionMessageDelivery>,
    pub(crate) target: SessionRef,
}

impl BatchSink for SessionDeliverySink {
    fn deliver<'a>(
        &'a self,
        record: ListenDeliveryRecord,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<(), BatchSinkFailure>> + Send + 'a>,
    > {
        Box::pin(async move { self.dispatch(record).await })
    }
}

impl SessionDeliverySink {
    async fn dispatch(&self, record: ListenDeliveryRecord) -> Result<(), BatchSinkFailure> {
        let text = render_record(&record).map_err(|_| BatchSinkFailure::Unavailable)?;
        // A delivery is Router's own record; the target session did not send it.
        let message = MessageContent::Router {
            text: text.try_into().map_err(|_| BatchSinkFailure::Unavailable)?,
        };
        let request = DeliveryRequest {
            target: self.target.clone(),
            message,
            mode: MessageDelivery::Auto,
            precondition: DeliveryPrecondition::Unpinned,
            correlation: collaboration_protocol::DeliveryCorrelationId::generate(),
            attempt: agent_automation::AttemptId::generate(),
        };
        let receipt = self
            .delivery
            .deliver(request, &UnstoredAttemptEvidenceSink)
            .await
            .map_err(|_| BatchSinkFailure::Unavailable)?;
        match &receipt.outcome {
            DeliveryOutcome::Started
            | DeliveryOutcome::Steered
            | DeliveryOutcome::StartedOrSteered
            | DeliveryOutcome::Queued
            | DeliveryOutcome::PeerMessageWritten
            | DeliveryOutcome::Unknown => Ok(()),
            DeliveryOutcome::Rejected(_)
            | DeliveryOutcome::NotSubmitted {
                retryable: false, ..
            } => Err(BatchSinkFailure::Rejected {
                evidence: serde_json::to_value(receipt)
                    .map_err(|_| BatchSinkFailure::Unavailable)?,
            }),
            DeliveryOutcome::NotSubmitted {
                retryable: true, ..
            } => Err(BatchSinkFailure::Unavailable),
        }
    }
}

fn render_record(record: &ListenDeliveryRecord) -> Result<String, serde_json::Error> {
    match record {
        ListenDeliveryRecord::Batch(batch_set) => {
            let summary = batch_summary(batch_set);
            Ok(format!("{summary}\n{}", serde_json::to_string(batch_set)?))
        }
        ListenDeliveryRecord::Heartbeat(heartbeat) => serde_json::to_string(heartbeat),
        ListenDeliveryRecord::Finalization(finalization) => serde_json::to_string(finalization),
    }
}

fn batch_summary(batch_set: &ThreadListenBatchSet) -> String {
    let count = batch_set
        .batches
        .iter()
        .map(|batch| batch.messages.len())
        .sum::<usize>();
    let first = batch_set
        .batches
        .iter()
        .flat_map(|batch| batch.messages.iter())
        .map(|message| message.activity_sequence.get())
        .min()
        .unwrap_or(0);
    let last = batch_set
        .batches
        .iter()
        .map(|batch| batch.delivered_through.get())
        .max()
        .unwrap_or(0);
    let threads = batch_set
        .batches
        .iter()
        .map(|batch| batch.root_message_id.as_str())
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "Thread activity: {threads}; count {count}; sequences {first}-{last}; catchUp: {}.",
        batch_set.catch_up
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AttemptEvidenceSink, AttemptReconciliation, AttemptReconciliationContext, DeliveryFuture,
        DeliveryReceipt,
    };
    use collaboration_protocol::{
        DeliveryNextAction, DeliveryRejection, DeliveryRejectionReason, SessionReachability,
    };
    use message_board::{ListenId, ThreadListenHeartbeat, ThreadListenHeartbeatKind};

    struct FakeDelivery(DeliveryOutcome);

    impl SessionMessageDelivery for FakeDelivery {
        fn deliver<'a>(
            &'a self,
            _: DeliveryRequest,
            _: &'a dyn AttemptEvidenceSink,
        ) -> DeliveryFuture<'a, DeliveryReceipt> {
            Box::pin(async move {
                Ok(DeliveryReceipt {
                    outcome: self.0.clone(),
                    reachability: Some(SessionReachability::ProviderAcp),
                    client: None,
                })
            })
        }

        fn reconcile_attempt(
            &self,
            _: AttemptReconciliationContext,
        ) -> DeliveryFuture<'_, AttemptReconciliation> {
            Box::pin(async { Ok(AttemptReconciliation::StillUnknown) })
        }
    }

    async fn push(outcome: DeliveryOutcome) -> Result<(), BatchSinkFailure> {
        let target: SessionRef = serde_json::from_value(serde_json::json!({
            "endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"fixture-provider"},
            "sessionId":"reader"
        })).expect("fixture session");
        let sink = SessionDeliverySink {
            delivery: Arc::new(FakeDelivery(outcome)),
            target,
        };
        sink.deliver(ListenDeliveryRecord::Heartbeat(ThreadListenHeartbeat {
            kind: ThreadListenHeartbeatKind::ListenHeartbeat,
            listen_id: ListenId::generate(),
            last_sequence: None,
            mark: 1,
            text: "heartbeat".into(),
        }))
        .await
    }

    #[tokio::test]
    async fn listen_push_uses_client_neutral_outcomes() {
        assert!(push(DeliveryOutcome::Started).await.is_ok());
        assert!(push(DeliveryOutcome::Unknown).await.is_ok());
        assert_eq!(
            push(DeliveryOutcome::NotSubmitted {
                retryable: true,
                reason: "starting".into()
            })
            .await,
            Err(BatchSinkFailure::Unavailable),
        );
        let rejected = push(DeliveryOutcome::Rejected(DeliveryRejection {
            reason: DeliveryRejectionReason::Busy,
            next_action: DeliveryNextAction::InspectTarget,
            client_code: Some(-32000),
            detail: None,
        }))
        .await;
        let Err(BatchSinkFailure::Rejected { evidence }) = rejected else {
            panic!("rejected push did not expose its receipt");
        };
        assert_eq!(
            evidence
                .pointer("/outcome/reason")
                .and_then(serde_json::Value::as_str),
            Some("busy")
        );
        assert_eq!(
            evidence
                .pointer("/outcome/clientCode")
                .and_then(serde_json::Value::as_i64),
            Some(-32000)
        );
    }
}

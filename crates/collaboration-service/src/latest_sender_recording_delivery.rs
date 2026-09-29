//! Persists reply addresses after accepted direct Agent deliveries.
use crate::{
    AttemptEvidenceSink, AttemptReconciliation, AttemptReconciliationContext, DeliveryFuture,
    DeliveryReceipt, DeliveryRequest, SessionMessageDelivery,
};
use automation_storage::{AutomationStore, LatestAgentSenderRecord};
use collaboration_protocol::{DeliveryOutcome, MessageContent};
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::Mutex;

pub(crate) struct LatestSenderRecordingDelivery {
    inner: Arc<dyn SessionMessageDelivery>,
    store: Option<Arc<Mutex<AutomationStore>>>,
    latest_sender_unknown: Arc<Mutex<HashSet<collaboration_protocol::SessionRef>>>,
}

impl LatestSenderRecordingDelivery {
    pub(crate) fn new(
        inner: Arc<dyn SessionMessageDelivery>,
        store: Option<Arc<Mutex<AutomationStore>>>,
        latest_sender_unknown: Arc<Mutex<HashSet<collaboration_protocol::SessionRef>>>,
    ) -> Self {
        Self {
            inner,
            store,
            latest_sender_unknown,
        }
    }
}

impl SessionMessageDelivery for LatestSenderRecordingDelivery {
    fn deliver<'a>(
        &'a self,
        request: DeliveryRequest,
        evidence: &'a dyn AttemptEvidenceSink,
    ) -> DeliveryFuture<'a, DeliveryReceipt> {
        Box::pin(async move {
            let latest_sender = match &request.message {
                MessageContent::Agent { sender, .. } => {
                    Some((request.target.clone(), sender.clone()))
                }
                MessageContent::HumanUser { .. } | MessageContent::Router { .. } => None,
            };
            let receipt = self.inner.deliver(request, evidence).await?;
            if let Some((recipient, sender)) = latest_sender
                && is_accepted(&receipt.outcome)
            {
                self.latest_sender_unknown
                    .lock()
                    .await
                    .insert(recipient.clone());
                let Some(store) = self.store.as_ref() else {
                    tracing::warn!(
                        "latest Agent sender could not be recorded because the Router automation store is not running"
                    );
                    return Ok(receipt);
                };

                let mut store = store.lock().await;
                let record =
                    LatestAgentSenderRecord::new(recipient.clone(), sender, chrono::Utc::now());
                let write_result = match record {
                    Ok(record) => store.store_latest_agent_sender(&record).await,
                    Err(error) => Err(error),
                };
                match write_result {
                    Ok(()) => {
                        self.latest_sender_unknown.lock().await.remove(&recipient);
                    }
                    Err(write_error) => {
                        tracing::warn!(
                            error = %write_error,
                            "could not record latest Agent sender; attempting to invalidate the previous reply address"
                        );
                        match store.remove_latest_agent_sender_for(&recipient).await {
                            Ok(()) => {
                                self.latest_sender_unknown.lock().await.remove(&recipient);
                            }
                            Err(invalidation_error) => {
                                tracing::warn!(
                                    error = %invalidation_error,
                                    "could not invalidate the previous Agent reply address; replies will require an explicit target"
                                );
                            }
                        }
                    }
                }
            }
            Ok(receipt)
        })
    }

    fn reconcile_attempt(
        &self,
        context: AttemptReconciliationContext,
    ) -> DeliveryFuture<'_, AttemptReconciliation> {
        self.inner.reconcile_attempt(context)
    }
}

fn is_accepted(outcome: &DeliveryOutcome) -> bool {
    matches!(
        outcome,
        DeliveryOutcome::Started
            | DeliveryOutcome::Steered
            | DeliveryOutcome::StartedOrSteered
            | DeliveryOutcome::Queued
            | DeliveryOutcome::PeerMessageWritten
    )
}

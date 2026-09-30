//! Best-effort reply-address updates tied only to direct Agent message dispatch.
use automation_storage::{AutomationStore, LatestAgentSenderRecord};
use collaboration_protocol::{DeliveryOutcome, SessionRef};
use std::{collections::HashSet, sync::Arc};
use tokio::sync::Mutex;

pub(crate) async fn record_after_acceptance(
    store: Option<&Arc<Mutex<AutomationStore>>>,
    latest_sender_unknown: &Arc<Mutex<HashSet<SessionRef>>>,
    recipient: &SessionRef,
    sender: &SessionRef,
) {
    mark_unknown(latest_sender_unknown, recipient).await;
    let Some(store) = store else {
        tracing::warn!(
            recipient = ?recipient,
            "latest Agent sender could not be recorded because the Router automation store is not running"
        );
        return;
    };

    let record =
        LatestAgentSenderRecord::new(recipient.clone(), sender.clone(), chrono::Utc::now());
    let write_result = match record {
        Ok(record) => store.lock().await.store_latest_agent_sender(&record).await,
        Err(error) => Err(error),
    };
    match write_result {
        Ok(()) => clear_unknown(latest_sender_unknown, recipient).await,
        Err(error) => {
            tracing::warn!(
                recipient = ?recipient,
                error = %error,
                "could not record latest Agent sender; attempting to invalidate the previous reply address"
            );
            invalidate_after_unknown(Some(store), latest_sender_unknown, recipient).await;
        }
    }
}

pub(crate) async fn invalidate_after_unknown(
    store: Option<&Arc<Mutex<AutomationStore>>>,
    latest_sender_unknown: &Arc<Mutex<HashSet<SessionRef>>>,
    recipient: &SessionRef,
) {
    mark_unknown(latest_sender_unknown, recipient).await;
    let Some(store) = store else {
        tracing::warn!(
            recipient = ?recipient,
            "latest Agent sender is unknown because the Router automation store is not running"
        );
        return;
    };
    match store
        .lock()
        .await
        .remove_latest_agent_sender_for(recipient)
        .await
    {
        Ok(()) => clear_unknown(latest_sender_unknown, recipient).await,
        Err(error) => tracing::warn!(
            recipient = ?recipient,
            error = %error,
            "could not invalidate the previous Agent reply address; replies will require an explicit target"
        ),
    }
}

pub(crate) fn is_accepted(outcome: &DeliveryOutcome) -> bool {
    matches!(
        outcome,
        DeliveryOutcome::Started
            | DeliveryOutcome::Steered
            | DeliveryOutcome::StartedOrSteered
            | DeliveryOutcome::Queued
            | DeliveryOutcome::PeerMessageWritten
    )
}

pub(crate) fn is_unknown(outcome: &DeliveryOutcome) -> bool {
    matches!(outcome, DeliveryOutcome::Unknown)
}

async fn mark_unknown(
    latest_sender_unknown: &Arc<Mutex<HashSet<SessionRef>>>,
    recipient: &SessionRef,
) {
    latest_sender_unknown.lock().await.insert(recipient.clone());
}

async fn clear_unknown(
    latest_sender_unknown: &Arc<Mutex<HashSet<SessionRef>>>,
    recipient: &SessionRef,
) {
    latest_sender_unknown.lock().await.remove(recipient);
}

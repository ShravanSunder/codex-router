//! A retained event cursor closes the read/subscribe race without holding a database transaction open.
use agent_automation::{SubscriptionId, WakeupId};
use automation_storage::AutomationStore;
use collaboration_protocol::{
    FireKind, FireReceipt, SavedMessage, UuidIdentity, WakeChange, WakeChanged, WakeShowRequest,
    WakeSubscription,
};
use std::{io, sync::Arc};
use tokio::sync::{Mutex, OwnedSemaphorePermit};
pub(crate) struct WakeSubscriptionState {
    pub store: Arc<Mutex<AutomationStore>>,
    pub service_id: UuidIdentity,
    pub wakeup_id: WakeupId,
    pub subscription_id: SubscriptionId,
    pub sequence: i64,
    pub observed_at_ms: i64,
    pub permit: OwnedSemaphorePermit,
}
pub(crate) async fn start(
    store: Arc<Mutex<AutomationStore>>,
    service_id: UuidIdentity,
    request: WakeShowRequest,
    permit: OwnedSemaphorePermit,
) -> Result<(WakeSubscriptionState, WakeSubscription), automation_storage::StorageError> {
    // Admission/permit occurs before reading; retained events cover all subsequent commits.
    let record = store
        .lock()
        .await
        .read_wakeup::<SavedMessage>(&request.wakeup_id)
        .await?;
    let now = chrono::Utc::now().timestamp_millis();
    let sequence = record.latest_event_sequence;
    let snapshot = crate::wakeup_projection::snapshot(record, &service_id, now)
        .map_err(|_| automation_storage::StorageError::InvalidRecord)?;
    let subscription_id = SubscriptionId::generate();
    let result = WakeSubscription {
        subscription_id: subscription_id.clone(),
        after: snapshot.latest_event_cursor.clone(),
        snapshot,
    };
    Ok((
        WakeSubscriptionState {
            store,
            service_id,
            wakeup_id: request.wakeup_id,
            subscription_id,
            sequence,
            observed_at_ms: now,
            permit,
        },
        result,
    ))
}
/// Why a wait could not resume from its cursor.
pub(crate) enum WakeResumeError {
    InvalidCursor,
    NotFound,
    Unavailable,
}

const WAKE_EVENT_COLLECTION: &str = "automation-events";

impl WakeSubscriptionState {
    /// Resumes observing a wake-up after `cursor`, the automation-events position an earlier
    /// wait returned. The wake-up must still exist.
    pub(crate) async fn resume(
        store: Arc<Mutex<AutomationStore>>,
        service_id: UuidIdentity,
        wakeup_id: WakeupId,
        cursor: &str,
        permit: OwnedSemaphorePermit,
    ) -> Result<Self, WakeResumeError> {
        let (version, cursor_service, collection, sequence, observed_at_ms): (
            u8,
            UuidIdentity,
            String,
            i64,
            i64,
        ) = serde_json::from_str(cursor).map_err(|_| WakeResumeError::InvalidCursor)?;
        if version != 1
            || cursor_service != service_id
            || collection != WAKE_EVENT_COLLECTION
            || sequence < 0
            || observed_at_ms < 0
        {
            return Err(WakeResumeError::InvalidCursor);
        }
        let read = store
            .lock()
            .await
            .read_wakeup::<SavedMessage>(&wakeup_id)
            .await;
        match read {
            Ok(_) => {}
            Err(automation_storage::StorageError::WakeNotFound) => {
                return Err(WakeResumeError::NotFound);
            }
            Err(_) => return Err(WakeResumeError::Unavailable),
        }
        Ok(Self {
            store,
            service_id,
            wakeup_id,
            subscription_id: SubscriptionId::generate(),
            sequence,
            observed_at_ms,
            permit,
        })
    }

    /// The automation-events position this observation has reached.
    pub(crate) fn cursor(&self) -> io::Result<String> {
        serde_json::to_string(&(
            1_u8,
            &self.service_id,
            WAKE_EVENT_COLLECTION,
            self.sequence,
            self.observed_at_ms,
        ))
        .map_err(io::Error::other)
    }

    pub async fn next_changes(&mut self) -> io::Result<Vec<WakeChanged>> {
        let _retained_permit = &self.permit;
        let now = chrono::Utc::now();
        let cutoff = now
            .checked_sub_months(chrono::Months::new(2))
            .ok_or_else(|| io::Error::other("wake cursor retention overflow"))?
            .timestamp_millis();
        if self.observed_at_ms < cutoff {
            return Err(io::Error::other(
                "wake subscription history expired; reconnect to current state",
            ));
        }
        let events = self
            .store
            .lock()
            .await
            .wake_transitions_after(&self.wakeup_id, self.sequence)
            .await
            .map_err(io::Error::other)?;
        let mut changes = Vec::new();
        for event in events {
            if event.sequence <= self.sequence {
                return Err(io::Error::other("wake event sequence regressed"));
            }
            self.sequence = event.sequence;
            self.observed_at_ms = event.recorded_at_ms;
            let change = match event.kind.as_str() {
                "fired" => {
                    let fire: agent_automation::FirstFire = serde_json::from_value(
                        event
                            .body
                            .get("fire")
                            .cloned()
                            .ok_or_else(|| io::Error::other("firing evidence missing"))?,
                    )
                    .map_err(io::Error::other)?;
                    if fire.wakeup_id != self.wakeup_id {
                        return Err(io::Error::other("firing identity mismatch"));
                    }
                    WakeChange::Fired {
                        fire: FireReceipt {
                            kind: FireKind::WakeFired,
                            wakeup_id: fire.wakeup_id,
                            occurrence_id: fire.occurrence_id,
                            due_at: crate::wakeup_projection::timestamp(fire.due_at_ms)
                                .map_err(|_| io::Error::other("invalid due timestamp"))?,
                            fired_at: crate::wakeup_projection::timestamp(fire.fired_at_ms)
                                .map_err(|_| io::Error::other("invalid firing timestamp"))?,
                        },
                    }
                }
                "paused" => WakeChange::Paused,
                "cancelled" => WakeChange::Cancelled,
                "expired" => WakeChange::Expired,
                "finishedWithoutFiring" => WakeChange::FinishedWithoutFiring,
                "created" | "resumed" | "finished" | "coalesced" => continue,
                _ => return Err(io::Error::other("unknown wake transition")),
            };
            let cursor = serde_json::to_string(&(
                1_u8,
                &self.service_id,
                "automation-events",
                self.sequence,
                self.observed_at_ms,
            ))
            .map_err(io::Error::other)?;
            changes.push(WakeChanged {
                subscription_id: self.subscription_id.clone(),
                cursor,
                wakeup_id: self.wakeup_id.clone(),
                change,
            });
        }
        Ok(changes)
    }
}

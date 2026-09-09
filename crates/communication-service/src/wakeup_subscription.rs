//! A retained event cursor closes the read/subscribe race without holding a database transaction open.
use agent_automation::{SubscriptionId, WakeupId};
use automation_storage::AutomationStore;
use communication_protocol::{
    FireKind, FireReceipt, SavedMessage, UuidIdentity, WakeChange, WakeChanged, WakeShowRequest,
    WakeSubscription,
};
use serde_json::{Value, json};
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
impl WakeSubscriptionState {
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
pub(crate) fn unavailable(id: Value, wakeup_id: WakeupId) -> Value {
    let data=communication_protocol::WaitUnavailable{kind:communication_protocol::WaitUnavailableKind::WaitUnavailable,stage:communication_protocol::WaitStage::WaitForFirstFire,message:"Wake wait unavailable; reconnect the wait without recreating or cancelling the reminder.".into(),wakeup_id,first_occurrence_id:None,effects:communication_protocol::WaitUnavailableEffects{first_fire:communication_protocol::UnknownFire::Unknown,wakeup_mutation:communication_protocol::NoMutation::None},next_action:communication_protocol::WaitNextAction::ReconnectWait};
    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32050,"message":"Wake wait unavailable","data":data}})
}

pub(crate) fn not_found(id: Value, wakeup_id: WakeupId) -> Value {
    let data=communication_protocol::WakeNotFound{kind:communication_protocol::WakeNotFoundKind::WakeNotFound,stage:communication_protocol::WaitStage::WaitForFirstFire,message:"Wake-up was not found in this service; verify the wake identity. Historical firing is unknown.".into(),wakeup_id,first_occurrence_id:None,effects:communication_protocol::UnknownFirstFire{first_fire:communication_protocol::UnknownFire::Unknown},next_action:communication_protocol::VerifyWakeupAddress::VerifyWakeupAddress};
    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32050,"message":"Wake not found","data":data}})
}

//! DM presence, submission and response handling inside the target's single delivery actor.
use super::*;
use collaboration_protocol::{MessageDelivery, PushDeliveryState};

impl ReaderDeliveryOwner {
    pub(super) async fn deliver_direct_messages(&mut self) -> Result<bool, BoardError> {
        let Ok(target) = target_session(&self.reader) else {
            return Ok(false);
        };
        let records = self.push.direct_messages(&target).await?;
        self.dm_checked_presence
            .retain(|push_id, _| records.iter().any(|record| &record.push_id == push_id));
        for record in &records {
            let due = self
                .dm_checked_presence
                .get(&record.push_id)
                .is_none_or(|checked| {
                    self.clock.monotonic_now().duration_since(*checked) >= PRESENCE_INTERVAL
                });
            if !due {
                continue;
            }
            self.dm_checked_presence
                .insert(record.push_id.clone(), self.clock.monotonic_now());
            // Wakeable establishes unloaded presence. Unreachable is ambiguous:
            // Layer 0 must still classify live-elsewhere and permanent route failures.
            let known_unloaded = matches!(
                self.presence.presence(&target).await,
                Ok(TargetPresence::Wakeable)
            );
            let settled = if !known_unloaded {
                self.push.deliver_direct_message(record).await?
            } else if record.mode == Some(MessageDelivery::Steer) || record.guard.is_some() {
                self.push.reject_direct_message(record).await?
            } else {
                self.push.hold_direct_message(record).await?
            };
            #[cfg(test)]
            self.observe(if settled.delivery_state == PushDeliveryState::Held {
                OwnerObservation::DirectMessageHeld
            } else {
                OwnerObservation::DirectMessageSettled
            });
            if settled.delivery_state != PushDeliveryState::Held {
                self.dm_checked_presence.remove(&record.push_id);
            }
            self.answer_direct_message(&settled);
        }
        // An accepted command may name work settled on an earlier scan. Read back its
        // durable result rather than replaying the effect to satisfy the response.
        let unanswered = self.dm_replies.keys().cloned().collect::<Vec<_>>();
        for push_id in unanswered {
            let record = self.push.direct_message(&push_id).await?;
            if record.last_outcome.is_some() {
                self.answer_direct_message(&record);
            }
        }
        Ok(!self.push.direct_messages(&target).await?.is_empty())
    }

    fn answer_direct_message(&mut self, record: &PushRecord) {
        if let Some(replies) = self.dm_replies.remove(&record.push_id) {
            for reply in replies {
                let _ = reply.send(Ok(record.clone()));
            }
        }
    }
}

//! Inline cleanup of replaced or ended held snapshots; no detached owner writes.
use super::*;
use collaboration_protocol::PushKind;
use message_board::MessageShowRequest;

pub(super) enum HeldPushCleanup {
    Superseded(PushId),
    Ended,
}

impl ReaderDeliveryOwner {
    pub(super) fn queue_held_push_cleanup(&mut self, cleanup: HeldPushCleanup, reason: String) {
        match cleanup {
            HeldPushCleanup::Superseded(push_id) => {
                self.pending_held_cleanup.superseded = Some((push_id, reason))
            }
            HeldPushCleanup::Ended => self.pending_held_cleanup.ended_reason = Some(reason),
        }
    }

    /// One attempt per trigger/pass. Failure only retains a small marker; it
    /// never parks the Reader in a retry loop or carries a stale selection.
    pub(in super::super) async fn try_pending_held_push_cleanup(&mut self) {
        if !self.pending_held_cleanup.is_pending() {
            return;
        }
        if matches!(self.reader, Identity::Human { .. }) {
            self.pending_held_cleanup = PendingHeldPushCleanup::default();
            return;
        }
        if let Err(error) = target_session(&self.reader) {
            tracing::warn!(%error, field = "reader", "held-push cleanup has no target session; dropping marker");
            self.pending_held_cleanup = PendingHeldPushCleanup::default();
            return;
        }
        if let Some((push_id, reason)) = self.pending_held_cleanup.superseded.clone()
            && self
                .try_held_push_cleanup(&HeldPushCleanup::Superseded(push_id), &reason)
                .await
        {
            self.pending_held_cleanup.superseded = None;
        }
        if let Some(reason) = self.pending_held_cleanup.ended_reason.clone()
            && self
                .try_held_push_cleanup(&HeldPushCleanup::Ended, &reason)
                .await
        {
            self.pending_held_cleanup.ended_reason = None;
        }
    }

    async fn try_held_push_cleanup(&self, cleanup: &HeldPushCleanup, reason: &str) -> bool {
        match self.settle_selected_held_pushes(cleanup, reason).await {
            Ok(_settled) => {
                #[cfg(test)]
                self.observe(OwnerObservation::HeldSubscriptionPushesSettled(_settled));
                true
            }
            Err(error) => {
                tracing::warn!(%error, %reason, field = "heldSubscriptionPushes", "held-push cleanup deferred to next owner pass");
                false
            }
        }
    }

    async fn settle_selected_held_pushes(
        &self,
        cleanup: &HeldPushCleanup,
        reason: &str,
    ) -> Result<u64, BoardError> {
        let target = target_session(&self.reader)?;
        let (created_before, candidates) = {
            let mut store = self.push.store.lock().await;
            let cutoff = match cleanup {
                HeldPushCleanup::Superseded(push_id) => Some(
                    store
                        .get_push_record(push_id)
                        .await
                        .map_err(|_| BoardError::board_unavailable())?
                        .ok_or_else(BoardError::board_unavailable)?
                        .created_at,
                ),
                HeldPushCleanup::Ended => None,
            };
            let candidates = store
                .list_held_push_records(&target)
                .await
                .map_err(|_| BoardError::board_unavailable())?;
            (cutoff, candidates)
        };
        let mut settled = 0;
        for record in candidates.into_iter().filter(|record| {
            record.kind == PushKind::SubscriptionActivity
                && created_before.is_none_or(|cutoff| record.created_at < cutoff)
        }) {
            // Ended cleanup is about the entire snapshot, not just one scope
            // that happened to disappear from the owner's current record list.
            if matches!(cleanup, HeldPushCleanup::Ended)
                && !self.all_push_roots_ended(&record).await?
            {
                continue;
            }
            settled += self
                .push
                .store
                .lock()
                .await
                .settle_held_subscription_push(
                    &target,
                    &record.push_id,
                    created_before,
                    reason,
                    self.clock.now(),
                )
                .await
                .map_err(|_| BoardError::board_unavailable())?;
        }
        Ok(settled)
    }

    async fn all_push_roots_ended(&self, record: &PushRecord) -> Result<bool, BoardError> {
        let Some(activity) = record.activity.as_ref() else {
            return Ok(false);
        };
        if activity.ranges.is_empty() {
            return Ok(false);
        }
        let mut board = self.board_store()?.lock().await;
        for range in &activity.ranges {
            let thread_scope = SubscriptionScope::thread(range.root_message_id.clone());
            let covering = match board
                .get_thread_subscription_record(&self.reader, &thread_scope)
                .await?
            {
                Some(record) => Some(record), // Thread overrides also shadow Topic scopes when ended.
                None => {
                    let root = board
                        .show_message(MessageShowRequest {
                            message_id: range.root_message_id.clone(),
                        })
                        .await?
                        .0;
                    board
                        .get_thread_subscription_record(
                            &self.reader,
                            &SubscriptionScope::topic(root.topic_id),
                        )
                        .await?
                }
            };
            if covering.is_some_and(|record| {
                record.state().is_delivery_eligible() && record.expires_at() > self.clock.now()
            }) {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

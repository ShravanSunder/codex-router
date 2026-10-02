//! Inline cleanup of replaced or ended held snapshots; no detached owner writes.
use super::*;
use collaboration_protocol::PushKind;
use message_board::MessageShowRequest;

pub(super) enum HeldPushCleanup {
    Superseded(PushId),
    Ended,
}

impl ReaderDeliveryOwner {
    pub(super) async fn complete_held_push_cleanup(
        &self,
        cleanup: HeldPushCleanup,
        reason: String,
    ) -> Result<(), BoardError> {
        let mut delay = Duration::from_secs(1);
        loop {
            if self.shutdown.is_cancelled() {
                return Err(BoardError::board_unavailable());
            }
            match self.settle_selected_held_pushes(&cleanup, &reason).await {
                Ok(_settled) => {
                    #[cfg(test)]
                    self.observe(OwnerObservation::HeldSubscriptionPushesSettled(_settled));
                    return Ok(());
                }
                Err(error) => {
                    tracing::warn!(%error, %reason, "retrying held subscription-push cleanup inline")
                }
            }
            let deadline = self
                .clock
                .monotonic_now()
                .checked_add(delay)
                .ok_or_else(BoardError::board_unavailable)?;
            tokio::select! {
                () = self.shutdown.cancelled() => return Err(BoardError::board_unavailable()),
                () = self.clock.sleep_until(deadline) => {}
            }
            delay = delay.saturating_mul(2).min(Duration::from_secs(30));
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

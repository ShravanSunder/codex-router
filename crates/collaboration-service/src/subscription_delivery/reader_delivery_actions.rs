//! Subscription lifecycle, policy decisions, and durable board settlement.
use super::*;

impl ReaderDeliveryOwner {
    pub(super) async fn expiry_notices(&self, records: &[ThreadSubscriptionRecord]) {
        let Ok(store) = self.board_store() else {
            return;
        };
        for prior in &self.prior_records {
            if records.iter().any(|record| record.scope() == prior.scope()) {
                continue;
            }
            let expired = store
                .lock()
                .await
                .get_thread_subscription_record(&self.reader, prior.scope())
                .await;
            let Ok(Some(expired)) = expired else {
                continue;
            };
            if expired.state().end_reason() != Some(EndReason::Expired)
                || expired.policy().mode() != SubscriptionMode::Deliver
            {
                continue;
            }
            let Ok(target) = target_session(&self.reader) else {
                continue;
            };
            if !matches!(
                self.presence.presence(&target).await,
                Ok(TargetPresence::Running)
            ) {
                continue;
            }
            if let Ok(prepared) = self
                .push
                .expiry(&target, expired.scope(), expired.generation())
                .await
            {
                let _ = self.push.deliver(&target, prepared).await;
            }
        }
    }

    pub(super) async fn complete_drains(&self, records: &[ThreadSubscriptionRecord]) -> bool {
        let Ok(store) = self.board_store() else {
            return false;
        };
        for record in records
            .iter()
            .filter(|record| record.state() == SubscriptionState::Draining)
        {
            let SubscriptionScope::Thread { root_message_id } = record.scope() else {
                continue;
            };
            if store
                .lock()
                .await
                .complete_thread_subscription_drain(&self.reader, root_message_id, self.clock.now())
                .await
                .is_ok()
            {
                return true;
            }
        }
        false
    }

    pub(super) async fn deliver_due(
        &mut self,
        due: &[MessageId],
        facts: &HashMap<MessageId, RootSubscriptionFacts>,
    ) -> Result<bool, BoardError> {
        let roots = due
            .iter()
            .filter(|root| {
                facts.get(*root).is_some_and(|facts| {
                    facts.mode == SubscriptionMode::Deliver
                        && (facts.held_since.is_none()
                            || self.checked_presence.get(*root).is_none_or(|checked| {
                                self.clock.monotonic_now().duration_since(*checked)
                                    >= PRESENCE_INTERVAL
                            }))
                })
            })
            .cloned()
            .collect::<Vec<_>>();
        if roots.is_empty() {
            return Ok(false);
        }
        let target = target_session(&self.reader)?;
        let presence = self
            .presence
            .presence(&target)
            .await
            .unwrap_or_else(|error| TargetPresence::Unreachable {
                reason: error.to_string(),
            });
        let mut dropped = Vec::new();
        let mut held = Vec::new();
        let mut delivered = Vec::new();
        let mut load_policy = LoadPolicy::LoadedOnly;
        for root in roots {
            self.checked_presence
                .insert(root.clone(), self.clock.monotonic_now());
            let Some(facts) = facts.get(&root) else {
                continue;
            };
            match (&presence, facts.when_idle) {
                (TargetPresence::Running, _) => delivered.push(root),
                (_, WhenIdle::Drop) => dropped.push(root),
                (TargetPresence::Wakeable, WhenIdle::Wake) => {
                    load_policy = LoadPolicy::MayLoad;
                    delivered.push(root);
                }
                _ => held.push(root),
            }
        }
        if !dropped.is_empty() {
            self.board_store()?
                .lock()
                .await
                .drop_subscription_roots(&self.reader, &dropped, self.clock.now())
                .await?;
        }
        if !held.is_empty() {
            let (_, settlement) = self.select(&held, usize::MAX).await?;
            self.hold(
                &settlement,
                if held.iter().any(|root| {
                    facts
                        .get(root)
                        .is_some_and(|facts| facts.when_idle == WhenIdle::Wake)
                }) {
                    "wake unavailable; holding"
                } else {
                    "target is not running"
                },
            )
            .await?;
        }
        if !delivered.is_empty() {
            let (batch, settlement) = self.select(&delivered, usize::MAX).await?;
            if batch.roots.is_empty() {
                return Ok(true);
            }
            let prepared = match self.push.activity(&target, &batch, load_policy).await {
                Ok(prepared) => prepared,
                Err(error) => {
                    self.release(&settlement).await;
                    return Err(error);
                }
            };
            let receipt = match self.push.deliver(&target, prepared).await {
                Ok(receipt) => receipt,
                Err(error) => {
                    self.release(&settlement).await;
                    return Err(error);
                }
            };
            self.handle_receipt(&settlement, &receipt, facts).await?;
        }
        Ok(true)
    }

    pub(super) async fn select(
        &self,
        roots: &[MessageId],
        maximum_root_notice_bytes: usize,
    ) -> Result<(SubscriptionBatch, SubscriptionBatchSettlement), BoardError> {
        let selected = self
            .board_store()?
            .lock()
            .await
            .select_subscription_notice(
                &self.reader,
                roots,
                self.clock.now(),
                maximum_root_notice_bytes,
            )
            .await?;
        #[cfg(test)]
        self.observe(OwnerObservation::Selected);
        Ok(selected)
    }

    pub(super) async fn handle_receipt(
        &self,
        settlement: &SubscriptionBatchSettlement,
        delivered: &super::super::subscription_push::SubscriptionPushReceipt,
        facts: &HashMap<MessageId, RootSubscriptionFacts>,
    ) -> Result<(), BoardError> {
        let evidence = delivered.evidence.clone();
        match &delivered.receipt.outcome {
            DeliveryOutcome::Started
            | DeliveryOutcome::Steered
            | DeliveryOutcome::StartedOrSteered
            | DeliveryOutcome::Queued
            | DeliveryOutcome::PeerMessageWritten => {
                self.settle(settlement, SubscriptionDeliveryOutcome::Accepted)
                    .await
            }
            DeliveryOutcome::Unknown => {
                self.settle(
                    settlement,
                    SubscriptionDeliveryOutcome::Unknown { evidence },
                )
                .await
            }
            DeliveryOutcome::NotSubmitted {
                retryable: true,
                reason,
            } => self.hold(settlement, reason).await,
            outcome => {
                let attempts = settlement
                    .roots
                    .iter()
                    .filter_map(|root| facts.get(&root.root_message_id))
                    .map(|facts| facts.retry_attempts)
                    .max()
                    .unwrap_or(0);
                let delay = chrono::Duration::from_std(delivery_retry_delay(attempts))
                    .map_err(|_| BoardError::board_unavailable())?;
                let now = self.clock.now();
                let retry_at = now
                    .checked_add_signed(delay)
                    .ok_or_else(BoardError::board_unavailable)?;
                let outcome = match outcome {
                    DeliveryOutcome::NotSubmitted { reason, retryable } => {
                        SubscriptionDeliveryOutcome::NotSubmitted {
                            reason: reason.clone(),
                            retryable: *retryable,
                        }
                    }
                    _ => SubscriptionDeliveryOutcome::Rejected { evidence },
                };
                self.board_store()?
                    .lock()
                    .await
                    .mark_subscription_batch_retry(&self.reader, settlement, now, retry_at, outcome)
                    .await?;
                #[cfg(test)]
                self.observe(OwnerObservation::RetryScheduled);
                Ok(())
            }
        }
    }

    pub(super) async fn settle(
        &self,
        settlement: &SubscriptionBatchSettlement,
        outcome: SubscriptionDeliveryOutcome,
    ) -> Result<(), BoardError> {
        let mut delay = Duration::from_secs(1);
        loop {
            if self
                .board_store()?
                .lock()
                .await
                .settle_subscription_batch(&self.reader, settlement, outcome.clone())
                .await
                .is_ok()
            {
                #[cfg(test)]
                self.observe(OwnerObservation::Settled);
                return Ok(());
            }
            #[cfg(test)]
            self.observe(OwnerObservation::SettlementRetry);
            let deadline = self
                .clock
                .monotonic_now()
                .checked_add(delay)
                .ok_or_else(BoardError::board_unavailable)?;
            tokio::select! { () = self.shutdown.cancelled() => return Err(BoardError::board_unavailable()), () = self.clock.sleep_until(deadline) => {} }
            delay = delay.saturating_mul(2).min(Duration::from_secs(30));
        }
    }

    pub(super) async fn hold(
        &self,
        settlement: &SubscriptionBatchSettlement,
        reason: &str,
    ) -> Result<(), BoardError> {
        self.board_store()?
            .lock()
            .await
            .mark_subscription_batch_held(
                &self.reader,
                settlement,
                self.clock.now(),
                SubscriptionDeliveryOutcome::NotSubmitted {
                    reason: reason.to_owned(),
                    retryable: true,
                },
            )
            .await?;
        #[cfg(test)]
        self.observe(OwnerObservation::Held);
        Ok(())
    }

    pub(super) async fn release(&self, settlement: &SubscriptionBatchSettlement) {
        let Ok(store) = self.board_store() else {
            return;
        };
        if let Err(error) = store
            .lock()
            .await
            .release_subscription_batch(&self.reader, settlement)
            .await
        {
            tracing::warn!(%error, "unhanded subscription selection could not be released");
        }
    }
}

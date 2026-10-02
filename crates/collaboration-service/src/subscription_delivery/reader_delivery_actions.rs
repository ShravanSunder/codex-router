//! Subscription lifecycle, policy decisions, and durable board settlement.
use super::*;
#[path = "reader_held_push_settlement.rs"]
mod reader_held_push_settlement;
use reader_held_push_settlement::HeldPushCleanup;

/// Each selected batch keeps ownership until its durable disposition is written.
enum SubscriptionBatchWrite {
    Settle(SubscriptionDeliveryOutcome),
    Hold {
        now: chrono::DateTime<chrono::Utc>,
        outcome: SubscriptionDeliveryOutcome,
    },
    Retry {
        now: chrono::DateTime<chrono::Utc>,
        retry_at: chrono::DateTime<chrono::Utc>,
        outcome: SubscriptionDeliveryOutcome,
    },
}

impl ReaderDeliveryOwner {
    pub(super) async fn expiry_notices(&self, records: &[ThreadSubscriptionRecord]) {
        let Ok(store) = self.board_store() else {
            return;
        };
        let mut ended_reason = None;
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
            let Some(end_reason) = expired.state().end_reason() else {
                continue;
            };
            ended_reason.get_or_insert(end_reason);
            if end_reason != EndReason::Expired
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
        if let Some(end_reason) = ended_reason
            && let Err(error) = self
                .complete_held_push_cleanup(
                    HeldPushCleanup::Ended,
                    format!("subscription ended: {}", end_reason.as_str()),
                )
                .await
        {
            tracing::warn!(%error, "ended subscription-push cleanup interrupted");
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
        let result = match &delivered.receipt.outcome {
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
                self.complete_batch_write(
                    settlement,
                    SubscriptionBatchWrite::Retry {
                        now,
                        retry_at,
                        outcome,
                    },
                )
                .await?;
                #[cfg(test)]
                self.observe(OwnerObservation::RetryScheduled);
                Ok(())
            }
        };
        result?;
        let link = collaboration_protocol::RouterLink::new(
            collaboration_protocol::MachineId::from(self.push.machine.service_id().clone()),
            delivered.push_id.clone(),
        );
        self.complete_held_push_cleanup(
            HeldPushCleanup::Superseded(delivered.push_id.clone()),
            format!("superseded by {link}"),
        )
        .await?;
        Ok(())
    }

    pub(super) async fn settle(
        &self,
        settlement: &SubscriptionBatchSettlement,
        outcome: SubscriptionDeliveryOutcome,
    ) -> Result<(), BoardError> {
        self.complete_batch_write(settlement, SubscriptionBatchWrite::Settle(outcome))
            .await?;
        #[cfg(test)]
        self.observe(OwnerObservation::Settled);
        Ok(())
    }

    pub(super) async fn hold(
        &self,
        settlement: &SubscriptionBatchSettlement,
        reason: &str,
    ) -> Result<(), BoardError> {
        self.complete_batch_write(
            settlement,
            SubscriptionBatchWrite::Hold {
                now: self.clock.now(),
                outcome: SubscriptionDeliveryOutcome::NotSubmitted {
                    reason: reason.to_owned(),
                    retryable: true,
                },
            },
        )
        .await?;
        #[cfg(test)]
        self.observe(OwnerObservation::Held);
        Ok(())
    }

    async fn complete_batch_write(
        &self,
        settlement: &SubscriptionBatchSettlement,
        write: SubscriptionBatchWrite,
    ) -> Result<(), BoardError> {
        let mut delay = Duration::from_secs(1);
        loop {
            let result = {
                let mut store = self.board_store()?.lock().await;
                match &write {
                    SubscriptionBatchWrite::Settle(outcome) => {
                        store
                            .settle_subscription_batch(&self.reader, settlement, outcome.clone())
                            .await
                    }
                    SubscriptionBatchWrite::Hold { now, outcome } => {
                        store
                            .mark_subscription_batch_held(
                                &self.reader,
                                settlement,
                                *now,
                                outcome.clone(),
                            )
                            .await
                    }
                    SubscriptionBatchWrite::Retry {
                        now,
                        retry_at,
                        outcome,
                    } => {
                        store
                            .mark_subscription_batch_retry(
                                &self.reader,
                                settlement,
                                *now,
                                *retry_at,
                                outcome.clone(),
                            )
                            .await
                    }
                }
            };
            match result {
                Ok(()) => return Ok(()),
                Err(error) => {
                    tracing::warn!(%error, "selected subscription batch write failed; retaining selection for retry")
                }
            }
            #[cfg(test)]
            self.observe(OwnerObservation::SettlementRetry);
            let deadline = self
                .clock
                .monotonic_now()
                .checked_add(delay)
                .ok_or_else(BoardError::board_unavailable)?;
            tokio::select! {
                () = self.shutdown.cancelled() => {
                    self.release(settlement).await;
                    return Err(BoardError::board_unavailable());
                },
                () = self.clock.sleep_until(deadline) => {}
            }
            delay = delay.saturating_mul(2).min(Duration::from_secs(30));
        }
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

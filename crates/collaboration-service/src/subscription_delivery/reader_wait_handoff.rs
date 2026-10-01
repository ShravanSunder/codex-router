//! FIFO poll handoff and optional session notice storage.
use super::*;

impl ReaderDeliveryOwner {
    pub(super) async fn hand_to_waiter(
        &mut self,
        due: &[MessageId],
        facts: &HashMap<MessageId, RootSubscriptionFacts>,
    ) -> Result<bool, BoardError> {
        for index in 0..self.waiters.len() {
            let Some(waiter) = self.waiters.get(index) else {
                continue;
            };
            let filter = waiter.filter.clone();
            let mut roots = Vec::new();
            for root in due.iter().filter(|root| {
                facts
                    .get(*root)
                    .is_some_and(|facts| facts.mode == SubscriptionMode::Poll)
            }) {
                if filter.matches(root, self.board_store()?).await? {
                    roots.push(root.clone());
                }
            }
            if roots.is_empty() {
                continue;
            }
            let (batch, settlement) = self.select(&roots).await?;
            let Some(waiter) = self.waiters.remove(index) else {
                self.release(&settlement).await;
                return Ok(false);
            };
            let prepared = if matches!(self.reader, Identity::Session { .. }) {
                let target = target_session(&self.reader)?;
                match self
                    .push
                    .activity(&target, &batch, LoadPolicy::LoadedOnly)
                    .await
                {
                    Ok(prepared) => {
                        if let Err(error) = self.push.mark_attempted(&prepared).await {
                            self.release(&settlement).await;
                            let _ = waiter.reply.send(Err(error));
                            return Ok(true);
                        }
                        Some(prepared)
                    }
                    Err(error) => {
                        self.release(&settlement).await;
                        let _ = waiter.reply.send(Err(error));
                        return Ok(true);
                    }
                }
            } else {
                None
            };
            let result = match &prepared {
                Some(prepared) => SubscriptionWaitResult::Notice {
                    push_id: prepared.push_id.clone(),
                    line: prepared.line.clone(),
                    batch,
                },
                None => SubscriptionWaitResult::Ranges { batch },
            };
            let accepted = waiter.reply.send(Ok(Some(result))).is_ok();
            if let Some(prepared) = prepared {
                let outcome = if accepted {
                    DeliveryOutcome::Started
                } else {
                    DeliveryOutcome::NotSubmitted {
                        retryable: false,
                        reason: "wait caller disconnected before handoff".to_owned(),
                    }
                };
                self.push
                    .record_receipt(
                        &prepared,
                        &DeliveryReceipt {
                            outcome,
                            reachability: None,
                            client: None,
                        },
                        true,
                    )
                    .await?;
            }
            if accepted {
                self.settle(&settlement, SubscriptionDeliveryOutcome::Accepted)
                    .await?;
            } else {
                self.release(&settlement).await;
            }
            return Ok(true);
        }
        Ok(false)
    }
}

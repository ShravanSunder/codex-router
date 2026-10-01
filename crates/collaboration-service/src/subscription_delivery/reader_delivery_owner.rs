#[path = "reader_delivery_actions.rs"]
mod reader_delivery_actions;
#[path = "reader_wait_handoff.rs"]
mod reader_wait_handoff;
#[cfg(test)]
use super::subscription_service::OwnerObservation;
use super::{
    BoardAvailability, SubscriptionClock,
    subscription_facts::{
        PRESENCE_INTERVAL, RootSubscriptionFacts, delivery_retry_delay, root_facts, wall_deadline,
    },
    subscription_push::{SubscriptionPushStore, target_session},
    subscription_wait::{PollWaiter, SubscriptionWaitFilter, SubscriptionWaitResult},
};
use crate::{LoadPolicy, TargetPresence, TargetPresenceProbe};
use collaboration_protocol::{DeliveryOutcome, DeliveryReceipt};
use message_board::{
    BoardError, EndReason, Identity, MessageId, SubscriptionBatch, SubscriptionBatchSettlement,
    SubscriptionDeliveryOutcome, SubscriptionMode, SubscriptionScope, SubscriptionState,
    ThreadSubscriptionRecord, WhenIdle,
};
use message_board_storage::BoardStore;
use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
    time::Duration,
};
use tokio::sync::{Mutex, broadcast, mpsc, oneshot};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

pub(super) enum ReaderDeliveryCommand {
    #[cfg(test)]
    Barrier(oneshot::Sender<()>),
    Reconcile,
    Wait {
        filter: SubscriptionWaitFilter,
        deadline: Instant,
        reply: oneshot::Sender<Result<Option<SubscriptionWaitResult>, BoardError>>,
    },
}

pub(super) struct ReaderDeliveryOwnerProps {
    pub reader: Identity,
    pub board_availability: BoardAvailability,
    pub push: Arc<SubscriptionPushStore>,
    pub presence: Arc<dyn TargetPresenceProbe>,
    pub clock: Arc<dyn SubscriptionClock>,
    pub shutdown: CancellationToken,
    pub commands: mpsc::Receiver<ReaderDeliveryCommand>,
    pub activity: Option<broadcast::Receiver<()>>,
    #[cfg(test)]
    pub observations: broadcast::Sender<OwnerObservation>,
}

pub(super) struct ReaderDeliveryOwner {
    reader: Identity,
    board_availability: BoardAvailability,
    push: Arc<SubscriptionPushStore>,
    presence: Arc<dyn TargetPresenceProbe>,
    clock: Arc<dyn SubscriptionClock>,
    shutdown: CancellationToken,
    commands: mpsc::Receiver<ReaderDeliveryCommand>,
    activity: Option<broadcast::Receiver<()>>,
    waiters: VecDeque<PollWaiter>,
    checked_presence: HashMap<MessageId, Instant>,
    prior_records: Vec<ThreadSubscriptionRecord>,
    rescan: bool,
    #[cfg(test)]
    barriers: Vec<oneshot::Sender<()>>,
    #[cfg(test)]
    observations: broadcast::Sender<OwnerObservation>,
}

impl ReaderDeliveryOwner {
    pub(super) fn new(props: ReaderDeliveryOwnerProps) -> Self {
        let ReaderDeliveryOwnerProps {
            reader,
            board_availability,
            push,
            presence,
            clock,
            shutdown,
            commands,
            activity,
            #[cfg(test)]
            observations,
        } = props;
        Self {
            reader,
            board_availability,
            push,
            presence,
            clock,
            shutdown,
            commands,
            activity,
            waiters: VecDeque::new(),
            checked_presence: HashMap::new(),
            prior_records: Vec::new(),
            rescan: true,
            #[cfg(test)]
            barriers: Vec::new(),
            #[cfg(test)]
            observations,
        }
    }

    pub(super) async fn run(mut self) {
        #[cfg(test)]
        self.observe(OwnerObservation::Started);
        while !self.shutdown.is_cancelled() {
            self.expire_waiters();
            if self.rescan {
                let result = match &self.board_availability {
                    BoardAvailability::Available(store) => {
                        store
                            .lock()
                            .await
                            .rescan_missing_subscription_windows(&self.reader, self.clock.now())
                            .await
                    }
                    BoardAvailability::Unavailable => Ok(0),
                };
                if result.is_err() {
                    if !self.wait_for_input(Some(self.retry_deadline())).await {
                        break;
                    }
                    continue;
                }
                self.rescan = false;
            }
            let now = self.clock.now();
            let result = match &self.board_availability {
                BoardAvailability::Available(store) => {
                    store
                        .lock()
                        .await
                        .list_reader_subscriptions(&self.reader, now)
                        .await
                }
                BoardAvailability::Unavailable => Ok(Vec::new()),
            };
            let records = match result {
                Ok(records) => records,
                Err(error) => {
                    tracing::warn!(%error, "subscription reload failed");
                    if !self.wait_for_input(Some(self.retry_deadline())).await {
                        break;
                    }
                    continue;
                }
            };
            #[cfg(test)]
            self.observe(OwnerObservation::RowsLoaded(records.len()));
            self.expiry_notices(&records).await;
            self.prior_records = records.clone();
            if records.is_empty() && self.waiters.is_empty() {
                break;
            }
            if self.complete_drains(&records).await {
                continue;
            }
            let facts = root_facts(&records);
            #[cfg(test)]
            self.observe(OwnerObservation::SelectingDueRoots);
            let due_result = match &self.board_availability {
                BoardAvailability::Available(store) => {
                    store
                        .lock()
                        .await
                        .due_subscription_roots(&self.reader, now)
                        .await
                }
                BoardAvailability::Unavailable => Ok(Vec::new()),
            };
            let due = match due_result {
                Ok(roots) => roots,
                Err(error) => {
                    tracing::warn!(%error, "subscription due selection failed");
                    Vec::new()
                }
            };
            #[cfg(test)]
            self.observe(OwnerObservation::DueRoots(due.len()));
            match self.deliver_due(&due, &facts).await {
                Ok(true) => continue,
                Ok(false) => {}
                Err(error) => {
                    tracing::warn!(%error, "subscription push failed before effect");
                    if !self.wait_for_input(Some(self.retry_deadline())).await {
                        break;
                    }
                    continue;
                }
            }
            match self.hand_to_waiter(&due, &facts).await {
                Ok(true) => continue,
                Ok(false) => {}
                Err(error) => {
                    tracing::warn!(%error, "subscription wait selection failed");
                    if !self.wait_for_input(Some(self.retry_deadline())).await {
                        break;
                    }
                    continue;
                }
            }
            let deadline = self.next_deadline(&records, &facts).await;
            if !self.wait_for_input(deadline).await {
                break;
            }
        }
        while let Some(waiter) = self.waiters.pop_front() {
            let _ = waiter.reply.send(Ok(None));
        }
        #[cfg(test)]
        self.observe(OwnerObservation::Stopped);
    }

    fn expire_waiters(&mut self) {
        let mut retained = VecDeque::new();
        while let Some(waiter) = self.waiters.pop_front() {
            if waiter.reply.is_closed() {
                continue;
            }
            if waiter.deadline <= self.clock.monotonic_now() {
                let _ = waiter.reply.send(Ok(None));
            } else {
                retained.push_back(waiter);
            }
        }
        self.waiters = retained;
    }

    async fn next_deadline(
        &self,
        records: &[ThreadSubscriptionRecord],
        facts: &HashMap<MessageId, RootSubscriptionFacts>,
    ) -> Option<Instant> {
        let now = self.clock.now();
        let monotonic = self.clock.monotonic_now();
        let mut next_deadline = self
            .waiters
            .iter()
            .map(|waiter| waiter.deadline)
            .chain(
                records
                    .iter()
                    .map(|record| wall_deadline(record.expires_at(), now, monotonic)),
            )
            .min();
        let mut include_deadline = |deadline: Instant| {
            next_deadline = Some(next_deadline.map_or(deadline, |current| current.min(deadline)));
        };
        for (root, facts) in facts {
            if facts.mode == SubscriptionMode::Off {
                continue;
            }
            if facts.mode == SubscriptionMode::Poll {
                let mut covered = false;
                for waiter in &self.waiters {
                    covered |= waiter
                        .filter
                        .matches(root, self.board_store().ok()?)
                        .await
                        .unwrap_or(false);
                }
                if !covered {
                    continue;
                }
            }
            if let Some(retry) = facts.next_retry_at {
                include_deadline(wall_deadline(retry, now, monotonic));
                continue;
            }
            if facts.held_since.is_some() {
                include_deadline(
                    self.checked_presence
                        .get(root)
                        .and_then(|checked| checked.checked_add(PRESENCE_INTERVAL))
                        .unwrap_or(monotonic),
                );
                continue;
            }
            let quiet = facts
                .last_arrival_at
                .checked_add_signed(chrono::Duration::seconds(
                    i64::try_from(facts.quiet_seconds).unwrap_or(i64::MAX),
                ));
            let cap = facts
                .opened_at
                .checked_add_signed(chrono::Duration::seconds(
                    i64::try_from(facts.cap_seconds).unwrap_or(i64::MAX),
                ));
            if let (Some(quiet), Some(cap)) = (quiet, cap) {
                include_deadline(wall_deadline(quiet.min(cap), now, monotonic));
            }
        }
        next_deadline
    }

    async fn wait_for_input(&mut self, deadline: Option<Instant>) -> bool {
        #[cfg(test)]
        for barrier in self.barriers.drain(..) {
            let _ = barrier.send(());
        }
        #[cfg(test)]
        self.observe(OwnerObservation::Sleeping(deadline));
        let clock = self.clock.clone();
        let timer = async {
            match deadline {
                Some(deadline) => clock.sleep_until(deadline).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            () = self.shutdown.cancelled() => false,
            command = self.commands.recv() => {
                match command {
                    #[cfg(test)]
                    Some(ReaderDeliveryCommand::Barrier(reply)) => { self.barriers.push(reply); true },
                    Some(ReaderDeliveryCommand::Reconcile) => { self.rescan = true; true },
                    Some(ReaderDeliveryCommand::Wait { filter, deadline, reply }) => {
                        let result = self.renew_wait(&filter).await;
                        match result { Ok(()) => { self.waiters.push_back(PollWaiter { filter, deadline, reply }); #[cfg(test)] self.observe(OwnerObservation::WaitQueued); }, Err(error) => { let _ = reply.send(Err(error)); } }
                        true
                    }
                    None => false,
                }
            },
            activity = async { match self.activity.as_mut() {
                Some(activity) => activity.recv().await,
                None => std::future::pending().await,
            } } => { if matches!(activity, Err(broadcast::error::RecvError::Lagged(_))) { self.rescan = true; } !matches!(activity, Err(broadcast::error::RecvError::Closed)) },
            () = timer => true,
        }
    }

    async fn renew_wait(&self, filter: &SubscriptionWaitFilter) -> Result<(), BoardError> {
        let records = self
            .board_store()?
            .lock()
            .await
            .list_reader_subscriptions(&self.reader, self.clock.now())
            .await?;
        let scopes = filter
            .covered_poll_scopes(&records, self.board_store()?)
            .await?;
        for scope in scopes {
            self.board_store()?
                .lock()
                .await
                .renew_thread_subscription(&self.reader, &scope, self.clock.now())
                .await?;
        }
        Ok(())
    }

    fn board_store(&self) -> Result<&Arc<Mutex<BoardStore>>, BoardError> {
        self.board_availability.require_store()
    }

    fn retry_deadline(&self) -> Instant {
        self.clock
            .monotonic_now()
            .checked_add(Duration::from_secs(1))
            .unwrap_or_else(|| self.clock.monotonic_now())
    }
    #[cfg(test)]
    fn observe(&self, observation: OwnerObservation) {
        let _ = self.observations.send(observation);
    }
}

use super::{
    SubscriptionClock,
    reader_delivery_owner::{ReaderDeliveryCommand, ReaderDeliveryOwner, ReaderDeliveryOwnerProps},
    subscription_push::{SubscriptionPushStore, SubscriptionPushStoreProps},
    subscription_wait::SubscriptionWaitFilter,
};
use crate::{MachineIdentity, SessionMessageDelivery, TargetPresenceProbe};
use message_board::{BoardError, Identity};
use message_board_storage::BoardStore;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

pub struct SubscriptionDeliveryServiceProps {
    pub store: Arc<Mutex<BoardStore>>,
    pub push_store: Arc<Mutex<automation_storage::AutomationStore>>,
    pub delivery: Arc<dyn SessionMessageDelivery>,
    pub presence: Arc<dyn TargetPresenceProbe>,
    pub machine_identity: MachineIdentity,
    pub clock: Arc<dyn SubscriptionClock>,
}

#[cfg(test)]
#[derive(Clone, Debug)]
pub(super) enum OwnerObservation {
    Started,
    RowsLoaded(usize),
    SelectingDueRoots,
    DueRoots(usize),
    Selected,
    Settled,
    SettlementRetry,
    PushSettlementRetry,
    Held,
    RetryScheduled,
    WaitQueued,
    Sleeping(Option<tokio::time::Instant>),
    Stopped,
}

struct ServiceInner {
    store: Arc<Mutex<BoardStore>>,
    push: Arc<SubscriptionPushStore>,
    presence: Arc<dyn TargetPresenceProbe>,
    clock: Arc<dyn SubscriptionClock>,
    owners: Mutex<HashMap<Identity, mpsc::Sender<ReaderDeliveryCommand>>>,
    start_gate: Mutex<bool>,
    shutdown: CancellationToken,
    tasks: TaskTracker,
    #[cfg(test)]
    observations: tokio::sync::broadcast::Sender<OwnerObservation>,
}

#[derive(Clone)]
pub struct SubscriptionDeliveryService {
    inner: Arc<ServiceInner>,
    lifetime: Arc<()>,
}

impl SubscriptionDeliveryService {
    pub fn new(props: SubscriptionDeliveryServiceProps) -> Self {
        #[cfg(test)]
        let (observations, _) = tokio::sync::broadcast::channel(1024);
        let shutdown = CancellationToken::new();
        let push = Arc::new(SubscriptionPushStore::new(SubscriptionPushStoreProps {
            store: props.push_store,
            delivery: props.delivery,
            machine: props.machine_identity,
            clock: props.clock.clone(),
            shutdown: shutdown.clone(),
            #[cfg(test)]
            observations: observations.clone(),
        }));
        Self {
            lifetime: Arc::new(()),
            inner: Arc::new(ServiceInner {
                store: props.store,
                push,
                presence: props.presence,
                clock: props.clock,
                owners: Mutex::new(HashMap::new()),
                start_gate: Mutex::new(false),
                shutdown,
                tasks: TaskTracker::new(),
                #[cfg(test)]
                observations,
            }),
        }
    }

    pub async fn start(&self) -> Result<(), BoardError> {
        let mut started = self.inner.start_gate.lock().await;
        if self.inner.shutdown.is_cancelled() {
            return Err(BoardError::board_unavailable());
        }
        if *started {
            return Ok(());
        }
        let records = self
            .inner
            .store
            .lock()
            .await
            .restore_active_and_draining(self.inner.clock.now())
            .await?;
        *started = true;
        let readers = records
            .iter()
            .map(|record| record.reader().clone())
            .collect::<HashSet<_>>();
        for reader in readers {
            self.ensure_owner(reader).await?;
        }
        Ok(())
    }

    pub async fn reconcile_reader(&self, reader: Identity) -> Result<(), BoardError> {
        self.require_started().await?;
        self.ensure_owner(reader)
            .await?
            .send(ReaderDeliveryCommand::Reconcile)
            .await
            .map_err(|_| BoardError::board_unavailable())
    }

    /// Internal batch handoff. The public stored-notice conversion belongs to the surface.
    pub async fn wait(
        &self,
        reader: Identity,
        filter: SubscriptionWaitFilter,
        max_wait_seconds: u64,
    ) -> Result<Option<super::SubscriptionWaitResult>, BoardError> {
        self.require_started().await?;
        if max_wait_seconds > 1500 {
            return Err(BoardError::invalid_field(
                "maxWaitSeconds",
                "must be at most 1500 seconds",
            ));
        }
        filter.validate()?;
        let deadline = self
            .inner
            .clock
            .monotonic_now()
            .checked_add(std::time::Duration::from_secs(max_wait_seconds))
            .ok_or_else(BoardError::board_unavailable)?;
        let (reply, response) = oneshot::channel();
        self.ensure_owner(reader)
            .await?
            .send(ReaderDeliveryCommand::Wait {
                filter,
                deadline,
                reply,
            })
            .await
            .map_err(|_| BoardError::board_unavailable())?;
        response
            .await
            .map_err(|_| BoardError::board_unavailable())?
    }

    pub fn cancel(&self) {
        self.inner.shutdown.cancel();
    }
    pub async fn shutdown(&self) {
        self.cancel();
        self.inner.tasks.close();
        self.inner.tasks.wait().await;
        self.inner.owners.lock().await.clear();
    }

    async fn require_started(&self) -> Result<(), BoardError> {
        if !*self.inner.start_gate.lock().await || self.inner.shutdown.is_cancelled() {
            Err(BoardError::board_unavailable())
        } else {
            Ok(())
        }
    }

    async fn ensure_owner(
        &self,
        reader: Identity,
    ) -> Result<mpsc::Sender<ReaderDeliveryCommand>, BoardError> {
        if self.inner.shutdown.is_cancelled() {
            return Err(BoardError::board_unavailable());
        }
        // Keep the map gate through spawn: no Reader can acquire two selection owners.
        let mut owners = self.inner.owners.lock().await;
        if let Some(sender) = owners.get(&reader).filter(|sender| !sender.is_closed()) {
            return Ok(sender.clone());
        }
        let activity = self.inner.store.lock().await.subscribe_activity();
        let (sender, commands) = mpsc::channel(64);
        let owner = ReaderDeliveryOwner::new(ReaderDeliveryOwnerProps {
            reader: reader.clone(),
            store: self.inner.store.clone(),
            push: self.inner.push.clone(),
            presence: self.inner.presence.clone(),
            clock: self.inner.clock.clone(),
            shutdown: self.inner.shutdown.clone(),
            commands,
            activity,
            #[cfg(test)]
            observations: self.inner.observations.clone(),
        });
        owners.insert(reader.clone(), sender.clone());
        let inner = self.inner.clone();
        let owner_sender = sender.clone();
        self.inner.tasks.spawn(async move {
            owner.run().await;
            let mut owners = inner.owners.lock().await;
            if owners
                .get(&reader)
                .is_some_and(|current| current.same_channel(&owner_sender))
            {
                owners.remove(&reader);
            }
        });
        Ok(sender)
    }

    #[cfg(test)]
    pub(super) fn observe_owners(&self) -> tokio::sync::broadcast::Receiver<OwnerObservation> {
        self.inner.observations.subscribe()
    }

    #[cfg(test)]
    pub(super) async fn synchronize_reader(&self, reader: Identity) {
        let (reply, response) = oneshot::channel();
        self.ensure_owner(reader)
            .await
            .expect("active owner")
            .send(ReaderDeliveryCommand::Barrier(reply))
            .await
            .expect("owner command");
        response
            .await
            .expect("owner reached storage-derived sleep boundary");
    }
}

impl Drop for SubscriptionDeliveryService {
    fn drop(&mut self) {
        if Arc::strong_count(&self.lifetime) == 1 {
            self.cancel();
        }
    }
}

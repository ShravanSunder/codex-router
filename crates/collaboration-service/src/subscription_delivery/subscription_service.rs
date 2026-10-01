use super::{
    BoardAvailability, SubscriptionClock,
    reader_delivery_owner::{ReaderDeliveryCommand, ReaderDeliveryOwner, ReaderDeliveryOwnerProps},
    subscription_push::{SubscriptionPushStore, SubscriptionPushStoreProps},
    subscription_wait::SubscriptionWaitFilter,
};
use crate::{MachineIdentity, SessionMessageDelivery, TargetPresenceProbe};
use collaboration_protocol::{PushId, PushRecord, SessionRef};
use message_board::{BoardError, Identity};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

pub struct SubscriptionDeliveryServiceProps {
    pub board_availability: BoardAvailability,
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
    DirectMessageQueued,
    DirectMessageHeld,
    DirectMessageSettled,
    Stopped,
}

struct ServiceInner {
    board_availability: BoardAvailability,
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
    #[must_use]
    pub fn subscription_clock(&self) -> Arc<dyn SubscriptionClock> {
        Arc::clone(&self.inner.clock)
    }

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
                board_availability: props.board_availability,
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
        let records = match &self.inner.board_availability {
            BoardAvailability::Available(store) => {
                store
                    .lock()
                    .await
                    .restore_active_and_draining(self.inner.clock.now())
                    .await?
            }
            BoardAvailability::Unavailable => Vec::new(),
        };
        let dm_targets = self.inner.push.restore_direct_messages().await?;
        *started = true;
        let mut readers = records
            .iter()
            .map(|record| record.reader().clone())
            .collect::<HashSet<_>>();
        for target in dm_targets {
            readers.insert(reader_for_target(&target)?);
        }
        for reader in readers {
            self.ensure_owner(reader).await?;
        }
        Ok(())
    }

    pub async fn reconcile_reader(&self, reader: Identity) -> Result<(), BoardError> {
        self.inner.board_availability.require_store()?;
        self.require_started().await?;
        loop {
            let (reply, response) = oneshot::channel();
            let sender = self.ensure_owner(reader.clone()).await?;
            if sender
                .send(ReaderDeliveryCommand::Reconcile(reply))
                .await
                .is_err()
            {
                self.require_started().await?;
                continue;
            }
            if response.await.is_ok() {
                return Ok(());
            }
            self.require_started().await?;
        }
    }

    /// The record already exists: route its known-unsent work through the target's owner.
    pub async fn deliver_direct_message(
        &self,
        target: SessionRef,
        push_id: PushId,
    ) -> Result<PushRecord, BoardError> {
        self.require_started().await?;
        let reader = reader_for_target(&target)?;
        loop {
            let (reply, response) = oneshot::channel();
            let sender = self.ensure_owner(reader.clone()).await?;
            if sender
                .send(ReaderDeliveryCommand::DmQueued {
                    push_id: push_id.clone(),
                    reply,
                })
                .await
                .is_err()
            {
                continue;
            }
            #[cfg(test)]
            let _ = self
                .inner
                .observations
                .send(OwnerObservation::DirectMessageQueued);
            match response.await {
                Ok(result) => return result,
                Err(_) => {
                    self.require_started().await?;
                }
            }
        }
    }

    /// Internal batch handoff. The public stored-notice conversion belongs to the surface.
    pub async fn wait(
        &self,
        reader: Identity,
        filter: SubscriptionWaitFilter,
        max_wait_seconds: u64,
    ) -> Result<Option<super::SubscriptionWaitResult>, BoardError> {
        self.inner.board_availability.require_store()?;
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
        loop {
            let (reply, response) = oneshot::channel();
            let sender = self.ensure_owner(reader.clone()).await?;
            if sender
                .send(ReaderDeliveryCommand::Wait {
                    filter: filter.clone(),
                    deadline,
                    reply,
                })
                .await
                .is_err()
            {
                self.require_started().await?;
                continue;
            }
            return response
                .await
                .map_err(|_| BoardError::board_unavailable())?;
        }
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
        let activity = match &self.inner.board_availability {
            BoardAvailability::Available(store) => Some(store.lock().await.subscribe_activity()),
            BoardAvailability::Unavailable => None,
        };
        let (sender, commands) = mpsc::channel(64);
        let owner = ReaderDeliveryOwner::new(ReaderDeliveryOwnerProps {
            reader: reader.clone(),
            board_availability: self.inner.board_availability.clone(),
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

fn reader_for_target(target: &SessionRef) -> Result<Identity, BoardError> {
    let session = serde_json::from_value(
        serde_json::to_value(target).map_err(|_| BoardError::board_unavailable())?,
    )
    .map_err(|_| BoardError::board_unavailable())?;
    Ok(Identity::Session { session })
}

#[cfg(test)]
mod reconcile_retry_proofs {
    use super::*;
    use collaboration_protocol::{EndpointId, EndpointRef, SessionId, UuidIdentity};

    #[tokio::test]
    async fn accepted_reconcile_dropped_by_an_idle_owner_is_acknowledged_by_its_replacement()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let directory = tempfile::tempdir()?;
        let board =
            message_board_storage::BoardStore::open(&directory.path().join("board.sqlite")).await?;
        let automation =
            automation_storage::AutomationStore::open(&directory.path().join("automation.sqlite"))
                .await?;
        let service_id = UuidIdentity::try_from("00000000-0000-4000-8000-000000000001".to_owned())?;
        let target = SessionRef {
            endpoint: EndpointRef {
                service_id: service_id.clone(),
                endpoint_id: EndpointId::try_from("codex-local".to_owned())?,
            },
            session_id: SessionId::try_from("closing-reader".to_owned())?,
        };
        let reader = reader_for_target(&target)?;
        let routes = Arc::new(crate::SessionDeliveryRouter::new(Vec::new()));
        let service = SubscriptionDeliveryService::new(SubscriptionDeliveryServiceProps {
            board_availability: BoardAvailability::Available(Arc::new(Mutex::new(board))),
            push_store: Arc::new(Mutex::new(automation)),
            delivery: routes.clone(),
            presence: routes,
            machine_identity: MachineIdentity::new(service_id, Some("closing-owner-proof"))?,
            clock: Arc::new(super::super::SystemSubscriptionClock),
        });
        service.start().await?;
        let mut observations = service.observe_owners();
        // The existing command channel is the race seam: acceptance precedes the
        // stale owner's close, while the response is never acknowledged.
        let (stale_sender, mut stale_receiver) = mpsc::channel(1);
        service
            .inner
            .owners
            .lock()
            .await
            .insert(reader.clone(), stale_sender);
        let reconciler = service.clone();
        let reconcile = tokio::spawn(async move { reconciler.reconcile_reader(reader).await });
        let accepted =
            tokio::time::timeout(std::time::Duration::from_secs(5), stale_receiver.recv())
                .await?
                .ok_or("stale owner received no command")?;
        let ReaderDeliveryCommand::Reconcile(reply) = accepted else {
            return Err("expected a reconcile command".into());
        };
        stale_receiver.close();
        drop(reply);
        drop(stale_receiver);
        tokio::time::timeout(std::time::Duration::from_secs(5), reconcile).await???;
        let started =
            tokio::time::timeout(std::time::Duration::from_secs(5), observations.recv()).await??;
        if !matches!(started, OwnerObservation::Started) {
            return Err("replacement owner did not start".into());
        }
        service.shutdown().await;
        Ok(())
    }
}

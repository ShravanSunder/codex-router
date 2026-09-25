//! Bounded, Host-lifetime FIFO for provider prompts accepted as queued.
use crate::{
    ExternalProviderSupervisor, LiveSessionOwnershipCheck,
    external_provider_supervisor::ProviderPromptDispatch,
    provider_acp_session_loading::{ProviderSessionLoadOutcome, ensure_provider_session_loaded},
};
use collaboration_protocol::{
    ConversationOperationWaitOutput, ConversationOperationWaitRequest, ConversationPromptRequest,
    PositiveSeconds, ProviderOperationStage, SessionRef,
};
use collaboration_service::{ProviderConversationBackend, ProviderOperationStore};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex as StdMutex},
};
use tokio::{
    sync::{Mutex, mpsc},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

const QUEUE_CAPACITY: usize = 128;
const MAX_SESSION_QUEUES: usize = 1024;
const SETTLEMENT_WAIT_SECONDS: u32 = 10;

pub(crate) struct ProviderAcpMessageFifo {
    supervisor: Arc<ExternalProviderSupervisor>,
    store: Arc<Mutex<ProviderOperationStore>>,
    ownership: Arc<dyn LiveSessionOwnershipCheck>,
    senders: StdMutex<HashMap<SessionRef, mpsc::Sender<ConversationPromptRequest>>>,
    workers: StdMutex<Vec<JoinHandle<()>>>,
    shutdown: CancellationToken,
}

impl ProviderAcpMessageFifo {
    pub(crate) fn new(
        supervisor: Arc<ExternalProviderSupervisor>,
        store: Arc<Mutex<ProviderOperationStore>>,
        ownership: Arc<dyn LiveSessionOwnershipCheck>,
    ) -> Self {
        Self {
            supervisor,
            store,
            ownership,
            senders: StdMutex::new(HashMap::new()),
            workers: StdMutex::new(Vec::new()),
            shutdown: CancellationToken::new(),
        }
    }

    pub(crate) fn reserve(
        &self,
        target: &SessionRef,
    ) -> Result<mpsc::OwnedPermit<ConversationPromptRequest>, &'static str> {
        if self.shutdown.is_cancelled() {
            return Err("provider queue is shutting down");
        }
        let mut senders = self
            .senders
            .lock()
            .map_err(|_| "provider queue unavailable")?;
        let sender = if let Some(sender) = senders.get(target).filter(|sender| !sender.is_closed())
        {
            sender.clone()
        } else {
            if senders.len() >= MAX_SESSION_QUEUES {
                return Err("provider session queue capacity exceeded");
            }
            let (sender, receiver) = mpsc::channel(QUEUE_CAPACITY);
            let worker = tokio::spawn(run_provider_message_fifo(
                target.clone(),
                receiver,
                Arc::clone(&self.supervisor),
                Arc::clone(&self.store),
                Arc::clone(&self.ownership),
                self.shutdown.clone(),
            ));
            self.workers
                .lock()
                .map_err(|_| "provider queue workers unavailable")?
                .push(worker);
            senders.insert(target.clone(), sender.clone());
            sender
        };
        sender
            .try_reserve_owned()
            .map_err(|_| "provider session queue is full")
    }

    pub(crate) async fn shutdown(&self) {
        self.shutdown.cancel();
        let workers = match self.workers.lock() {
            Ok(mut workers) => std::mem::take(&mut *workers),
            Err(poisoned) => std::mem::take(&mut *poisoned.into_inner()),
        };
        for worker in workers {
            let _result = worker.await;
        }
    }
}

impl Drop for ProviderAcpMessageFifo {
    fn drop(&mut self) {
        self.shutdown.cancel();
        if let Ok(mut workers) = self.workers.lock() {
            for worker in workers.drain(..) {
                worker.abort();
            }
        }
    }
}

async fn run_provider_message_fifo(
    target: SessionRef,
    mut receiver: mpsc::Receiver<ConversationPromptRequest>,
    supervisor: Arc<ExternalProviderSupervisor>,
    store: Arc<Mutex<ProviderOperationStore>>,
    ownership: Arc<dyn LiveSessionOwnershipCheck>,
    shutdown: CancellationToken,
) {
    loop {
        let request = tokio::select! {
            () = shutdown.cancelled() => return,
            request = receiver.recv() => match request { Some(request) => request, None => return },
        };
        let operation_id = request.operation_id.clone();
        let loaded = tokio::select! {
            () = shutdown.cancelled() => return,
            loaded = ensure_provider_session_loaded(&supervisor, &store, ownership.as_ref(), &target) => loaded,
        };
        if !matches!(loaded, ProviderSessionLoadOutcome::Ready) {
            return;
        }
        let Some(runtime) = supervisor.runtime_for(&target.endpoint) else {
            return;
        };
        let idle = tokio::select! {
            () = shutdown.cancelled() => return,
            idle = runtime.wait_session_idle(String::from(target.session_id.clone())) => idle,
        };
        if idle.is_err() || runtime.retirement().is_cancelled() {
            return;
        }
        let submitted = tokio::select! {
            () = shutdown.cancelled() => return,
            submitted = supervisor.submit_delivery_prompt(request) => submitted,
        };
        if !matches!(submitted, Ok(ProviderPromptDispatch::Submitted)) {
            return;
        }
        let Ok(wait_seconds) = PositiveSeconds::try_from(SETTLEMENT_WAIT_SECONDS) else {
            return;
        };
        loop {
            let settled = tokio::select! {
                () = shutdown.cancelled() => return,
                result = supervisor.wait(ConversationOperationWaitRequest {
                    operation_id: operation_id.clone(), timeout_seconds: wait_seconds,
                }) => result,
            };
            match settled {
                Ok(result)
                    if result.operation.stage == ProviderOperationStage::Terminal
                        && !matches!(result.output, ConversationOperationWaitOutput::Pending) =>
                {
                    break;
                }
                Ok(_) => {}
                Err(_) => return,
            }
        }
    }
}

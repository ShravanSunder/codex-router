//! Bounded, Host-lifetime FIFO for provider prompts accepted as queued.
use crate::{
    ExternalProviderSupervisor, LiveSessionOwnershipCheck,
    external_provider_supervisor::ProviderPromptDispatch,
    provider_acp_session_loading::{ProviderSessionLoadOutcome, ensure_provider_session_loaded},
};
use collaboration_protocol::{
    ConversationOperationFailureKind, ConversationOperationWaitOutput,
    ConversationOperationWaitRequest, ConversationPromptRequest, PositiveSeconds,
    ProviderOperationStage, SessionRef,
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
const MIN_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(50);
const MAX_RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(1);

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
            () = shutdown.cancelled() => {
                mark_remaining_not_submitted(&supervisor, &mut receiver, "Router shutdown before provider submission");
                return;
            },
            request = receiver.recv() => match request {
                Some(request) => request,
                None => return,
            },
        };
        let operation_id = request.operation_id.clone();
        let Some(runtime) = supervisor.runtime_for(&target.endpoint) else {
            drop_current_and_remaining(
                &supervisor,
                &operation_id,
                &mut receiver,
                "provider retired before queued prompt submission",
            );
            return;
        };
        let retirement = runtime.retirement();
        let mut retry_delay = MIN_RETRY_DELAY;
        let mut loaded = false;
        let mut operation_admitted = false;
        loop {
            if shutdown.is_cancelled() {
                drop_current_and_remaining(
                    &supervisor,
                    &operation_id,
                    &mut receiver,
                    "Router shutdown before provider submission",
                );
                return;
            }
            if retirement.is_cancelled() {
                drop_current_and_remaining(
                    &supervisor,
                    &operation_id,
                    &mut receiver,
                    "provider retired before queued prompt submission",
                );
                return;
            }
            if !loaded {
                let load_outcome = tokio::select! {
                    () = shutdown.cancelled() => {
                        drop_current_and_remaining(&supervisor, &operation_id, &mut receiver, "Router shutdown before provider submission");
                        return;
                    },
                    () = retirement.cancelled() => {
                        drop_current_and_remaining(&supervisor, &operation_id, &mut receiver, "provider retired before queued prompt submission");
                        return;
                    },
                    outcome = ensure_provider_session_loaded(&supervisor, &store, ownership.as_ref(), &target) => outcome,
                };
                match load_outcome {
                    ProviderSessionLoadOutcome::Ready => loaded = true,
                    ProviderSessionLoadOutcome::MissingRecord => {
                        supervisor.queued_operation_registry().mark_not_submitted(
                            &operation_id,
                            "provider session record is missing",
                        );
                        break;
                    }
                    ProviderSessionLoadOutcome::LiveElsewhere => {
                        supervisor
                            .queued_operation_registry()
                            .mark_not_submitted(&operation_id, "session is live elsewhere");
                        break;
                    }
                    ProviderSessionLoadOutcome::Unavailable { reason } => {
                        supervisor
                            .queued_operation_registry()
                            .mark_not_submitted(&operation_id, reason);
                        break;
                    }
                }
            }
            let idle = tokio::select! {
                () = shutdown.cancelled() => {
                    drop_current_and_remaining(&supervisor, &operation_id, &mut receiver, "Router shutdown before provider submission");
                    return;
                },
                () = retirement.cancelled() => {
                    drop_current_and_remaining(&supervisor, &operation_id, &mut receiver, "provider retired before queued prompt submission");
                    return;
                },
                idle = runtime.wait_session_idle(String::from(target.session_id.clone())) => idle,
            };
            if idle.is_err() {
                if !wait_before_retry(&shutdown, &retirement, retry_delay).await {
                    drop_current_and_remaining(
                        &supervisor,
                        &operation_id,
                        &mut receiver,
                        "provider retired or Router shut down before queued prompt submission",
                    );
                    return;
                }
                retry_delay = next_retry_delay(retry_delay);
                continue;
            }
            let submitted = tokio::select! {
                () = shutdown.cancelled() => {
                    drop_current_and_remaining(&supervisor, &operation_id, &mut receiver, "Router shutdown before provider submission");
                    return;
                },
                () = retirement.cancelled() => {
                    drop_current_and_remaining(&supervisor, &operation_id, &mut receiver, "provider retired before queued prompt submission");
                    return;
                },
                submitted = supervisor.submit_delivery_prompt(request.clone()) => submitted,
            };
            match submitted {
                Ok(
                    ProviderPromptDispatch::Submitted
                    | ProviderPromptDispatch::NotSubmitted
                    | ProviderPromptDispatch::Existing
                    | ProviderPromptDispatch::Uncertain,
                ) => {
                    // This ID has been admitted. From here on, observe this exact
                    // operation; never submit its logical message under the same ID again.
                    operation_admitted = true;
                    break;
                }
                Err(failure) if failure.kind == ConversationOperationFailureKind::Busy => {}
                Err(failure) => {
                    supervisor
                        .queued_operation_registry()
                        .mark_not_submitted(&operation_id, String::from(failure.message));
                    break;
                }
            }
            if !wait_before_retry(&shutdown, &retirement, retry_delay).await {
                drop_current_and_remaining(
                    &supervisor,
                    &operation_id,
                    &mut receiver,
                    "provider retired or Router shut down before queued prompt submission",
                );
                return;
            }
            retry_delay = next_retry_delay(retry_delay);
        }
        if !operation_admitted {
            continue;
        }
        let Ok(wait_seconds) = PositiveSeconds::try_from(SETTLEMENT_WAIT_SECONDS) else {
            continue;
        };
        loop {
            let settled = tokio::select! {
                () = shutdown.cancelled() => {
                    mark_remaining_not_submitted(&supervisor, &mut receiver, "Router shutdown before queued prompt settlement");
                    return;
                },
                () = retirement.cancelled() => {
                    mark_remaining_not_submitted(&supervisor, &mut receiver, "provider retired before queued prompt settlement");
                    return;
                },
                result = supervisor.wait(ConversationOperationWaitRequest {
                    operation_id: operation_id.clone(), timeout_seconds: wait_seconds,
                }) => result,
            };
            match settled {
                Ok(result)
                    if result.operation.stage == ProviderOperationStage::Terminal
                        && !matches!(result.output, ConversationOperationWaitOutput::Pending) =>
                {
                    supervisor.queued_operation_registry().clear(&operation_id);
                    break;
                }
                Ok(_) => {}
                Err(_failure) => {
                    let terminal_record = store.lock().await.inspect(&operation_id).await;
                    if let Ok(Some(record)) = terminal_record
                        && record.stage == ProviderOperationStage::Terminal
                    {
                        supervisor.queued_operation_registry().clear(&operation_id);
                        break;
                    }
                    if !wait_before_retry(&shutdown, &retirement, MIN_RETRY_DELAY).await {
                        mark_remaining_not_submitted(
                            &supervisor,
                            &mut receiver,
                            "provider retired or Router shut down before queued prompt settlement",
                        );
                        return;
                    }
                }
            }
        }
    }
}

async fn wait_before_retry(
    shutdown: &CancellationToken,
    retirement: &CancellationToken,
    delay: std::time::Duration,
) -> bool {
    tokio::select! {
        () = shutdown.cancelled() => false,
        () = retirement.cancelled() => false,
        () = tokio::time::sleep(delay) => true,
    }
}

fn next_retry_delay(current: std::time::Duration) -> std::time::Duration {
    current.saturating_mul(2).min(MAX_RETRY_DELAY)
}

fn drop_current_and_remaining(
    supervisor: &ExternalProviderSupervisor,
    current: &collaboration_protocol::OperationId,
    receiver: &mut mpsc::Receiver<ConversationPromptRequest>,
    reason: &str,
) {
    supervisor
        .queued_operation_registry()
        .mark_not_submitted(current, reason);
    mark_remaining_not_submitted(supervisor, receiver, reason);
}

fn mark_remaining_not_submitted(
    supervisor: &ExternalProviderSupervisor,
    receiver: &mut mpsc::Receiver<ConversationPromptRequest>,
    reason: &str,
) {
    while let Ok(request) = receiver.try_recv() {
        supervisor
            .queued_operation_registry()
            .mark_not_submitted(&request.operation_id, reason);
    }
}

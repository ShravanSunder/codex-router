//! Ephemeral Listen ownership with bounded admission, monotonic deadlines, and cancellation.
use message_board::*;
use message_board_storage::BoardStore;
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

const MAX_ACTIVE_THREAD_LISTENS: usize = 64;
const MAX_TERMINAL_THREAD_LISTENS: usize = 64;

#[path = "thread_listen_session_delivery.rs"]
mod session_delivery;

struct ThreadListenState {
    context: ThreadListenContext,
    mode: ThreadListenMode,
    delivery: ThreadListenDelivery,
    acknowledge: bool,
    deadline: Instant,
    cancellation: CancellationToken,
    wait_gate: Mutex<()>,
    batches_delivered: AtomicU64,
    first_sequence: AtomicU64,
    last_sequence: AtomicU64,
    catch_up: AtomicBool,
    acknowledged: AtomicBool,
    consecutive_rejections: AtomicU8,
    /// Never held across an await: set and read in one statement.
    last_rejection: std::sync::Mutex<Value>,
    _permit: OwnedSemaphorePermit,
}

struct WaitDropGuard {
    cancellation: CancellationToken,
    completed: bool,
}

impl WaitDropGuard {
    fn complete(&mut self) {
        self.completed = true;
    }
}

impl Drop for WaitDropGuard {
    fn drop(&mut self) {
        if !self.completed {
            self.cancellation.cancel();
        }
    }
}

impl ThreadListenState {
    fn snapshot(&self, listen_id: ListenId) -> ThreadListenSnapshot {
        ThreadListenSnapshot {
            listen_id,
            context: self.context.clone(),
            mode: self.mode.clone(),
            acknowledge: self.acknowledge,
            active: !self.cancellation.is_cancelled() && Instant::now() < self.deadline,
            delivery: self.delivery,
            batches_delivered: self.batches_delivered.load(Ordering::Relaxed),
            first_sequence: nonzero_sequence(self.first_sequence.load(Ordering::Relaxed)),
            last_sequence: nonzero_sequence(self.last_sequence.load(Ordering::Relaxed)),
            catch_up: self.catch_up.load(Ordering::Relaxed),
            acknowledged: self.acknowledged.load(Ordering::Relaxed),
            consecutive_rejections: self.consecutive_rejections.load(Ordering::Relaxed),
            last_rejection: self
                .last_rejection
                .lock()
                .ok()
                .map(|rejection| rejection.clone())
                .filter(|rejection| !rejection.is_null()),
        }
    }
}

#[derive(Clone)]
pub(crate) struct ThreadListenRegistry {
    entries: Arc<Mutex<ThreadListenEntries>>,
    permits: Arc<Semaphore>,
    spawn_lifecycle_cleanup: bool,
}

#[derive(Default)]
struct ThreadListenEntries {
    active: HashMap<ListenId, Arc<ThreadListenState>>,
    terminal: HashMap<ListenId, ThreadListenEndReason>,
    terminal_order: VecDeque<ListenId>,
}

enum ThreadListenEntry {
    Active(Arc<ThreadListenState>),
    Terminal(ThreadListenEndReason),
}

impl ThreadListenRegistry {
    pub(crate) fn new() -> Self {
        Self {
            entries: Arc::new(Mutex::new(ThreadListenEntries::default())),
            permits: Arc::new(Semaphore::new(MAX_ACTIVE_THREAD_LISTENS)),
            spawn_lifecycle_cleanup: true,
        }
    }

    #[cfg(test)]
    fn new_without_lifecycle_cleanup() -> Self {
        Self {
            entries: Arc::new(Mutex::new(ThreadListenEntries::default())),
            permits: Arc::new(Semaphore::new(MAX_ACTIVE_THREAD_LISTENS)),
            spawn_lifecycle_cleanup: false,
        }
    }

    pub(crate) async fn register(
        &self,
        request: &ThreadListenRequest,
        context: ThreadListenContext,
    ) -> Result<ThreadListenSnapshot, BoardError> {
        let lifetime = match request.mode {
            ThreadListenMode::Once { max_wait_seconds } => max_wait_seconds,
            ThreadListenMode::Repeating { lifetime_seconds } => lifetime_seconds,
        };
        if lifetime == 0 {
            return Err(BoardError::invalid_field(
                "mode",
                "Listen duration must be at least one second",
            ));
        }
        let duration = std::time::Duration::from_secs(lifetime);
        let deadline = Instant::now().checked_add(duration).ok_or_else(|| {
            BoardError::invalid_field("mode", "Listen duration exceeds the monotonic clock range")
        })?;
        let permit = Arc::clone(&self.permits)
            .try_acquire_owned()
            .map_err(|_| BoardError::board_unavailable())?;
        let listen_id = ListenId::generate();
        let state = Arc::new(ThreadListenState {
            context,
            mode: request.mode.clone(),
            delivery: request.delivery,
            acknowledge: request.acknowledge,
            deadline,
            cancellation: CancellationToken::new(),
            wait_gate: Mutex::new(()),
            batches_delivered: AtomicU64::new(0),
            first_sequence: AtomicU64::new(0),
            last_sequence: AtomicU64::new(0),
            // Set from the first delivered Batch set, never from --from.
            catch_up: AtomicBool::new(false),
            acknowledged: AtomicBool::new(false),
            consecutive_rejections: AtomicU8::new(0),
            last_rejection: std::sync::Mutex::new(Value::Null),
            _permit: permit,
        });
        let snapshot = state.snapshot(listen_id.clone());
        self.entries
            .lock()
            .await
            .active
            .insert(listen_id.clone(), Arc::clone(&state));
        if self.spawn_lifecycle_cleanup {
            let cleanup_registry = self.clone();
            tokio::spawn(async move {
                tokio::select! {
                    _ = tokio::time::sleep_until(state.deadline) => {
                        cleanup_registry.preserve_terminal(&listen_id, &state).await;
                    }
                    _ = state.cancellation.cancelled() => {
                        cleanup_registry.preserve_terminal(&listen_id, &state).await;
                    }
                }
            });
        }
        Ok(snapshot)
    }

    pub(crate) fn spawn_session_delivery(
        &self,
        listen_id: ListenId,
        store: Arc<Mutex<BoardStore>>,
        sink: Arc<dyn BatchSink>,
    ) {
        let registry = self.clone();
        tokio::spawn(async move {
            session_delivery::run(registry, listen_id, store, sink).await;
        });
    }

    pub(crate) async fn show(
        &self,
        listen_id: &ListenId,
    ) -> Result<ThreadListenSnapshot, BoardError> {
        let state = self.state(listen_id).await?;
        Ok(state.snapshot(listen_id.clone()))
    }

    pub(crate) async fn cancel(
        &self,
        listen_id: &ListenId,
    ) -> Result<ThreadListenSnapshot, BoardError> {
        let state = self.state(listen_id).await?;
        state.cancellation.cancel();
        Ok(state.snapshot(listen_id.clone()))
    }

    pub(crate) async fn wait(
        &self,
        listen_id: &ListenId,
        store: &Arc<Mutex<BoardStore>>,
        maximum_batch_set_bytes: usize,
    ) -> Result<ThreadWaitResult, BoardError> {
        let state = match self.take_entry(listen_id).await? {
            ThreadListenEntry::Active(state) => state,
            ThreadListenEntry::Terminal(reason) => return Ok(end(listen_id, reason)),
        };
        let _wait_guard = state.wait_gate.lock().await;
        let mut drop_guard = WaitDropGuard {
            cancellation: state.cancellation.clone(),
            completed: false,
        };
        let mut activity_receiver = store.lock().await.subscribe_activity();
        let mut poll = tokio::time::interval(THREAD_LISTEN_POLL_INTERVAL);
        poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            if state.cancellation.is_cancelled() {
                self.remove(listen_id).await;
                drop_guard.complete();
                return Ok(end(listen_id, ThreadListenEndReason::Cancelled));
            }
            if Instant::now() >= state.deadline {
                let reason = match state.mode {
                    ThreadListenMode::Once { .. } => ThreadListenEndReason::Timeout,
                    ThreadListenMode::Repeating { .. } => ThreadListenEndReason::Lifetime,
                };
                self.remove(listen_id).await;
                drop_guard.complete();
                return Ok(end(listen_id, reason));
            }
            let latest = match store
                .lock()
                .await
                .thread_listen_latest_activity(&state.context)
                .await
            {
                Ok(latest) => latest,
                Err(error) => {
                    self.remove(listen_id).await;
                    return Err(error);
                }
            };
            if let Some(mut observed_activity) = latest {
                let cap_deadline = Instant::now() + THREAD_LISTEN_DEBOUNCE_CAP;
                let mut quiet_deadline = Instant::now() + THREAD_LISTEN_DEBOUNCE;
                loop {
                    tokio::select! {
                        _ = state.cancellation.cancelled() => {
                            self.remove(listen_id).await;
                            drop_guard.complete();
                            return Ok(end(listen_id, ThreadListenEndReason::Cancelled));
                        }
                        _ = tokio::time::sleep_until(state.deadline) => {
                            break;
                        }
                        _ = tokio::time::sleep_until(quiet_deadline) => break,
                        _ = tokio::time::sleep_until(cap_deadline) => break,
                        notification = activity_receiver.recv() => {
                            if notification.is_err() && !matches!(notification, Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) {
                                continue;
                            }
                        }
                        _ = poll.tick() => {}
                    }
                    let latest = match store
                        .lock()
                        .await
                        .thread_listen_latest_activity(&state.context)
                        .await
                    {
                        Ok(latest) => latest,
                        Err(error) => {
                            self.remove(listen_id).await;
                            return Err(error);
                        }
                    };
                    if let Some(latest_activity) = latest
                        && latest_activity > observed_activity
                    {
                        observed_activity = latest_activity;
                        quiet_deadline = Instant::now() + THREAD_LISTEN_DEBOUNCE;
                    }
                }
                let selection = {
                    let mut store = store.lock().await;
                    match state.delivery {
                        ThreadListenDelivery::Stdout => {
                            store
                                .select_thread_listen_batch_set(
                                    listen_id.clone(),
                                    &state.context,
                                    maximum_batch_set_bytes,
                                )
                                .await
                        }
                        ThreadListenDelivery::Session => {
                            store
                                .select_pending_thread_listen_batch_set(
                                    listen_id.clone(),
                                    &state.context,
                                    maximum_batch_set_bytes,
                                )
                                .await
                        }
                    }
                };
                let batch_set = match selection {
                    Ok(batch_set) => batch_set,
                    Err(error) => {
                        self.remove(listen_id).await;
                        return Err(error);
                    }
                };
                if batch_set.batches.is_empty() {
                    continue;
                }
                if state.delivery == ThreadListenDelivery::Stdout {
                    record_batch_progress(&state, &batch_set);
                }
                let terminal = (state.delivery == ThreadListenDelivery::Stdout
                    && matches!(state.mode, ThreadListenMode::Once { .. }))
                .then(|| ThreadListenEnd {
                    listen_id: listen_id.clone(),
                    reason: ThreadListenEndReason::Emitted,
                });
                if terminal.is_some() {
                    self.remove(listen_id).await;
                }
                drop_guard.complete();
                return Ok(ThreadWaitResult {
                    batch_set: Some(batch_set),
                    end: terminal,
                });
            }

            tokio::select! {
                _ = state.cancellation.cancelled() => {}
                _ = tokio::time::sleep_until(state.deadline) => {}
                _ = poll.tick() => {}
                _ = activity_receiver.recv() => {}
            }
        }
    }

    async fn state(&self, listen_id: &ListenId) -> Result<Arc<ThreadListenState>, BoardError> {
        self.entries
            .lock()
            .await
            .active
            .get(listen_id)
            .cloned()
            .ok_or_else(|| BoardError::invalid_field("listenId", "must identify an active Listen"))
    }

    async fn remove(&self, listen_id: &ListenId) {
        let mut entries = self.entries.lock().await;
        entries.terminal.remove(listen_id);
        if let Some(state) = entries.active.remove(listen_id) {
            state.cancellation.cancel();
        }
    }

    async fn take_entry(&self, listen_id: &ListenId) -> Result<ThreadListenEntry, BoardError> {
        let mut entries = self.entries.lock().await;
        if let Some(reason) = entries.terminal.remove(listen_id) {
            return Ok(ThreadListenEntry::Terminal(reason));
        }
        entries
            .active
            .get(listen_id)
            .cloned()
            .map(ThreadListenEntry::Active)
            .ok_or_else(|| BoardError::invalid_field("listenId", "must identify an active Listen"))
    }

    async fn preserve_terminal(&self, listen_id: &ListenId, state: &Arc<ThreadListenState>) {
        let mut entries = self.entries.lock().await;
        let is_current = entries
            .active
            .get(listen_id)
            .is_some_and(|current| Arc::ptr_eq(current, state));
        if !is_current {
            return;
        }
        entries.active.remove(listen_id);
        let reason = terminal_reason(state);
        entries.terminal.insert(listen_id.clone(), reason);
        entries.terminal_order.push_back(listen_id.clone());
        while entries.terminal_order.len() > MAX_TERMINAL_THREAD_LISTENS {
            if let Some(expired) = entries.terminal_order.pop_front() {
                entries.terminal.remove(&expired);
            }
        }
    }
}

fn nonzero_sequence(value: u64) -> Option<ActivitySequence> {
    (value != 0)
        .then(|| ActivitySequence::try_from(value).ok())
        .flatten()
}

/// Makes non-terminal rejections observable through `listen show` and every finalization.
fn publish_rejection_state(state: &ThreadListenState, consecutive: u8, last: &Value) {
    state
        .consecutive_rejections
        .store(consecutive, Ordering::Relaxed);
    if let Ok(mut stored) = state.last_rejection.lock() {
        *stored = last.clone();
    }
}

fn record_batch_progress(state: &ThreadListenState, batch_set: &ThreadListenBatchSet) {
    if state.batches_delivered.load(Ordering::Relaxed) == 0 {
        state.catch_up.store(batch_set.catch_up, Ordering::Relaxed);
    }
    let first = batch_set
        .batches
        .iter()
        .flat_map(|batch| batch.messages.iter())
        .map(|message| message.activity_sequence.get())
        .min();
    let last = batch_set
        .batches
        .iter()
        .map(|batch| batch.delivered_through.get())
        .max();
    if let Some(first) = first {
        let _ =
            state
                .first_sequence
                .compare_exchange(0, first, Ordering::Relaxed, Ordering::Relaxed);
    }
    if let Some(last) = last {
        state.last_sequence.store(last, Ordering::Relaxed);
    }
    state.batches_delivered.fetch_add(1, Ordering::Relaxed);
}

fn record_rejection(
    consecutive_rejections: &mut u8,
    last_rejection: &mut Value,
    evidence: Value,
) -> bool {
    *consecutive_rejections = consecutive_rejections.saturating_add(1);
    *last_rejection = evidence;
    *consecutive_rejections >= 3
}

fn terminal_reason(state: &ThreadListenState) -> ThreadListenEndReason {
    if state.cancellation.is_cancelled() {
        ThreadListenEndReason::Cancelled
    } else {
        match state.mode {
            ThreadListenMode::Once { .. } => ThreadListenEndReason::Timeout,
            ThreadListenMode::Repeating { .. } => ThreadListenEndReason::Lifetime,
        }
    }
}

fn end(listen_id: &ListenId, reason: ThreadListenEndReason) -> ThreadWaitResult {
    ThreadWaitResult {
        batch_set: None,
        end: Some(ThreadListenEnd {
            listen_id: listen_id.clone(),
            reason,
        }),
    }
}

#[cfg(test)]
#[path = "thread_listen_registry_tests.rs"]
mod tests;

use super::*;

/// Tracks active local WebSocket streams by local token generation.
const MAX_WEBSOCKET_REGISTRY_SAMPLE_COUNTS: usize = 1024;
const MAX_CAPACITY_RETRY_SESSION_IDENTITIES: usize = 1024;

/// Tracks active local WebSocket streams by local token generation.
#[derive(Clone, Debug, Default)]
pub struct WebSocketRevocationRegistry {
    #[cfg(test)]
    connections: Arc<Mutex<HashMap<TokenGeneration, Vec<TcpStream>>>>,
    cancellations: Arc<Mutex<HashMap<TokenGeneration, Vec<WebSocketCancellationEntry>>>>,
    stats: Arc<Mutex<WebSocketRegistryStats>>,
    registered_session_ids: Arc<Mutex<Vec<u64>>>,
    completed_session_ids: Arc<Mutex<Vec<u64>>>,
    closed_session_ids: Arc<Mutex<Vec<u64>>>,
    session_peer_addrs: Arc<Mutex<Vec<WebSocketSessionPeerAddr>>>,
    forwarded_upstream_messages_by_session: Arc<Mutex<HashMap<u64, usize>>>,
    completed_session_forwarded_upstream_message_counts: Arc<Mutex<Vec<usize>>>,
    final_session_forwarded_upstream_message_counts: Arc<Mutex<Vec<usize>>>,
    quota_reconnect_signal_count: Arc<Mutex<usize>>,
    quota_reconnect_signal_unix_ms: Arc<Mutex<Option<u128>>>,
    capacity_retry_tracker: CapacityRetryTracker,
    capacity_retry_thread_ids: Arc<Mutex<HashMap<u64, String>>>,
}

/// Narrow runtime handle for reconnecting sessions pinned to a floor-blocked account.
#[derive(Clone, Debug)]
pub struct WebSocketQuotaFloorNotifier {
    registry: WebSocketRevocationRegistry,
}

impl WebSocketQuotaFloorNotifier {
    pub(crate) const fn new(registry: WebSocketRevocationRegistry) -> Self {
        Self { registry }
    }

    /// Sends the existing Codex reconnect signal to sessions pinned to this account.
    pub fn signal_weekly_quota_floor_reached(&self, account_id: &AccountId) {
        self.registry.signal_weekly_quota_floor_reached(account_id);
    }

    /// Defers an early switch until a safe turn boundary and a live peer assessment.
    pub fn request_weekly_quota_floor_switch(&self, account_id: &AccountId) {
        self.registry
            .update_weekly_quota_floor_switch(account_id, true);
    }

    /// Clears a previously observed early-switch intent after a saved recovery observation.
    pub fn clear_weekly_quota_floor_switch(&self, account_id: &AccountId) {
        self.registry
            .update_weekly_quota_floor_switch(account_id, false);
    }
}

#[derive(Clone, Debug)]
struct WebSocketCancellationEntry {
    session_id: u64,
    account_id: AccountId,
    token: CancellationToken,
    quota_floor_reconnect: CancellationToken,
    graceful_floor_switch: watch::Sender<FloorSwitchIntent>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct WebSocketRegistryStats {
    next_session_id: u64,
    active_sessions: usize,
    high_water_sessions: usize,
    registered_sessions: usize,
    closed_sessions: usize,
    completed_response_sessions: usize,
    forwarded_upstream_messages: usize,
}

/// Redacted local peer socket observed for a WebSocket session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WebSocketSessionPeerAddr {
    /// Redacted numeric router-local session id.
    pub session_id: u64,
    /// Loopback client socket address as observed by the router.
    pub peer_addr: String,
}

/// Redacted snapshot of WebSocket session registry counters.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WebSocketRegistrySnapshot {
    /// Currently active async WebSocket sessions.
    pub active_sessions: usize,
    /// Highest active async WebSocket session count observed.
    pub high_water_sessions: usize,
    /// Total async WebSocket sessions registered since router start.
    pub registered_sessions: usize,
    /// Total async WebSocket sessions that have dropped their registry handle.
    pub closed_sessions: usize,
    /// Total response.completed events forwarded by async WebSocket sessions.
    pub completed_response_sessions: usize,
    /// Total upstream-to-local WebSocket messages written to local clients.
    pub forwarded_upstream_messages: usize,
    /// Redacted numeric session ids opened by the router registry.
    pub registered_session_ids: Vec<u64>,
    /// Redacted numeric session ids that forwarded response.completed.
    pub completed_session_ids: Vec<u64>,
    /// Redacted numeric session ids closed by the router registry.
    pub closed_session_ids: Vec<u64>,
    /// Redacted local peer socket addresses associated with opened sessions.
    pub session_peer_addrs: Vec<WebSocketSessionPeerAddr>,
    /// Upstream-to-local write counts captured when each response.completed event is observed.
    pub completed_session_forwarded_upstream_message_counts: Vec<usize>,
    /// Final upstream-to-local write counts captured once per closed async WebSocket session.
    pub final_session_forwarded_upstream_message_counts: Vec<usize>,
    /// Number of router-owned quota reconnect signals emitted.
    pub quota_reconnect_signal_count: usize,
    /// First router-owned quota reconnect signal timestamp in Unix milliseconds.
    pub quota_reconnect_signal_unix_ms: Option<u128>,
}

#[derive(Debug)]
pub(super) struct WebSocketSessionRegistration {
    pub(super) registry: WebSocketRevocationRegistry,
    pub(super) generation: TokenGeneration,
    pub(super) session_id: u64,
    pub(super) cancellation: CancellationToken,
    pub(super) quota_floor_reconnect: CancellationToken,
    pub(super) graceful_floor_switch: watch::Receiver<FloorSwitchIntent>,
    pub(super) early_floor_reconnect: CancellationToken,
}

impl WebSocketRevocationRegistry {
    /// Creates an empty revocation registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub(super) fn set_capacity_retry_thread_id(&self, session_id: u64, thread_id: Option<String>) {
        if let Some(thread_id) = thread_id
            && let Ok(mut thread_ids) = self.capacity_retry_thread_ids.lock()
            && thread_ids.len() < MAX_CAPACITY_RETRY_SESSION_IDENTITIES
        {
            thread_ids.insert(session_id, thread_id);
        }
    }

    pub(super) fn record_capacity_retry(&self, session_id: u64) -> Option<CapacityRetryOutcome> {
        let thread_id = self
            .capacity_retry_thread_ids
            .lock()
            .ok()
            .and_then(|thread_ids| thread_ids.get(&session_id).cloned())?;
        Some(self.capacity_retry_tracker.record_or_exhaust(&thread_id))
    }

    pub(super) fn clear_capacity_retry(&self, session_id: u64) {
        let thread_id = self
            .capacity_retry_thread_ids
            .lock()
            .ok()
            .and_then(|thread_ids| thread_ids.get(&session_id).cloned());
        if let Some(thread_id) = thread_id {
            self.capacity_retry_tracker.clear(&thread_id);
        }
    }

    #[cfg(test)]
    pub(super) fn register(
        &self,
        generation: TokenGeneration,
        stream: &TcpStream,
    ) -> Result<(), WebSocketTunnelError> {
        let stream = stream
            .try_clone()
            .map_err(WebSocketTunnelError::ConnectionTracking)?;
        if let Ok(mut connections) = self.connections.lock() {
            connections.entry(generation).or_default().push(stream);
        }

        Ok(())
    }

    #[cfg(test)]
    pub(super) fn register_cancellation(
        &self,
        generation: TokenGeneration,
    ) -> WebSocketSessionRegistration {
        self.register_cancellation_with_peer_addr(
            generation,
            AccountId::new("acct_registry_test").unwrap_or_else(|error| panic!("{error}")),
            None,
        )
    }

    pub(super) fn register_cancellation_with_peer_addr(
        &self,
        generation: TokenGeneration,
        account_id: AccountId,
        peer_addr: Option<SocketAddr>,
    ) -> WebSocketSessionRegistration {
        let cancellation = CancellationToken::new();
        let quota_floor_reconnect = CancellationToken::new();
        let (graceful_floor_switch, graceful_floor_switch_receiver) =
            watch::channel(FloorSwitchIntent::default());
        let early_floor_reconnect = CancellationToken::new();
        let session_id = self.note_session_opened();
        if let Some(peer_addr) = peer_addr
            && let Ok(mut session_peer_addrs) = self.session_peer_addrs.lock()
        {
            push_bounded_peer_addr(
                &mut session_peer_addrs,
                WebSocketSessionPeerAddr {
                    session_id,
                    peer_addr: peer_addr.to_string(),
                },
            );
        }
        if let Ok(mut cancellations) = self.cancellations.lock() {
            cancellations
                .entry(generation)
                .or_default()
                .push(WebSocketCancellationEntry {
                    session_id,
                    account_id,
                    token: cancellation.clone(),
                    quota_floor_reconnect: quota_floor_reconnect.clone(),
                    graceful_floor_switch,
                });
        }

        WebSocketSessionRegistration {
            registry: self.clone(),
            generation,
            session_id,
            cancellation,
            quota_floor_reconnect,
            graceful_floor_switch: graceful_floor_switch_receiver,
            early_floor_reconnect,
        }
    }

    /// Asks active sessions pinned to an account to reconnect after it reaches its quota floor.
    pub fn signal_weekly_quota_floor_reached(&self, account_id: &AccountId) {
        let Ok(cancellations) = self.cancellations.lock() else {
            return;
        };
        for entry in cancellations
            .values()
            .flatten()
            .filter(|entry| &entry.account_id == account_id)
        {
            entry.quota_floor_reconnect.cancel();
        }
    }

    fn update_weekly_quota_floor_switch(&self, account_id: &AccountId, pending: bool) {
        let Ok(cancellations) = self.cancellations.lock() else {
            return;
        };
        for entry in cancellations
            .values()
            .flatten()
            .filter(|entry| &entry.account_id == account_id)
        {
            entry.graceful_floor_switch.send_modify(|intent| {
                intent.epoch = intent.epoch.saturating_add(1);
                intent.pending = pending;
            });
        }
    }

    /// Returns redacted active-session registry counters.
    #[must_use]
    pub fn snapshot(&self) -> WebSocketRegistrySnapshot {
        self.stats.lock().map_or_else(
            |_| WebSocketRegistrySnapshot::default(),
            |stats| WebSocketRegistrySnapshot {
                active_sessions: stats.active_sessions,
                high_water_sessions: stats.high_water_sessions,
                registered_sessions: stats.registered_sessions,
                closed_sessions: stats.closed_sessions,
                completed_response_sessions: stats.completed_response_sessions,
                forwarded_upstream_messages: stats.forwarded_upstream_messages,
                registered_session_ids: self
                    .registered_session_ids
                    .lock()
                    .map_or_else(|_| Vec::new(), |ids| ids.clone()),
                completed_session_ids: self
                    .completed_session_ids
                    .lock()
                    .map_or_else(|_| Vec::new(), |ids| ids.clone()),
                closed_session_ids: self
                    .closed_session_ids
                    .lock()
                    .map_or_else(|_| Vec::new(), |ids| ids.clone()),
                session_peer_addrs: self
                    .session_peer_addrs
                    .lock()
                    .map_or_else(|_| Vec::new(), |peers| peers.clone()),
                completed_session_forwarded_upstream_message_counts: self
                    .completed_session_forwarded_upstream_message_counts
                    .lock()
                    .map_or_else(|_| Vec::new(), |counts| counts.clone()),
                final_session_forwarded_upstream_message_counts: self
                    .final_session_forwarded_upstream_message_counts
                    .lock()
                    .map_or_else(|_| Vec::new(), |counts| counts.clone()),
                quota_reconnect_signal_count: self
                    .quota_reconnect_signal_count
                    .lock()
                    .map_or(0, |count| *count),
                quota_reconnect_signal_unix_ms: self
                    .quota_reconnect_signal_unix_ms
                    .lock()
                    .map_or(None, |timestamp| *timestamp),
            },
        )
    }

    fn note_session_opened(&self) -> u64 {
        let Ok(mut stats) = self.stats.lock() else {
            return 0;
        };
        stats.next_session_id = stats.next_session_id.saturating_add(1);
        stats.active_sessions = stats.active_sessions.saturating_add(1);
        stats.registered_sessions = stats.registered_sessions.saturating_add(1);
        stats.high_water_sessions = stats.high_water_sessions.max(stats.active_sessions);
        if let Ok(mut registered_session_ids) = self.registered_session_ids.lock() {
            push_bounded_u64(&mut registered_session_ids, stats.next_session_id);
        }
        stats.next_session_id
    }

    fn note_session_closed(&self, generation: TokenGeneration, session_id: u64) {
        if let Ok(mut cancellations) = self.cancellations.lock()
            && let Some(entries) = cancellations.get_mut(&generation)
        {
            entries.retain(|entry| entry.session_id != session_id);
            if entries.is_empty() {
                cancellations.remove(&generation);
            }
        }
        if let Ok(mut stats) = self.stats.lock() {
            stats.active_sessions = stats.active_sessions.saturating_sub(1);
            stats.closed_sessions = stats.closed_sessions.saturating_add(1);
        }
        if let Ok(mut closed_session_ids) = self.closed_session_ids.lock() {
            push_bounded_u64(&mut closed_session_ids, session_id);
        }
        let forwarded_count = self
            .forwarded_upstream_messages_by_session
            .lock()
            .ok()
            .and_then(|forwarded_by_session| forwarded_by_session.get(&session_id).copied())
            .unwrap_or_default();
        if let Ok(mut counts) = self.final_session_forwarded_upstream_message_counts.lock() {
            push_bounded_count(&mut counts, forwarded_count);
        }
        if let Ok(mut forwarded_by_session) = self.forwarded_upstream_messages_by_session.lock() {
            forwarded_by_session.remove(&session_id);
        }
        if let Ok(mut thread_ids) = self.capacity_retry_thread_ids.lock() {
            thread_ids.remove(&session_id);
        }
    }

    pub(super) fn note_upstream_message_forwarded(&self, session_id: u64) {
        if let Ok(mut stats) = self.stats.lock() {
            stats.forwarded_upstream_messages = stats.forwarded_upstream_messages.saturating_add(1);
        }
        if let Ok(mut forwarded_by_session) = self.forwarded_upstream_messages_by_session.lock() {
            let count = forwarded_by_session.entry(session_id).or_default();
            *count = count.saturating_add(1);
        }
    }

    pub(super) fn note_response_completed(&self, session_id: u64) {
        if let Ok(mut stats) = self.stats.lock() {
            stats.completed_response_sessions = stats.completed_response_sessions.saturating_add(1);
        }
        if let Ok(mut completed_session_ids) = self.completed_session_ids.lock() {
            push_bounded_u64(&mut completed_session_ids, session_id);
        }
        let forwarded_count = self
            .forwarded_upstream_messages_by_session
            .lock()
            .ok()
            .and_then(|forwarded_by_session| forwarded_by_session.get(&session_id).copied())
            .unwrap_or_default();
        if let Ok(mut counts) = self
            .completed_session_forwarded_upstream_message_counts
            .lock()
        {
            push_bounded_count(&mut counts, forwarded_count);
        }
    }

    pub(super) fn note_quota_reconnect_signal(&self) {
        if let Ok(mut count) = self.quota_reconnect_signal_count.lock() {
            *count = count.saturating_add(1);
        }
        if let Ok(mut timestamp) = self.quota_reconnect_signal_unix_ms.lock()
            && timestamp.is_none()
        {
            *timestamp = Some(current_unix_millis());
        }
    }

    /// Closes connections that authenticated with generations other than the active one.
    pub fn close_all_except(&self, active_generation: TokenGeneration) {
        #[cfg(test)]
        {
            let Ok(mut connections) = self.connections.lock() else {
                return;
            };
            let stale_generations = connections
                .keys()
                .copied()
                .filter(|generation| *generation != active_generation)
                .collect::<Vec<_>>();
            for stale_generation in stale_generations {
                if let Some(streams) = connections.remove(&stale_generation) {
                    for stream in streams {
                        let _result = stream.shutdown(Shutdown::Both);
                    }
                }
            }
        }
        let Ok(mut cancellations) = self.cancellations.lock() else {
            return;
        };
        let stale_generations = cancellations
            .keys()
            .copied()
            .filter(|generation| *generation != active_generation)
            .collect::<Vec<_>>();
        for stale_generation in stale_generations {
            if let Some(entries) = cancellations.remove(&stale_generation) {
                for entry in entries {
                    entry.token.cancel();
                }
            }
        }
    }
}

fn push_bounded_count(counts: &mut Vec<usize>, count: usize) {
    counts.push(count);
    if counts.len() > MAX_WEBSOCKET_REGISTRY_SAMPLE_COUNTS {
        let excess = counts.len() - MAX_WEBSOCKET_REGISTRY_SAMPLE_COUNTS;
        counts.drain(0..excess);
    }
}

fn push_bounded_u64(values: &mut Vec<u64>, value: u64) {
    values.push(value);
    if values.len() > MAX_WEBSOCKET_REGISTRY_SAMPLE_COUNTS {
        let excess = values.len() - MAX_WEBSOCKET_REGISTRY_SAMPLE_COUNTS;
        values.drain(0..excess);
    }
}

fn push_bounded_peer_addr(
    values: &mut Vec<WebSocketSessionPeerAddr>,
    value: WebSocketSessionPeerAddr,
) {
    values.push(value);
    if values.len() > MAX_WEBSOCKET_REGISTRY_SAMPLE_COUNTS {
        let excess = values.len() - MAX_WEBSOCKET_REGISTRY_SAMPLE_COUNTS;
        values.drain(0..excess);
    }
}
impl WebSocketSessionRegistration {
    pub(super) fn cancellation(&self) -> &CancellationToken {
        &self.cancellation
    }

    #[cfg(test)]
    pub(super) fn note_response_completed(&self) {
        self.registry.note_response_completed(self.session_id);
    }
}

impl Drop for WebSocketSessionRegistration {
    fn drop(&mut self) {
        self.registry
            .note_session_closed(self.generation, self.session_id);
    }
}

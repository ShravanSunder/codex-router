//! In-memory event order and attach replay for Router-owned provider Sessions.

use std::{collections::BTreeMap, sync::Arc};

use message_board::{SessionEndpointRef, SessionRef};
use session_event_model::{
    PendingInteraction, PendingInteractions, SessionEvent, SessionSettings, SessionState,
    TurnLostReason, TurnOutcome,
};
use tokio::sync::{Mutex, broadcast};

use crate::{
    HubEvent, HubFuture, HubSessionSummary, ProviderOperationStore, SessionEventAttachment,
    SessionEventHub, SessionEventHubError,
};

pub struct ProviderSessionEventHub {
    store: Arc<Mutex<ProviderOperationStore>>,
    histories: Mutex<BTreeMap<SessionRef, Arc<Mutex<SessionHistory>>>>,
    subscriber_capacity: usize,
}

struct SessionHistory {
    replay_epoch: u64,
    next_sequence: u64,
    events: BTreeMap<u64, HubEvent>,
    items: BTreeMap<String, RetainedItem>,
    sender: broadcast::Sender<HubEvent>,
    subscriber_capacity: usize,
    state: SessionState,
    settings: Option<SessionSettings>,
    pending: BTreeMap<String, PendingInteraction>,
    turn_running: bool,
}

#[derive(Default)]
struct RetainedItem {
    latest: Option<HubEvent>,
    completion: Option<HubEvent>,
}

#[derive(Debug)]
enum HubProjectionDiagnostic {
    PendingAtTurnEnd,
    DuplicateInteraction,
    UnknownInteractionResolution,
    PendingStateConflict,
    EmptyRequiresAction,
    UnexpectedResyncEvent,
}

impl SessionHistory {
    fn new(subscriber_capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(subscriber_capacity);
        Self {
            replay_epoch: 0,
            next_sequence: 1,
            events: BTreeMap::new(),
            items: BTreeMap::new(),
            sender,
            subscriber_capacity,
            state: SessionState::Unloaded,
            settings: None,
            pending: BTreeMap::new(),
            turn_running: false,
        }
    }

    fn snapshot(&self) -> Vec<HubEvent> {
        let mut snapshot = self.events.values().cloned().collect::<Vec<_>>();
        for retained in self.items.values() {
            snapshot.extend(retained.latest.iter().cloned());
            snapshot.extend(retained.completion.iter().cloned());
        }
        snapshot.sort_by_key(|event| event.sequence);
        snapshot
    }

    fn project(&mut self, event: &SessionEvent) -> Result<(), HubProjectionDiagnostic> {
        match event {
            SessionEvent::TurnStarted { .. } => {
                self.turn_running = true;
                self.state = if self.pending.is_empty() {
                    SessionState::Running
                } else {
                    self.requires_action_state()?
                };
            }
            SessionEvent::TurnEnded { outcome, .. } => {
                if !self.pending.is_empty() {
                    return Err(HubProjectionDiagnostic::PendingAtTurnEnd);
                }
                self.turn_running = false;
                self.state = match outcome {
                    TurnOutcome::Ended { .. } => SessionState::Idle,
                    TurnOutcome::Lost {
                        reason: TurnLostReason::EndNotObservable,
                    } => SessionState::Idle,
                    TurnOutcome::Lost { .. } => SessionState::Unloaded,
                };
            }
            SessionEvent::InteractionRequested { interaction } => {
                if self.pending.contains_key(interaction.request_id()) {
                    return Err(HubProjectionDiagnostic::DuplicateInteraction);
                }
                self.pending
                    .insert(interaction.request_id().to_owned(), interaction.clone());
                self.state = self.requires_action_state()?;
            }
            SessionEvent::InteractionResolved { request_id } => {
                if self.pending.remove(request_id).is_none() {
                    return Err(HubProjectionDiagnostic::UnknownInteractionResolution);
                }
                self.state = if self.pending.is_empty() {
                    if self.turn_running {
                        SessionState::Running
                    } else {
                        SessionState::Idle
                    }
                } else {
                    self.requires_action_state()?
                };
            }
            SessionEvent::StateChanged { state } => {
                if let SessionState::RequiresAction { pending } = state {
                    let advertised: BTreeMap<String, PendingInteraction> = pending
                        .iter()
                        .map(|interaction| {
                            (interaction.request_id().to_owned(), interaction.clone())
                        })
                        .collect();
                    if !self.pending.is_empty() && self.pending != advertised {
                        return Err(HubProjectionDiagnostic::PendingStateConflict);
                    }
                    self.pending = advertised;
                } else if !self.pending.is_empty() {
                    return Err(HubProjectionDiagnostic::PendingStateConflict);
                }
                self.state = state.clone();
                match state {
                    SessionState::Running => self.turn_running = true,
                    SessionState::RequiresAction { .. } => {}
                    _ => self.turn_running = false,
                }
            }
            SessionEvent::InputAccepted { .. }
            | SessionEvent::ItemStarted { .. }
            | SessionEvent::ItemUpdated { .. }
            | SessionEvent::ItemCompleted { .. }
            | SessionEvent::CapabilitiesChanged { .. } => {}
            SessionEvent::SettingsChanged { settings } => {
                self.settings = Some(settings.clone());
            }
            SessionEvent::ResyncRequired { .. } => {
                return Err(HubProjectionDiagnostic::UnexpectedResyncEvent);
            }
        }
        Ok(())
    }

    fn requires_action_state(&self) -> Result<SessionState, HubProjectionDiagnostic> {
        let pending = PendingInteractions::new(self.pending.values().cloned().collect())
            .ok_or(HubProjectionDiagnostic::EmptyRequiresAction)?;
        Ok(SessionState::RequiresAction { pending })
    }

    fn reset_after_projection_rejection(&mut self) -> HubEvent {
        let replay_epoch = self.replay_epoch.wrapping_add(1);
        let control = HubEvent {
            sequence: self.next_sequence,
            event: SessionEvent::ResyncRequired { replay_epoch },
        };
        let _ = self.sender.send(control.clone());
        let (sender, _) = broadcast::channel(self.subscriber_capacity);
        self.sender = sender;
        self.events.clear();
        self.items.clear();
        self.pending.clear();
        self.turn_running = false;
        self.state = SessionState::Unloaded;
        self.settings = None;
        self.next_sequence = 1;
        self.replay_epoch = replay_epoch;
        control
    }
}

impl ProviderSessionEventHub {
    #[must_use]
    pub fn new(store: Arc<Mutex<ProviderOperationStore>>) -> Self {
        Self::with_capacity(store, 256)
    }

    #[must_use]
    pub fn with_capacity(
        store: Arc<Mutex<ProviderOperationStore>>,
        subscriber_capacity: usize,
    ) -> Self {
        Self {
            store,
            histories: Mutex::new(BTreeMap::new()),
            subscriber_capacity: subscriber_capacity.max(1),
        }
    }

    /// Publishing and attach share the lock for this Session only.
    pub async fn publish(
        &self,
        session: SessionRef,
        event: SessionEvent,
    ) -> Result<HubEvent, SessionEventHubError> {
        let history = self
            .histories
            .lock()
            .await
            .entry(session.clone())
            .or_insert_with(|| Arc::new(Mutex::new(SessionHistory::new(self.subscriber_capacity))))
            .clone();
        let mut history = history.lock().await;
        let sequence = history.next_sequence;
        let next_sequence = sequence
            .checked_add(1)
            .ok_or(SessionEventHubError::Unavailable)?;
        if let Err(projection_failure) = history.project(&event) {
            tracing::warn!(
                ?session,
                ?projection_failure,
                "provider Session hub projection rejected; Session history reset"
            );
            return Ok(history.reset_after_projection_rejection());
        }
        history.next_sequence = next_sequence;
        let item = HubEvent { sequence, event };
        match &item.event {
            SessionEvent::ItemStarted { item: snapshot }
            | SessionEvent::ItemUpdated { item: snapshot } => {
                history
                    .items
                    .entry(snapshot.item_id.clone())
                    .or_default()
                    .latest = Some(item.clone());
            }
            SessionEvent::ItemCompleted { item_id } => {
                history.items.entry(item_id.clone()).or_default().completion = Some(item.clone());
            }
            _ => {
                history.events.insert(sequence, item.clone());
            }
        }
        let _ = history.sender.send(item.clone());
        if matches!(
            &item.event,
            SessionEvent::StateChanged {
                state: SessionState::Unloaded | SessionState::Closed
            }
        ) || matches!(&item.event, SessionEvent::TurnEnded { .. })
            && history.state == SessionState::Unloaded
        {
            let replay_epoch = history
                .replay_epoch
                .checked_add(1)
                .ok_or(SessionEventHubError::Unavailable)?;
            let _ = history.sender.send(HubEvent {
                sequence: history.next_sequence,
                event: SessionEvent::ResyncRequired { replay_epoch },
            });
            let (sender, _) = broadcast::channel(self.subscriber_capacity);
            history.sender = sender;
            history.events.clear();
            history.items.clear();
            history.next_sequence = 1;
            history.replay_epoch = replay_epoch;
        }
        Ok(item)
    }

    /// Start a fresh agent history replay after provider loss. Old subscribers
    /// receive a control event and must attach again to the new epoch.
    pub async fn begin_history_replay(
        &self,
        session: SessionRef,
    ) -> Result<u64, SessionEventHubError> {
        let history = self
            .histories
            .lock()
            .await
            .entry(session)
            .or_insert_with(|| Arc::new(Mutex::new(SessionHistory::new(self.subscriber_capacity))))
            .clone();
        let mut history = history.lock().await;
        if history.state != SessionState::Unloaded {
            return Err(SessionEventHubError::Unavailable);
        }
        let replay_epoch = history
            .replay_epoch
            .checked_add(1)
            .ok_or(SessionEventHubError::Unavailable)?;
        let _ = history.sender.send(HubEvent {
            sequence: history.next_sequence,
            event: SessionEvent::ResyncRequired { replay_epoch },
        });
        let (sender, _) = broadcast::channel(self.subscriber_capacity);
        history.sender = sender;
        history.events.clear();
        history.items.clear();
        history.pending.clear();
        history.turn_running = false;
        history.state = SessionState::Unloaded;
        history.settings = None;
        history.next_sequence = 1;
        history.replay_epoch = replay_epoch;
        Ok(replay_epoch)
    }

    /// A resumed Session has no replayable history. Invalidate retained events
    /// and subscribers while keeping the newly connected Session idle.
    pub async fn begin_history_unavailable(
        &self,
        session: SessionRef,
    ) -> Result<u64, SessionEventHubError> {
        let history = self
            .histories
            .lock()
            .await
            .entry(session)
            .or_insert_with(|| Arc::new(Mutex::new(SessionHistory::new(self.subscriber_capacity))))
            .clone();
        let mut history = history.lock().await;
        let replay_epoch = history
            .replay_epoch
            .checked_add(1)
            .ok_or(SessionEventHubError::Unavailable)?;
        let _ = history.sender.send(HubEvent {
            sequence: history.next_sequence,
            event: SessionEvent::ResyncRequired { replay_epoch },
        });
        // Resume has no transcript replay, but the client publishes its fresh
        // current settings before this Host invalidation. Carry that state
        // event into the new epoch ahead of capabilities and Idle.
        let current_settings = history
            .events
            .values()
            .rev()
            .find_map(|item| match &item.event {
                SessionEvent::SettingsChanged { settings } => Some(settings.clone()),
                _ => None,
            });
        let (sender, _) = broadcast::channel(self.subscriber_capacity);
        history.sender = sender;
        history.events.clear();
        history.items.clear();
        history.pending.clear();
        history.turn_running = false;
        history.state = SessionState::Idle;
        history.next_sequence = 1;
        if let Some(settings) = current_settings {
            history.events.insert(
                1,
                HubEvent {
                    sequence: 1,
                    event: SessionEvent::SettingsChanged { settings },
                },
            );
            history.next_sequence = 2;
        }
        history.replay_epoch = replay_epoch;
        Ok(replay_epoch)
    }
}

impl SessionEventHub for ProviderSessionEventHub {
    fn attach(&self, session: SessionRef) -> HubFuture<'_, SessionEventAttachment> {
        Box::pin(async move {
            let already_loaded = self.histories.lock().await.contains_key(&session);
            let stored = if already_loaded {
                true
            } else {
                self.persisted_session_exists(&session).await?
            };
            let mut histories = self.histories.lock().await;
            if !stored && !histories.contains_key(&session) {
                return Err(SessionEventHubError::SessionNotFound);
            }
            let history = histories
                .entry(session)
                .or_insert_with(|| {
                    Arc::new(Mutex::new(SessionHistory::new(self.subscriber_capacity)))
                })
                .clone();
            drop(histories);
            let history = history.lock().await;
            Ok(SessionEventAttachment {
                snapshot: history.snapshot(),
                receiver: history.sender.subscribe(),
                epoch: history.replay_epoch,
            })
        })
    }

    fn state(&self, session: SessionRef) -> HubFuture<'_, SessionState> {
        Box::pin(async move {
            if let Some(history) = self.histories.lock().await.get(&session).cloned() {
                return Ok(history.lock().await.state.clone());
            }
            if self.persisted_session_exists(&session).await? {
                if let Some(history) = self.histories.lock().await.get(&session).cloned() {
                    return Ok(history.lock().await.state.clone());
                }
                return Ok(SessionState::Unloaded);
            }
            Err(SessionEventHubError::SessionNotFound)
        })
    }

    fn sessions(&self, endpoint: SessionEndpointRef) -> HubFuture<'_, Vec<HubSessionSummary>> {
        Box::pin(async move {
            let stored_endpoint = to_stored_endpoint(&endpoint)?;
            let inventory = self
                .store
                .lock()
                .await
                .list_sessions(&stored_endpoint)
                .await
                .map_err(|_| SessionEventHubError::Unavailable)?;
            let histories = self.histories.lock().await.clone();
            let mut summaries = Vec::with_capacity(inventory.len());
            for entry in inventory {
                let session = from_stored_session(&entry.target)?;
                let history = if let Some(history) = histories.get(&session) {
                    Some(history.lock().await)
                } else {
                    None
                };
                let state = history
                    .as_ref()
                    .map_or(SessionState::Unloaded, |history| history.state.clone());
                let settings = history
                    .as_ref()
                    .and_then(|history| history.settings.as_ref());
                summaries.push(HubSessionSummary {
                    session,
                    approver: entry
                        .approver
                        .to_board_identity()
                        .map_err(|_| SessionEventHubError::Unavailable)?,
                    working_directory: std::path::PathBuf::from(String::from(
                        entry.working_directory,
                    )),
                    updated_at_seconds: entry.updated_at_ms.div_euclid(1_000),
                    preview: String::new(),
                    name: None,
                    model: settings.and_then(|settings| settings.model.clone()),
                    mode: settings.and_then(|settings| settings.mode.clone()),
                    state,
                });
            }
            Ok(summaries)
        })
    }
}

impl ProviderSessionEventHub {
    async fn persisted_session_exists(
        &self,
        session: &SessionRef,
    ) -> Result<bool, SessionEventHubError> {
        let stored_session = to_stored_session(session)?;
        self.store
            .lock()
            .await
            .session_record(&stored_session)
            .await
            .map(|record| record.is_some())
            .map_err(|_| SessionEventHubError::Unavailable)
    }
}

fn to_stored_session(
    session: &SessionRef,
) -> Result<collaboration_protocol::SessionRef, SessionEventHubError> {
    Ok(collaboration_protocol::SessionRef {
        endpoint: to_stored_endpoint(&session.endpoint)?,
        session_id: collaboration_protocol::SessionId::try_from(
            session.session_id.as_str().to_owned(),
        )
        .map_err(|_| SessionEventHubError::Unavailable)?,
    })
}

fn to_stored_endpoint(
    endpoint: &SessionEndpointRef,
) -> Result<collaboration_protocol::EndpointRef, SessionEventHubError> {
    Ok(collaboration_protocol::EndpointRef {
        service_id: collaboration_protocol::UuidIdentity::try_from(
            endpoint.service_id.as_str().to_owned(),
        )
        .map_err(|_| SessionEventHubError::Unavailable)?,
        endpoint_id: collaboration_protocol::EndpointId::try_from(
            endpoint.endpoint_id.as_str().to_owned(),
        )
        .map_err(|_| SessionEventHubError::Unavailable)?,
    })
}

fn from_stored_session(
    session: &collaboration_protocol::SessionRef,
) -> Result<SessionRef, SessionEventHubError> {
    Ok(SessionRef {
        endpoint: SessionEndpointRef {
            service_id: message_board::ServiceId::try_from(String::from(
                session.endpoint.service_id.clone(),
            ))
            .map_err(|_| SessionEventHubError::Unavailable)?,
            endpoint_id: message_board::EndpointId::try_from(String::from(
                session.endpoint.endpoint_id.clone(),
            ))
            .map_err(|_| SessionEventHubError::Unavailable)?,
        },
        session_id: message_board::SessionId::try_from(String::from(session.session_id.clone()))
            .map_err(|_| SessionEventHubError::Unavailable)?,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum HubReceiveError {
    #[error("resyncRequired")]
    ResyncRequired,
}

pub async fn receive_hub_event(
    receiver: &mut broadcast::Receiver<HubEvent>,
) -> Result<HubEvent, HubReceiveError> {
    match receiver.recv().await {
        Ok(HubEvent {
            event: SessionEvent::ResyncRequired { .. },
            ..
        })
        | Err(broadcast::error::RecvError::Lagged(_))
        | Err(broadcast::error::RecvError::Closed) => Err(HubReceiveError::ResyncRequired),
        Ok(event) => Ok(event),
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use message_board::SessionRef;
    use session_event_model::{SessionEvent, SessionItem, SessionItemKind};
    use tokio::sync::Mutex;

    use super::ProviderSessionEventHub;
    use crate::ProviderOperationStore;

    #[tokio::test]
    async fn locked_attach_on_one_session_does_not_stall_another_publish() {
        let root = tempfile::tempdir().expect("temp root");
        let store = ProviderOperationStore::open(&root.path().join("operations.sqlite"))
            .await
            .expect("store");
        let hub = Arc::new(ProviderSessionEventHub::new(Arc::new(Mutex::new(store))));
        let first: SessionRef = serde_json::from_value(serde_json::json!({
            "endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"claude-local"},
            "sessionId":"first"
        })).expect("first session");
        let second: SessionRef = serde_json::from_value(serde_json::json!({
            "endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"claude-local"},
            "sessionId":"second"
        })).expect("second session");
        for target in [&first, &second] {
            hub.publish(
                target.clone(),
                SessionEvent::ItemStarted {
                    item: SessionItem {
                        item_id: "one".into(),
                        kind: SessionItemKind::UserMessage,
                        text: Some("hello".into()),
                    },
                },
            )
            .await
            .expect("seed item");
        }
        let first_history = hub
            .histories
            .lock()
            .await
            .get(&first)
            .cloned()
            .expect("first history");
        let _held_attach_lock = first_history.lock().await;
        tokio::time::timeout(
            Duration::from_millis(250),
            hub.publish(
                second,
                SessionEvent::ItemStarted {
                    item: SessionItem {
                        item_id: "two".into(),
                        kind: SessionItemKind::UserMessage,
                        text: Some("world".into()),
                    },
                },
            ),
        )
        .await
        .expect("independent publish must not wait")
        .expect("publish");
    }
}

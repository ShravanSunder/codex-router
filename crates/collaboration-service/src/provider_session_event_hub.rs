//! In-memory event order and attach replay for Router-owned provider Sessions.

use std::{collections::BTreeMap, sync::Arc};

use message_board::{SessionEndpointRef, SessionRef};
use session_event_model::{
    PendingInteraction, PendingInteractions, SessionEvent, SessionState, TurnOutcome,
};
use tokio::sync::{Mutex, broadcast};

use crate::{
    HubEvent, HubFuture, HubSessionSummary, ProviderOperationStore, SessionEventAttachment,
    SessionEventHub, SessionEventHubError,
};

pub struct ProviderSessionEventHub {
    store: Arc<Mutex<ProviderOperationStore>>,
    histories: Mutex<BTreeMap<SessionRef, SessionHistory>>,
    subscriber_capacity: usize,
}

struct SessionHistory {
    replay_epoch: u64,
    next_sequence: u64,
    events: Vec<HubEvent>,
    sender: broadcast::Sender<HubEvent>,
    state: SessionState,
    pending: BTreeMap<String, PendingInteraction>,
    turn_running: bool,
}

impl SessionHistory {
    fn new(subscriber_capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(subscriber_capacity);
        Self {
            replay_epoch: 0,
            next_sequence: 1,
            events: Vec::new(),
            sender,
            state: SessionState::Unloaded,
            pending: BTreeMap::new(),
            turn_running: false,
        }
    }

    fn project(&mut self, event: &SessionEvent) -> Result<(), SessionEventHubError> {
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
                    return Err(SessionEventHubError::Unavailable);
                }
                self.turn_running = false;
                self.state = match outcome {
                    TurnOutcome::Ended { .. } => SessionState::Idle,
                    TurnOutcome::Lost { reason } if reason == "endNotObservable" => {
                        SessionState::Idle
                    }
                    TurnOutcome::Lost { .. } => SessionState::Unloaded,
                };
            }
            SessionEvent::InteractionRequested { interaction } => {
                if self.pending.contains_key(interaction.request_id()) {
                    return Err(SessionEventHubError::Unavailable);
                }
                self.pending
                    .insert(interaction.request_id().to_owned(), interaction.clone());
                self.state = self.requires_action_state()?;
            }
            SessionEvent::InteractionResolved { request_id } => {
                if self.pending.remove(request_id).is_none() {
                    return Err(SessionEventHubError::Unavailable);
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
                        return Err(SessionEventHubError::Unavailable);
                    }
                    self.pending = advertised;
                } else if !self.pending.is_empty() {
                    return Err(SessionEventHubError::Unavailable);
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
            SessionEvent::ResyncRequired { .. } => {
                return Err(SessionEventHubError::Unavailable);
            }
        }
        Ok(())
    }

    fn requires_action_state(&self) -> Result<SessionState, SessionEventHubError> {
        let pending = PendingInteractions::new(self.pending.values().cloned().collect())
            .ok_or(SessionEventHubError::Unavailable)?;
        Ok(SessionState::RequiresAction { pending })
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

    /// Publishing and attach share one lock. No I/O is performed under it.
    pub async fn publish(
        &self,
        session: SessionRef,
        event: SessionEvent,
    ) -> Result<HubEvent, SessionEventHubError> {
        let mut histories = self.histories.lock().await;
        let history = histories
            .entry(session)
            .or_insert_with(|| SessionHistory::new(self.subscriber_capacity));
        let sequence = history.next_sequence;
        let next_sequence = sequence
            .checked_add(1)
            .ok_or(SessionEventHubError::Unavailable)?;
        history.project(&event)?;
        history.next_sequence = next_sequence;
        let item = HubEvent { sequence, event };
        history.events.push(item.clone());
        let _ = history.sender.send(item.clone());
        Ok(item)
    }

    /// Start a fresh agent history replay after provider loss. Old subscribers
    /// receive a control event and must attach again to the new epoch.
    pub async fn begin_history_replay(
        &self,
        session: SessionRef,
    ) -> Result<u64, SessionEventHubError> {
        let mut histories = self.histories.lock().await;
        let history = histories
            .entry(session)
            .or_insert_with(|| SessionHistory::new(self.subscriber_capacity));
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
        history.pending.clear();
        history.turn_running = false;
        history.state = SessionState::Unloaded;
        history.next_sequence = 1;
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
                .or_insert_with(|| SessionHistory::new(self.subscriber_capacity));
            Ok(SessionEventAttachment {
                snapshot: history.events.clone(),
                receiver: history.sender.subscribe(),
            })
        })
    }

    fn state(&self, session: SessionRef) -> HubFuture<'_, SessionState> {
        Box::pin(async move {
            if let Some(state) = self
                .histories
                .lock()
                .await
                .get(&session)
                .map(|history| history.state.clone())
            {
                return Ok(state);
            }
            if self.persisted_session_exists(&session).await? {
                return Ok(self
                    .histories
                    .lock()
                    .await
                    .get(&session)
                    .map_or(SessionState::Unloaded, |history| history.state.clone()));
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
            let histories = self.histories.lock().await;
            inventory
                .into_iter()
                .map(|entry| {
                    let session = from_stored_session(&entry.target)?;
                    let state = histories
                        .get(&session)
                        .map_or(SessionState::Unloaded, |history| history.state.clone());
                    Ok(HubSessionSummary {
                        session,
                        approver: message_board::Identity::Session {
                            session: from_stored_session(&entry.approver)?,
                        },
                        working_directory: std::path::PathBuf::from(String::from(
                            entry.working_directory,
                        )),
                        updated_at_seconds: entry.updated_at_ms.div_euclid(1_000),
                        preview: String::new(),
                        name: None,
                        model: None,
                        state,
                    })
                })
                .collect()
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

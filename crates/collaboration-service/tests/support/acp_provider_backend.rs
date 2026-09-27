//! Test-only provider command and event stand-in for the ACP socket journey.
use collaboration_service::{
    CommandFailure, CommandFuture, CreateSessionCommand, HubEvent, HubFuture, HubSessionSummary,
    PromptSessionCommand, QueueInputCommand, QueuedSessionInput, SessionCommandPort,
    SessionEventAttachment, SessionEventHub, SessionEventHubError, SessionSteerOutcome,
    SessionTargetCommand, SessionTurnHandle, SetSessionSettingCommand, SteerSessionCommand,
};
use message_board::{Identity, SessionEndpointRef, SessionRef};
use session_event_model::{
    CapabilityReport, QueueCapability, QueueSupport, SessionEvent, SessionItem, SessionItemKind,
    SessionState, StopReason, TurnOutcome,
};
use std::sync::Mutex;
use tokio::sync::broadcast;

pub(crate) struct ScriptedProviderBackend {
    pub(crate) endpoint: SessionEndpointRef,
    pub(crate) session: SessionRef,
    pub(crate) approver: Identity,
    pub(crate) created_by: Mutex<Vec<Identity>>,
    pub(crate) prompted_by: Mutex<Vec<Identity>>,
    pub(crate) loaded_by: Mutex<Vec<Identity>>,
    pub(crate) resumed_by: Mutex<Vec<Identity>>,
    pub(crate) steered_by: Mutex<Vec<Identity>>,
    pub(crate) queued_by: Mutex<Vec<Identity>>,
    pub(crate) state: Mutex<SessionState>,
    pub(crate) history_available: std::sync::atomic::AtomicBool,
    pub(crate) events: broadcast::Sender<HubEvent>,
}

impl ScriptedProviderBackend {
    pub(crate) fn new() -> Result<Self, serde_json::Error> {
        let endpoint = serde_json::from_value(serde_json::json!({
            "serviceId":"0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89","endpointId":"claude-local"
        }))?;
        let session = serde_json::from_value(serde_json::json!({
            "endpoint":endpoint,"sessionId":"claude-session-1"
        }))?;
        let approver =
            serde_json::from_value(serde_json::json!({"kind":"human","humanId":"owner"}))?;
        let (events, _) = broadcast::channel(16);
        Ok(Self {
            endpoint,
            session,
            approver,
            created_by: Mutex::new(Vec::new()),
            prompted_by: Mutex::new(Vec::new()),
            loaded_by: Mutex::new(Vec::new()),
            resumed_by: Mutex::new(Vec::new()),
            steered_by: Mutex::new(Vec::new()),
            queued_by: Mutex::new(Vec::new()),
            state: Mutex::new(SessionState::Idle),
            history_available: std::sync::atomic::AtomicBool::new(false),
            events,
        })
    }
}

impl SessionCommandPort for ScriptedProviderBackend {
    fn create(&self, command: CreateSessionCommand) -> CommandFuture<'_, SessionRef> {
        self.created_by
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(command.actor);
        let session = self.session.clone();
        Box::pin(async move { Ok(session) })
    }
    fn load_session(&self, command: SessionTargetCommand) -> CommandFuture<'_, ()> {
        self.loaded_by
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(command.actor);
        *self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = SessionState::Idle;
        self.history_available
            .store(true, std::sync::atomic::Ordering::SeqCst);
        Box::pin(async { Ok(()) })
    }
    fn resume_session(&self, command: SessionTargetCommand) -> CommandFuture<'_, ()> {
        self.resumed_by
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(command.actor);
        *self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = SessionState::Idle;
        Box::pin(async { Ok(()) })
    }
    fn prompt(&self, command: PromptSessionCommand) -> CommandFuture<'_, SessionTurnHandle> {
        self.prompted_by
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(command.actor);
        let _sent = self.events.send(HubEvent {
            sequence: 2,
            event: SessionEvent::StateChanged {
                state: SessionState::Running,
            },
        });
        let _sent = self.events.send(HubEvent {
            sequence: 3,
            event: SessionEvent::ItemStarted {
                item: SessionItem {
                    item_id: "item-1".into(),
                    kind: SessionItemKind::AgentMessage,
                    text: Some("Claude reply".into()),
                },
            },
        });
        let _sent = self.events.send(HubEvent {
            sequence: 4,
            event: SessionEvent::StateChanged {
                state: SessionState::Idle,
            },
        });
        let _sent = self.events.send(HubEvent {
            sequence: 5,
            event: SessionEvent::TurnEnded {
                turn_id: "turn-1".into(),
                outcome: TurnOutcome::Ended {
                    stop_reason: StopReason::EndTurn,
                    local_cause: None,
                },
            },
        });
        Box::pin(async {
            Ok(SessionTurnHandle {
                turn_id: "turn-1".into(),
            })
        })
    }
    fn steer(&self, command: SteerSessionCommand) -> CommandFuture<'_, SessionSteerOutcome> {
        self.steered_by
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(command.actor);
        Box::pin(async {
            Ok(SessionSteerOutcome::Injected {
                turn_id: "turn-1".into(),
            })
        })
    }
    fn queue_add(&self, command: QueueInputCommand) -> CommandFuture<'_, QueuedSessionInput> {
        self.queued_by
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(command.actor);
        Box::pin(async {
            Ok(QueuedSessionInput {
                input_id: "input-2".into(),
                position: 1,
                preview: "later".into(),
            })
        })
    }
    fn queue_list(&self, _: SessionTargetCommand) -> CommandFuture<'_, Vec<QueuedSessionInput>> {
        Box::pin(async {
            Ok(vec![QueuedSessionInput {
                input_id: "input-2".into(),
                position: 1,
                preview: "later".into(),
            }])
        })
    }
    fn queue_cancel(&self, _: SessionTargetCommand, _: String) -> CommandFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
    fn cancel(&self, _: SessionTargetCommand) -> CommandFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
    fn close(&self, _: SessionTargetCommand) -> CommandFuture<'_, ()> {
        Box::pin(async { Err(CommandFailure::Unsupported) })
    }
    fn set_setting(&self, _: SetSessionSettingCommand) -> CommandFuture<'_, ()> {
        Box::pin(async { Err(CommandFailure::Unsupported) })
    }
}

impl SessionEventHub for ScriptedProviderBackend {
    fn attach(&self, session: SessionRef) -> HubFuture<'_, SessionEventAttachment> {
        let exists = session == self.session;
        let receiver = self.events.subscribe();
        let running = *self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            == SessionState::Running;
        let history_available = self
            .history_available
            .load(std::sync::atomic::Ordering::SeqCst);
        Box::pin(async move {
            if !exists {
                return Err(SessionEventHubError::SessionNotFound);
            }
            Ok(SessionEventAttachment {
                snapshot: vec![HubEvent {
                    sequence: 1,
                    event: SessionEvent::CapabilitiesChanged {
                        capabilities: CapabilityReport {
                            load: true,
                            resume: true,
                            steer: true,
                            queue: Some(QueueSupport {
                                kind: QueueCapability::Router,
                                can_cancel: true,
                            }),
                            ..CapabilityReport::default()
                        },
                    },
                }]
                .into_iter()
                .chain(history_available.then_some(HubEvent {
                    sequence: 2,
                    event: SessionEvent::ItemStarted {
                        item: SessionItem {
                            item_id: "historical-item".into(),
                            kind: SessionItemKind::AgentMessage,
                            text: Some("Prior reply".into()),
                        },
                    },
                }))
                .chain(running.then_some(HubEvent {
                    sequence: 2,
                    event: SessionEvent::TurnStarted {
                        turn_id: "turn-1".into(),
                        input_id: session_event_model::InputId::generate(),
                    },
                }))
                .collect(),
                receiver,
            })
        })
    }
    fn state(&self, session: SessionRef) -> HubFuture<'_, SessionState> {
        let exists = session == self.session;
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        Box::pin(async move {
            if exists {
                Ok(state)
            } else {
                Err(SessionEventHubError::SessionNotFound)
            }
        })
    }
    fn sessions(&self, endpoint: SessionEndpointRef) -> HubFuture<'_, Vec<HubSessionSummary>> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let result = if endpoint == self.endpoint {
            vec![HubSessionSummary {
                session: self.session.clone(),
                approver: self.approver.clone(),
                working_directory: "/tmp".into(),
                updated_at_seconds: 1,
                preview: String::new(),
                name: None,
                model: None,
                state,
            }]
        } else {
            Vec::new()
        };
        Box::pin(async move { Ok(result) })
    }
}

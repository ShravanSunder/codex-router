use super::*;
use crate::{
    CommandFailure, CommandFuture, CreateSessionCommand, HubFuture, HubSessionSummary,
    PromptSessionCommand, QueueInputCommand, QueuedSessionInput, SessionCommandPort,
    SessionEventAttachment, SessionEventHub, SessionEventHubError, SessionSteerOutcome,
    SessionTargetCommand, SessionTurnHandle, SetSessionSettingCommand, SteerSessionCommand,
};
use message_board::{Identity, SessionEndpointRef, SessionRef};
use session_event_model::{SessionEvent, SessionState, StopReason, TurnOutcome};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio::sync::{Notify, broadcast};

pub(super) struct ScriptedSessionBackend {
    pub(super) endpoint: SessionEndpointRef,
    pub(super) session: SessionRef,
    pub(super) approver: Identity,
    pub(super) durable_inventory: Mutex<Vec<HubSessionSummary>>,
    pub(super) live_states: Mutex<HashMap<SessionRef, SessionState>>,
    pub(super) create_commands: Mutex<Vec<CreateSessionCommand>>,
    pub(super) load_commands: Mutex<Vec<SessionTargetCommand>>,
    pub(super) prompt_commands: Mutex<Vec<PromptSessionCommand>>,
    pub(super) steer_commands: Mutex<Vec<SteerSessionCommand>>,
    pub(super) steer_gate: Mutex<Option<Arc<Notify>>>,
    pub(super) steer_started: Notify,
    pub(super) turn_command_order: Mutex<Vec<&'static str>>,
    pub(super) cancel_commands: Mutex<Vec<SessionTargetCommand>>,
    pub(super) setting_commands: Mutex<Vec<SetSessionSettingCommand>>,
    pub(super) created: Notify,
    pub(super) events: broadcast::Sender<crate::HubEvent>,
    pub(super) history: Mutex<Vec<crate::HubEvent>>,
    pub(super) effective_model: Mutex<Option<String>>,
    pub(super) attachment_count: AtomicUsize,
    pub(super) attachment_created: Notify,
    pub(super) prompt_gate: Mutex<Option<Arc<Notify>>>,
    pub(super) prompt_submitted: Notify,
}

impl ScriptedSessionBackend {
    pub(super) fn new() -> Result<Arc<Self>, serde_json::Error> {
        let endpoint: SessionEndpointRef = serde_json::from_value(json!({
            "serviceId":"0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89",
            "endpointId":"claude-local"
        }))?;
        let session: SessionRef = serde_json::from_value(json!({
            "endpoint":endpoint,
            "sessionId":"19e49a31-daa3-428c-b985-e0c7373a89ed"
        }))?;
        let approver = Self::actor()?;
        let (events, _) = broadcast::channel(16);
        Ok(Arc::new(Self {
            endpoint,
            session,
            approver,
            durable_inventory: Mutex::new(Vec::new()),
            live_states: Mutex::new(HashMap::new()),
            create_commands: Mutex::new(Vec::new()),
            load_commands: Mutex::new(Vec::new()),
            prompt_commands: Mutex::new(Vec::new()),
            steer_commands: Mutex::new(Vec::new()),
            steer_gate: Mutex::new(None),
            steer_started: Notify::new(),
            turn_command_order: Mutex::new(Vec::new()),
            cancel_commands: Mutex::new(Vec::new()),
            setting_commands: Mutex::new(Vec::new()),
            created: Notify::new(),
            events,
            history: Mutex::new(Vec::new()),
            effective_model: Mutex::new(None),
            attachment_count: AtomicUsize::new(0),
            attachment_created: Notify::new(),
            prompt_gate: Mutex::new(None),
            prompt_submitted: Notify::new(),
        }))
    }

    pub(super) fn actor() -> Result<Identity, serde_json::Error> {
        serde_json::from_value(json!({"kind":"human","humanId":"owner"}))
    }
}

impl SessionCommandPort for ScriptedSessionBackend {
    fn create(&self, command: CreateSessionCommand) -> CommandFuture<'_, SessionRef> {
        self.create_commands
            .lock()
            .expect("test lock")
            .push(command.clone());
        self.durable_inventory
            .lock()
            .expect("test lock")
            .push(HubSessionSummary {
                session: self.session.clone(),
                approver: self.approver.clone(),
                working_directory: command.working_directory,
                updated_at_seconds: 1_700_000_000,
                preview: String::new(),
                name: None,
                model: self.effective_model.lock().expect("test lock").clone(),
                mode: None,
                state: SessionState::Unloaded,
            });
        self.live_states
            .lock()
            .expect("test lock")
            .insert(self.session.clone(), SessionState::Idle);
        self.created.notify_one();
        let session = self.session.clone();
        Box::pin(async move { Ok(session) })
    }
    fn load_session(&self, command: SessionTargetCommand) -> CommandFuture<'_, ()> {
        self.load_commands.lock().expect("test lock").push(command);
        self.live_states
            .lock()
            .expect("test lock")
            .insert(self.session.clone(), SessionState::Idle);
        Box::pin(async { Ok(()) })
    }
    fn resume_session(&self, _: SessionTargetCommand) -> CommandFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
    fn prompt(&self, command: PromptSessionCommand) -> CommandFuture<'_, SessionTurnHandle> {
        let input_id = command.input_id.clone();
        let gate = self.prompt_gate.lock().expect("test lock").clone();
        self.prompt_commands
            .lock()
            .expect("test lock")
            .push(command);
        self.live_states
            .lock()
            .expect("test lock")
            .insert(self.session.clone(), SessionState::Running);
        let _sent = self.events.send(crate::HubEvent {
            sequence: 1,
            event: SessionEvent::TurnStarted {
                turn_id: "turn-1".into(),
                input_id,
            },
        });
        self.prompt_submitted.notify_one();
        Box::pin(async move {
            if let Some(gate) = gate {
                gate.notified().await;
            }
            Ok(SessionTurnHandle {
                turn_id: "turn-1".into(),
            })
        })
    }
    fn steer(&self, command: SteerSessionCommand) -> CommandFuture<'_, SessionSteerOutcome> {
        let gate = self.steer_gate.lock().expect("test lock").take();
        self.steer_started.notify_one();
        Box::pin(async move {
            if let Some(gate) = gate {
                gate.notified().await;
            }
            self.steer_commands.lock().expect("test lock").push(command);
            self.turn_command_order
                .lock()
                .expect("test lock")
                .push("steer");
            Ok(SessionSteerOutcome::Injected {
                turn_id: "turn-1".into(),
            })
        })
    }
    fn queue_add(&self, _: QueueInputCommand) -> CommandFuture<'_, QueuedSessionInput> {
        Box::pin(async { Err(CommandFailure::Unsupported) })
    }
    fn queue_list(&self, _: SessionTargetCommand) -> CommandFuture<'_, Vec<QueuedSessionInput>> {
        Box::pin(async { Err(CommandFailure::Unsupported) })
    }
    fn queue_cancel(&self, _: SessionTargetCommand, _: String) -> CommandFuture<'_, ()> {
        Box::pin(async { Err(CommandFailure::Unsupported) })
    }
    fn cancel(&self, command: SessionTargetCommand) -> CommandFuture<'_, ()> {
        self.turn_command_order
            .lock()
            .expect("test lock")
            .push("interrupt");
        self.cancel_commands
            .lock()
            .expect("test lock")
            .push(command);
        let _sent = self.events.send(crate::HubEvent {
            sequence: 2,
            event: SessionEvent::TurnEnded {
                turn_id: "turn-1".into(),
                outcome: TurnOutcome::Ended {
                    stop_reason: StopReason::Cancelled,
                    local_cause: None,
                },
            },
        });
        Box::pin(async { Ok(()) })
    }
    fn close(&self, _: SessionTargetCommand) -> CommandFuture<'_, ()> {
        Box::pin(async { Err(CommandFailure::Unsupported) })
    }
    fn set_setting(&self, command: SetSessionSettingCommand) -> CommandFuture<'_, ()> {
        self.setting_commands
            .lock()
            .expect("test lock")
            .push(command);
        Box::pin(async { Ok(()) })
    }
}

impl SessionEventHub for ScriptedSessionBackend {
    fn attach(&self, _: SessionRef) -> HubFuture<'_, SessionEventAttachment> {
        self.attachment_count.fetch_add(1, Ordering::Relaxed);
        self.attachment_created.notify_one();
        let receiver = self.events.subscribe();
        let snapshot = self.history.lock().expect("test lock").clone();
        Box::pin(async move {
            Ok(SessionEventAttachment {
                snapshot,
                receiver,
                epoch: 0,
            })
        })
    }
    fn state(&self, session: SessionRef) -> HubFuture<'_, SessionState> {
        let exists = self
            .durable_inventory
            .lock()
            .expect("test lock")
            .iter()
            .find(|summary| summary.session == session)
            .is_some();
        let state = self
            .live_states
            .lock()
            .expect("test lock")
            .get(&session)
            .cloned()
            .or_else(|| exists.then_some(SessionState::Unloaded));
        Box::pin(async move { state.ok_or(SessionEventHubError::SessionNotFound) })
    }
    fn sessions(&self, endpoint: SessionEndpointRef) -> HubFuture<'_, Vec<HubSessionSummary>> {
        let live_states = self.live_states.lock().expect("test lock");
        let sessions = self
            .durable_inventory
            .lock()
            .expect("test lock")
            .iter()
            .filter(|summary| summary.session.endpoint == endpoint)
            .cloned()
            .map(|mut summary| {
                summary.state = live_states
                    .get(&summary.session)
                    .cloned()
                    .unwrap_or(SessionState::Unloaded);
                summary
            })
            .collect();
        Box::pin(async move { Ok(sessions) })
    }
}

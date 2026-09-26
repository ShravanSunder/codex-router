//! Provider-session commands shared by client-facing protocol adapters.
//!
//! The Host supplies the implementation. This crate owns the interface so no
//! front door needs to know the provider runtime or ACP SDK.
use collaboration_protocol::EndpointRef;
use message_board::{Identity, SessionRef};
use std::{future::Future, path::PathBuf, pin::Pin};

pub type CommandFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, CommandFailure>> + Send + 'a>>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CommandContent {
    Text(String),
    ResourceLink { uri: String, name: String },
    Image { mime_type: String, bytes: Vec<u8> },
    Audio { mime_type: String, bytes: Vec<u8> },
    EmbeddedResource { uri: String, text: String },
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SessionSettingsCommand {
    pub model: Option<String>,
    pub mode: Option<String>,
    pub effort: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreateSessionCommand {
    pub endpoint: EndpointRef,
    pub working_directory: PathBuf,
    pub settings: SessionSettingsCommand,
    pub actor: Identity,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PromptSessionCommand {
    pub target: SessionRef,
    pub content: Vec<CommandContent>,
    pub actor: Identity,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SteerSessionCommand {
    pub target: SessionRef,
    pub content: Vec<CommandContent>,
    pub actor: Identity,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueueInputCommand {
    pub target: SessionRef,
    pub content: Vec<CommandContent>,
    pub actor: Identity,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueuedSessionInput {
    pub input_id: String,
    pub position: u32,
    pub preview: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionTurnHandle {
    pub turn_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SessionSteerOutcome {
    Injected { turn_id: String },
    StartedNewTurn { turn_id: String },
    PromptRequired,
    Failed { reason: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionTargetCommand {
    pub target: SessionRef,
    pub actor: Identity,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SetSessionSettingCommand {
    pub target: SessionRef,
    pub setting_id: String,
    pub value: String,
    pub actor: Identity,
}

#[derive(Clone, Debug, thiserror::Error)]
pub enum CommandFailure {
    #[error("session not found")]
    SessionNotFound,
    #[error("operation is unsupported")]
    Unsupported,
    #[error("session is busy")]
    Busy,
    #[error("session settings are unresolved")]
    SettingsUnresolved,
    #[error("content type is unsupported: {0}")]
    UnsupportedContent(&'static str),
    #[error("setting value is invalid")]
    InvalidSetting { advertised: Vec<String> },
    #[error("actor is not authorized")]
    UnauthorizedActor,
    #[error("provider command failed")]
    ProviderFailure,
}

/// Commands have distinct results and all carry the identity of the caller.
/// Read-side history and live events belong to the session event hub.
pub trait SessionCommandPort: Send + Sync {
    fn create(&self, command: CreateSessionCommand) -> CommandFuture<'_, SessionRef>;
    fn prompt(&self, command: PromptSessionCommand) -> CommandFuture<'_, SessionTurnHandle>;
    fn steer(&self, command: SteerSessionCommand) -> CommandFuture<'_, SessionSteerOutcome>;
    fn queue_add(&self, command: QueueInputCommand) -> CommandFuture<'_, QueuedSessionInput>;
    fn queue_list(
        &self,
        command: SessionTargetCommand,
    ) -> CommandFuture<'_, Vec<QueuedSessionInput>>;
    fn queue_cancel(
        &self,
        command: SessionTargetCommand,
        input_id: String,
    ) -> CommandFuture<'_, ()>;
    fn cancel(&self, command: SessionTargetCommand) -> CommandFuture<'_, ()>;
    fn close(&self, command: SessionTargetCommand) -> CommandFuture<'_, ()>;
    fn set_setting(&self, command: SetSessionSettingCommand) -> CommandFuture<'_, ()>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Debug, Eq, PartialEq)]
    enum RecordedCommand {
        Create(CreateSessionCommand),
        Prompt(PromptSessionCommand),
        Steer(SteerSessionCommand),
        QueueAdd(QueueInputCommand),
        QueueList(SessionTargetCommand),
        QueueCancel(SessionTargetCommand, String),
        Cancel(SessionTargetCommand),
        Close(SessionTargetCommand),
        SetSetting(SetSessionSettingCommand),
    }

    /// A scripted stand-in records the exact front-door command. It does not
    /// prove Host composition, provider state, or delivery.
    struct ScriptedCommandPort {
        recorded: Arc<Mutex<Vec<RecordedCommand>>>,
        created_session: SessionRef,
    }

    impl ScriptedCommandPort {
        fn record(&self, command: RecordedCommand) {
            self.recorded.lock().expect("test lock").push(command);
        }
    }

    impl SessionCommandPort for ScriptedCommandPort {
        fn create(&self, command: CreateSessionCommand) -> CommandFuture<'_, SessionRef> {
            self.record(RecordedCommand::Create(command));
            let session = self.created_session.clone();
            Box::pin(async move { Ok(session) })
        }
        fn prompt(&self, command: PromptSessionCommand) -> CommandFuture<'_, SessionTurnHandle> {
            self.record(RecordedCommand::Prompt(command));
            Box::pin(async {
                Ok(SessionTurnHandle {
                    turn_id: "turn-1".into(),
                })
            })
        }
        fn steer(&self, command: SteerSessionCommand) -> CommandFuture<'_, SessionSteerOutcome> {
            self.record(RecordedCommand::Steer(command));
            Box::pin(async {
                Ok(SessionSteerOutcome::Injected {
                    turn_id: "turn-1".into(),
                })
            })
        }
        fn queue_add(&self, command: QueueInputCommand) -> CommandFuture<'_, QueuedSessionInput> {
            self.record(RecordedCommand::QueueAdd(command));
            Box::pin(async {
                Ok(QueuedSessionInput {
                    input_id: "input-1".into(),
                    position: 1,
                    preview: "hello".into(),
                })
            })
        }
        fn queue_list(
            &self,
            command: SessionTargetCommand,
        ) -> CommandFuture<'_, Vec<QueuedSessionInput>> {
            self.record(RecordedCommand::QueueList(command));
            Box::pin(async { Ok(Vec::new()) })
        }
        fn queue_cancel(
            &self,
            command: SessionTargetCommand,
            input_id: String,
        ) -> CommandFuture<'_, ()> {
            self.record(RecordedCommand::QueueCancel(command, input_id));
            Box::pin(async { Ok(()) })
        }
        fn cancel(&self, command: SessionTargetCommand) -> CommandFuture<'_, ()> {
            self.record(RecordedCommand::Cancel(command));
            Box::pin(async { Ok(()) })
        }
        fn close(&self, command: SessionTargetCommand) -> CommandFuture<'_, ()> {
            self.record(RecordedCommand::Close(command));
            Box::pin(async { Ok(()) })
        }
        fn set_setting(&self, command: SetSessionSettingCommand) -> CommandFuture<'_, ()> {
            self.record(RecordedCommand::SetSetting(command));
            Box::pin(async { Ok(()) })
        }
    }

    #[tokio::test]
    async fn every_command_keeps_the_typed_actor() {
        let actor: Identity =
            serde_json::from_value(serde_json::json!({"kind":"human","humanId":"owner"}))
                .expect("typed human actor");
        let session: SessionRef = serde_json::from_value(serde_json::json!({
            "endpoint":{"serviceId":"0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89","endpointId":"claude-local"},
            "sessionId":"19e49a31-daa3-428c-b985-e0c7373a89ed"
        }))
        .expect("session reference");
        let endpoint: EndpointRef = serde_json::from_value(serde_json::json!({
            "serviceId":"0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89","endpointId":"claude-local"
        }))
        .expect("endpoint");
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let port = ScriptedCommandPort {
            recorded: Arc::clone(&recorded),
            created_session: session.clone(),
        };
        let target = SessionTargetCommand {
            target: session.clone(),
            actor: actor.clone(),
        };
        let content = vec![CommandContent::Text("hello".into())];

        assert_eq!(
            port.create(CreateSessionCommand {
                endpoint,
                working_directory: PathBuf::from("/tmp"),
                settings: SessionSettingsCommand::default(),
                actor: actor.clone(),
            })
            .await
            .expect("create"),
            session
        );
        port.prompt(PromptSessionCommand {
            target: session.clone(),
            content: content.clone(),
            actor: actor.clone(),
        })
        .await
        .expect("prompt");
        port.steer(SteerSessionCommand {
            target: session.clone(),
            content: content.clone(),
            actor: actor.clone(),
        })
        .await
        .expect("steer");
        port.queue_add(QueueInputCommand {
            target: session.clone(),
            content,
            actor: actor.clone(),
        })
        .await
        .expect("queue add");
        port.queue_list(target.clone()).await.expect("queue list");
        port.queue_cancel(target.clone(), "input-1".into())
            .await
            .expect("queue cancel");
        port.cancel(target.clone()).await.expect("cancel");
        port.close(target.clone()).await.expect("close");
        port.set_setting(SetSessionSettingCommand {
            target: session,
            setting_id: "model".into(),
            value: "default".into(),
            actor: actor.clone(),
        })
        .await
        .expect("set setting");

        let recorded = recorded.lock().expect("test lock");
        assert_eq!(recorded.len(), 9);
        assert!(recorded.iter().all(|command| match command {
            RecordedCommand::Create(command) => command.actor == actor,
            RecordedCommand::Prompt(command) => command.actor == actor,
            RecordedCommand::Steer(command) => command.actor == actor,
            RecordedCommand::QueueAdd(command) => command.actor == actor,
            RecordedCommand::QueueList(command)
            | RecordedCommand::QueueCancel(command, _)
            | RecordedCommand::Cancel(command)
            | RecordedCommand::Close(command) => command.actor == actor,
            RecordedCommand::SetSetting(command) => command.actor == actor,
        }));
    }
}

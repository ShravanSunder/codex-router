//! Host implementation of the provider Session command boundary used by both faces.

use crate::{
    ExternalProviderRuntimeError, ExternalProviderSupervisor, ProviderAcpDeliveryRoute,
    ProviderCancelActiveTurnError, ProviderPromptContentsError, ProviderQueueAdmissionError,
    ProviderQueueCancellationError, ProviderSteerContentsError, ProviderSteeringOutcome,
};
use collaboration_protocol::{
    ConversationCloseRequest, ConversationCreateRequest, ConversationLoadRequest,
    ConversationOperationFailure, ConversationOperationFailureKind,
    ConversationOperationSettlement, ConversationOperationWaitOutput,
    ConversationOperationWaitRequest, ConversationResumeRequest, EndpointId, EndpointRef,
    OperationId, PositiveSeconds, ProviderIdentity, ProviderRequestedPolicy,
    ProviderRequestedSettings, ProviderSettingName, ProviderSettingsFailureKind,
    ProviderSettingsSetRequest, ProviderWorkingDirectory, RouterAccess, SessionId, SessionRef,
    UuidIdentity,
};
use collaboration_service::{
    CommandFailure, CommandFuture, CreateSessionCommand, PromptSessionCommand,
    ProviderConversationBackend, ProviderOperationStore, ProviderSessionEventHub,
    QueueInputCommand, QueuedSessionInput, SessionCommandPort, SessionEventAttachment,
    SessionEventHub, SessionSteerOutcome, SessionTargetCommand, SessionTurnHandle,
    SetSessionSettingCommand, SteerSessionCommand,
};
use message_board::{Identity, SessionEndpointRef};
use session_event_model::{InputId, SessionEvent};
use std::{sync::Arc, time::Duration};
use tokio::sync::Mutex;

const OPERATION_WAIT_SECONDS: u32 = 30;
const TURN_START_DEADLINE: Duration = Duration::from_secs(30);

pub struct HostSessionCommandPort {
    supervisor: Arc<ExternalProviderSupervisor>,
    delivery: Arc<ProviderAcpDeliveryRoute>,
    hub: Arc<ProviderSessionEventHub>,
    store: Arc<Mutex<ProviderOperationStore>>,
}

impl HostSessionCommandPort {
    #[must_use]
    pub fn new(
        supervisor: Arc<ExternalProviderSupervisor>,
        delivery: Arc<ProviderAcpDeliveryRoute>,
        hub: Arc<ProviderSessionEventHub>,
        store: Arc<Mutex<ProviderOperationStore>>,
    ) -> Self {
        Self {
            supervisor,
            delivery,
            hub,
            store,
        }
    }

    async fn stored_session(
        &self,
        target: &SessionRef,
    ) -> Result<collaboration_service::ProviderSessionRecord, CommandFailure> {
        self.store
            .lock()
            .await
            .session_record(target)
            .await
            .map_err(|_| CommandFailure::ProviderFailure)?
            .ok_or(CommandFailure::SessionNotFound)
    }
}

impl SessionCommandPort for HostSessionCommandPort {
    fn create(
        &self,
        command: CreateSessionCommand,
    ) -> CommandFuture<'_, message_board::SessionRef> {
        Box::pin(async move {
            let operation_id = OperationId::generate();
            let request = ConversationCreateRequest {
                operation_id: operation_id.clone(),
                endpoint: protocol_endpoint(&command.endpoint)?,
                generation: None,
                working_directory: working_directory(&command.working_directory)?,
                created_by: provider_actor(&command.actor)?,
                approver: provider_actor(&command.actor)?,
                requested_policy: default_policy(),
                settings: Some(ProviderRequestedSettings {
                    mode: command.settings.mode,
                    model: command.settings.model,
                    effort: command.settings.effort,
                }),
            };
            self.supervisor
                .create(request)
                .await
                .map_err(map_operation_failure)?;
            let settlement = wait_for_settlement(self.supervisor.as_ref(), operation_id).await?;
            match settlement {
                ConversationOperationSettlement::Created { target, .. }
                | ConversationOperationSettlement::CreatedWithoutSettings { target, .. } => {
                    board_session(target)
                }
                _ => Err(CommandFailure::ProviderFailure),
            }
        })
    }

    fn load_session(&self, command: SessionTargetCommand) -> CommandFuture<'_, ()> {
        Box::pin(async move {
            let target = protocol_session(&command.target)?;
            let stored = self.stored_session(&target).await?;
            let operation_id = OperationId::generate();
            self.supervisor
                .load(ConversationLoadRequest {
                    operation_id: operation_id.clone(),
                    target,
                    generation: None,
                    working_directory: stored.working_directory,
                    requested_by: provider_actor(&command.actor)?,
                    approver: stored.approver,
                    requested_policy: stored.requested_policy,
                })
                .await
                .map_err(map_operation_failure)?;
            match wait_for_settlement(self.supervisor.as_ref(), operation_id).await? {
                ConversationOperationSettlement::Loaded { .. } => Ok(()),
                _ => Err(CommandFailure::ProviderFailure),
            }
        })
    }

    fn resume_session(&self, command: SessionTargetCommand) -> CommandFuture<'_, ()> {
        Box::pin(async move {
            let target = protocol_session(&command.target)?;
            let stored = self.stored_session(&target).await?;
            let operation_id = OperationId::generate();
            self.supervisor
                .resume(ConversationResumeRequest {
                    operation_id: operation_id.clone(),
                    target,
                    generation: None,
                    working_directory: stored.working_directory,
                    requested_by: provider_actor(&command.actor)?,
                    approver: stored.approver,
                    requested_policy: stored.requested_policy,
                })
                .await
                .map_err(map_operation_failure)?;
            match wait_for_settlement(self.supervisor.as_ref(), operation_id).await? {
                ConversationOperationSettlement::Resumed { .. } => Ok(()),
                _ => Err(CommandFailure::ProviderFailure),
            }
        })
    }

    fn prompt(&self, command: PromptSessionCommand) -> CommandFuture<'_, SessionTurnHandle> {
        Box::pin(async move {
            let attachment = self
                .hub
                .attach(command.target.clone())
                .await
                .map_err(|_| CommandFailure::SessionNotFound)?;
            let target = protocol_session(&command.target)?;
            let actor = provider_actor(&command.actor)?;
            let input_id = command.input_id;
            let _submitted = self
                .delivery
                .prompt_contents(target, actor, input_id.clone(), command.content)
                .await
                .map_err(map_prompt_failure)?;
            let turn_id =
                wait_for_turn_start(self.hub.as_ref(), command.target, attachment, input_id)
                    .await?;
            Ok(SessionTurnHandle { turn_id })
        })
    }

    fn steer(&self, command: SteerSessionCommand) -> CommandFuture<'_, SessionSteerOutcome> {
        Box::pin(async move {
            let attachment = self
                .hub
                .attach(command.target.clone())
                .await
                .map_err(|_| CommandFailure::SessionNotFound)?;
            if active_turn_id(&attachment) != Some(command.expected_turn_id.as_str()) {
                return Err(CommandFailure::Busy);
            }
            let target = protocol_session(&command.target)?;
            let actor = provider_actor(&command.actor)?;
            let outcome = self
                .delivery
                .steer_contents(target, actor, command.input_id.clone(), command.content)
                .await
                .map_err(map_steer_failure)?;
            match outcome {
                ProviderSteeringOutcome::Injected { .. } => Ok(SessionSteerOutcome::Injected {
                    turn_id: command.expected_turn_id,
                }),
                ProviderSteeringOutcome::StartedNewTurn => {
                    let turn_id = wait_for_turn_start(
                        self.hub.as_ref(),
                        command.target,
                        attachment,
                        command.input_id,
                    )
                    .await?;
                    Ok(SessionSteerOutcome::StartedNewTurn { turn_id })
                }
                ProviderSteeringOutcome::PromptRequired => Ok(SessionSteerOutcome::PromptRequired),
                ProviderSteeringOutcome::Failed => Ok(SessionSteerOutcome::Failed {
                    reason: "provider steering failed".into(),
                }),
            }
        })
    }

    fn queue_add(&self, command: QueueInputCommand) -> CommandFuture<'_, QueuedSessionInput> {
        Box::pin(async move {
            let queued = self
                .delivery
                .queue_contents(
                    protocol_session(&command.target)?,
                    provider_actor(&command.actor)?,
                    command.content,
                )
                .await
                .map_err(map_admission_failure)?;
            Ok(queued_input(queued))
        })
    }

    fn queue_list(
        &self,
        command: SessionTargetCommand,
    ) -> CommandFuture<'_, Vec<QueuedSessionInput>> {
        Box::pin(async move {
            let target = protocol_session(&command.target)?;
            Ok(self
                .delivery
                .queue_list(&target)
                .into_iter()
                .map(queued_input)
                .collect())
        })
    }

    fn queue_cancel(
        &self,
        command: SessionTargetCommand,
        input_id: String,
    ) -> CommandFuture<'_, ()> {
        Box::pin(async move {
            let target = protocol_session(&command.target)?;
            let input_id = InputId::new(input_id).map_err(|_| CommandFailure::SessionNotFound)?;
            self.delivery
                .queue_cancel(&target, &input_id)
                .map_err(|error| match error {
                    ProviderQueueCancellationError::NotQueued => CommandFailure::SessionNotFound,
                })
        })
    }

    fn cancel(&self, command: SessionTargetCommand) -> CommandFuture<'_, ()> {
        Box::pin(async move {
            let target = protocol_session(&command.target)?;
            self.delivery
                .cancel_active_turn(target, provider_actor(&command.actor)?)
                .await
                .map_err(|error| match error {
                    ProviderCancelActiveTurnError::SessionNotFound => {
                        CommandFailure::SessionNotFound
                    }
                    ProviderCancelActiveTurnError::NoActiveTurn => CommandFailure::NoActiveTurn,
                    ProviderCancelActiveTurnError::Unavailable => CommandFailure::ProviderFailure,
                    ProviderCancelActiveTurnError::Operation(failure) => {
                        map_operation_failure(*failure)
                    }
                })?;
            Ok(())
        })
    }

    fn close(&self, command: SessionTargetCommand) -> CommandFuture<'_, ()> {
        Box::pin(async move {
            let target = protocol_session(&command.target)?;
            let stored = self.stored_session(&target).await?;
            let operation_id = OperationId::generate();
            self.supervisor
                .close(ConversationCloseRequest {
                    operation_id: operation_id.clone(),
                    target,
                    generation: None,
                    requested_by: provider_actor(&command.actor)?,
                    approver: stored.approver,
                })
                .await
                .map_err(map_operation_failure)?;
            match wait_for_settlement(self.supervisor.as_ref(), operation_id).await? {
                ConversationOperationSettlement::Closed { .. } => Ok(()),
                _ => Err(CommandFailure::ProviderFailure),
            }
        })
    }

    fn set_setting(&self, command: SetSessionSettingCommand) -> CommandFuture<'_, ()> {
        Box::pin(async move {
            let setting = match command.setting_id.as_str() {
                "mode" => ProviderSettingName::Mode,
                "model" => ProviderSettingName::Model,
                "effort" => ProviderSettingName::Effort,
                _ => {
                    return Err(CommandFailure::InvalidSetting {
                        advertised: vec!["mode".into(), "model".into(), "effort".into()],
                    });
                }
            };
            self.supervisor
                .settings_set(ProviderSettingsSetRequest {
                    target: protocol_session(&command.target)?,
                    actor: provider_actor(&command.actor)?,
                    setting,
                    value: command.value,
                })
                .await
                .map_err(|error| match error.kind {
                    ProviderSettingsFailureKind::WrongActor => CommandFailure::UnauthorizedActor,
                    ProviderSettingsFailureKind::NotFound => CommandFailure::SessionNotFound,
                    ProviderSettingsFailureKind::Busy => CommandFailure::Busy,
                    ProviderSettingsFailureKind::InvalidSetting => CommandFailure::InvalidSetting {
                        advertised: error.advertised,
                    },
                    _ => CommandFailure::ProviderFailure,
                })?;
            Ok(())
        })
    }
}

fn default_policy() -> ProviderRequestedPolicy {
    ProviderRequestedPolicy {
        access: RouterAccess::WriteRestricted,
    }
}

fn working_directory(path: &std::path::Path) -> Result<ProviderWorkingDirectory, CommandFailure> {
    let path = path.to_str().ok_or(CommandFailure::ProviderFailure)?;
    ProviderWorkingDirectory::try_from(path.to_owned()).map_err(|_| CommandFailure::ProviderFailure)
}

fn protocol_endpoint(endpoint: &SessionEndpointRef) -> Result<EndpointRef, CommandFailure> {
    Ok(EndpointRef {
        service_id: UuidIdentity::try_from(endpoint.service_id.as_str().to_owned())
            .map_err(|_| CommandFailure::ProviderFailure)?,
        endpoint_id: EndpointId::try_from(endpoint.endpoint_id.as_str().to_owned())
            .map_err(|_| CommandFailure::ProviderFailure)?,
    })
}

fn protocol_session(session: &message_board::SessionRef) -> Result<SessionRef, CommandFailure> {
    Ok(SessionRef {
        endpoint: protocol_endpoint(&session.endpoint)?,
        session_id: SessionId::try_from(session.session_id.as_str().to_owned())
            .map_err(|_| CommandFailure::ProviderFailure)?,
    })
}

fn board_session(session: SessionRef) -> Result<message_board::SessionRef, CommandFailure> {
    let identity = ProviderIdentity::Session(session)
        .to_board_identity()
        .map_err(|_| CommandFailure::ProviderFailure)?;
    match identity {
        Identity::Session { session } => Ok(session),
        Identity::Human { .. } => Err(CommandFailure::ProviderFailure),
    }
}

fn provider_actor(actor: &Identity) -> Result<ProviderIdentity, CommandFailure> {
    ProviderIdentity::from_board_identity(actor).map_err(|_| CommandFailure::UnauthorizedActor)
}

fn queued_input(queued: crate::ProviderQueuedInput) -> QueuedSessionInput {
    QueuedSessionInput {
        input_id: queued.input_id.as_str().to_owned(),
        position: queued.position,
        preview: queued.preview,
    }
}

fn map_admission_failure(error: ProviderQueueAdmissionError) -> CommandFailure {
    match error {
        ProviderQueueAdmissionError::SessionNotFound => CommandFailure::SessionNotFound,
        ProviderQueueAdmissionError::SettingsUnresolved => CommandFailure::SettingsUnresolved,
        ProviderQueueAdmissionError::UnsupportedContent { content_type } => {
            CommandFailure::UnsupportedContent(content_type)
        }
        ProviderQueueAdmissionError::Busy => CommandFailure::Busy,
        _ => CommandFailure::ProviderFailure,
    }
}

fn map_prompt_failure(error: ProviderPromptContentsError) -> CommandFailure {
    match error {
        ProviderPromptContentsError::Admission(error) => map_admission_failure(error),
        ProviderPromptContentsError::Operation(error) => map_operation_failure(*error),
    }
}

fn map_steer_failure(error: ProviderSteerContentsError) -> CommandFailure {
    match error {
        ProviderSteerContentsError::Admission(error) => map_admission_failure(error),
        ProviderSteerContentsError::Runtime(error) => map_runtime_failure(error),
    }
}

fn map_runtime_failure(error: ExternalProviderRuntimeError) -> CommandFailure {
    match error {
        ExternalProviderRuntimeError::LocalBusy => CommandFailure::Busy,
        ExternalProviderRuntimeError::LocalNotFound
        | ExternalProviderRuntimeError::ProviderSessionNotFound { .. } => {
            CommandFailure::SessionNotFound
        }
        ExternalProviderRuntimeError::SettingsUnresolved => CommandFailure::SettingsUnresolved,
        ExternalProviderRuntimeError::UnsupportedContent { content_type } => {
            CommandFailure::UnsupportedContent(content_type)
        }
        ExternalProviderRuntimeError::UnsupportedCapability { capability } => {
            CommandFailure::UnsupportedOperation {
                operation: capability,
            }
        }
        _ => CommandFailure::ProviderFailure,
    }
}

fn map_operation_failure(error: ConversationOperationFailure) -> CommandFailure {
    match error.kind {
        ConversationOperationFailureKind::NotFound
        | ConversationOperationFailureKind::ProviderSessionNotFound => {
            CommandFailure::SessionNotFound
        }
        ConversationOperationFailureKind::Busy => CommandFailure::Busy,
        ConversationOperationFailureKind::UnsupportedCapability => CommandFailure::Unsupported,
        ConversationOperationFailureKind::SettingsUnresolved => CommandFailure::SettingsUnresolved,
        ConversationOperationFailureKind::InvalidSetting => CommandFailure::InvalidSetting {
            advertised: error
                .invalid_setting
                .map_or_else(Vec::new, |setting| setting.advertised),
        },
        _ => CommandFailure::ProviderFailure,
    }
}

async fn wait_for_settlement(
    supervisor: &ExternalProviderSupervisor,
    operation_id: OperationId,
) -> Result<ConversationOperationSettlement, CommandFailure> {
    let timeout_seconds = PositiveSeconds::try_from(OPERATION_WAIT_SECONDS)
        .map_err(|_| CommandFailure::ProviderFailure)?;
    loop {
        let result = supervisor
            .wait(ConversationOperationWaitRequest {
                operation_id: operation_id.clone(),
                timeout_seconds,
            })
            .await
            .map_err(map_operation_failure)?;
        match result.output {
            ConversationOperationWaitOutput::Available { settlement } => return Ok(settlement),
            ConversationOperationWaitOutput::Pending => {}
            ConversationOperationWaitOutput::OutputUnavailable { .. } => {
                return Err(CommandFailure::ProviderFailure);
            }
        }
    }
}

async fn wait_for_turn_start(
    hub: &ProviderSessionEventHub,
    session: message_board::SessionRef,
    mut attachment: SessionEventAttachment,
    input_id: InputId,
) -> Result<String, CommandFailure> {
    for event in &attachment.snapshot {
        if let SessionEvent::TurnStarted {
            turn_id,
            input_id: observed,
        } = &event.event
            && observed == &input_id
        {
            return Ok(turn_id.clone());
        }
    }
    let started = tokio::time::timeout(TURN_START_DEADLINE, async {
        loop {
            match attachment.receiver.recv().await {
                Ok(event) => {
                    if let SessionEvent::TurnStarted {
                        turn_id,
                        input_id: observed,
                    } = &event.event
                        && observed == &input_id
                    {
                        return Some(turn_id.clone());
                    }
                    if let SessionEvent::TurnEnded { .. } = event.event {
                        continue;
                    }
                }
                Err(_) => return None,
            }
        }
    })
    .await;
    if let Ok(Some(turn_id)) = started {
        return Ok(turn_id);
    }
    let refreshed = hub
        .attach(session)
        .await
        .map_err(|_| CommandFailure::ProviderFailure)?;
    if let Some(turn_id) = refreshed
        .snapshot
        .into_iter()
        .find_map(|event| match event.event {
            SessionEvent::TurnStarted {
                turn_id,
                input_id: observed,
            } if observed == input_id => Some(turn_id),
            _ => None,
        })
    {
        return Ok(turn_id);
    }
    Err(CommandFailure::ProviderFailure)
}

fn active_turn_id(attachment: &SessionEventAttachment) -> Option<&str> {
    attachment
        .snapshot
        .iter()
        .fold(None, |current, event| match &event.event {
            SessionEvent::TurnStarted { turn_id, .. } => Some(turn_id.as_str()),
            SessionEvent::TurnEnded { turn_id, .. } if current == Some(turn_id.as_str()) => None,
            _ => current,
        })
}

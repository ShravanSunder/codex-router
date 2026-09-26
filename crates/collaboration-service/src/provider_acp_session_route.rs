//! ACP route for Router-owned provider Sessions on the shared connection shell.
use crate::ServiceApprovalBroker;
use crate::provider_acp_event_projection::{ProviderSessionObservers, stream_prompt};
use crate::provider_acp_interaction::{
    OutboundInteraction, apply_interaction_reply, present_interaction,
};
use crate::{
    CommandContent, CommandFailure, CreateSessionCommand, PromptSessionCommand, QueueInputCommand,
    SessionCommandPort, SessionEventHub, SessionSettingsCommand, SessionSteerOutcome,
    SessionTargetCommand, SteerSessionCommand,
};
use codex_acp_adapter::{
    AcpConnectionContext, AcpRouteFuture, AcpRouterChannels, AcpSchemaCatalog, AcpSessionRoute,
};
use message_board::{Identity, SessionEndpointRef, SessionRef};
use serde_json::{Value, json};
use session_event_model::session_profile_codec::ProfileElement;
use session_event_model::{CapabilityReport, SessionEvent, SessionState};
use std::{collections::HashMap, io, path::PathBuf, sync::Arc};
use tokio::{sync::mpsc, task::JoinSet};

pub struct ProviderAcpSessionRoute {
    endpoint: SessionEndpointRef,
    commands: Arc<dyn SessionCommandPort>,
    events: Arc<dyn SessionEventHub>,
    interaction_broker: Option<Arc<ServiceApprovalBroker>>,
}

impl ProviderAcpSessionRoute {
    pub fn new(
        endpoint: SessionEndpointRef,
        commands: Arc<dyn SessionCommandPort>,
        events: Arc<dyn SessionEventHub>,
    ) -> Self {
        Self {
            endpoint,
            commands,
            events,
            interaction_broker: None,
        }
    }

    #[must_use]
    pub fn with_interaction_broker(mut self, broker: Arc<ServiceApprovalBroker>) -> Self {
        self.interaction_broker = Some(broker);
        self
    }
}

impl AcpSessionRoute for ProviderAcpSessionRoute {
    fn endpoint_id(&self) -> &str {
        self.endpoint.endpoint_id.as_str()
    }

    fn run(
        self: Box<Self>,
        router: AcpRouterChannels,
        context: AcpConnectionContext,
    ) -> AcpRouteFuture {
        let supports_state = context
            .client_profile
            .as_ref()
            .is_some_and(|profile| profile.supports(ProfileElement::State).is_ok());
        Box::pin(serve_provider_sessions(
            *self,
            router,
            context.actor,
            supports_state,
            context.client_supports_elicitation_form,
        ))
    }
}

pub(crate) fn response(id: Value, result: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"result":result})
}

pub(crate) fn failure(id: Value, code: i32, message: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
}

fn command_failure(id: Value, error: CommandFailure) -> Value {
    let code = match error {
        CommandFailure::SessionNotFound => -32002,
        CommandFailure::Unsupported | CommandFailure::UnsupportedOperation { .. } => -32601,
        CommandFailure::UnauthorizedActor
        | CommandFailure::InvalidSetting { .. }
        | CommandFailure::UnsupportedContent(_) => -32602,
        _ => -32000,
    };
    failure(id, code, &error.to_string())
}

fn named_session(params: &Value, endpoint: &SessionEndpointRef) -> Result<SessionRef, ()> {
    if let Some(reference) = params.pointer("/_meta/router/sessionRef") {
        let session: SessionRef = serde_json::from_value(reference.clone()).map_err(|_| ())?;
        return (session.endpoint == *endpoint).then_some(session).ok_or(());
    }
    let session_id = params.get("sessionId").and_then(Value::as_str).ok_or(())?;
    serde_json::from_value(json!({"endpoint":endpoint,"sessionId":session_id})).map_err(|_| ())
}

fn content_blocks(params: &Value) -> Result<Vec<CommandContent>, ()> {
    let blocks = params.get("prompt").and_then(Value::as_array).ok_or(())?;
    blocks
        .iter()
        .map(|block| match block.get("type").and_then(Value::as_str) {
            Some("text") => block
                .get("text")
                .and_then(Value::as_str)
                .map(|text| CommandContent::Text(text.to_owned()))
                .ok_or(()),
            Some("resource_link") => Ok(CommandContent::ResourceLink {
                uri: block
                    .get("uri")
                    .and_then(Value::as_str)
                    .ok_or(())?
                    .to_owned(),
                name: block
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or(())?
                    .to_owned(),
            }),
            _ => Err(()),
        })
        .collect()
}

async fn session_result(
    events: &dyn SessionEventHub,
    session: &SessionRef,
    capabilities: CapabilityReport,
    history_unavailable: bool,
) -> Result<Value, ()> {
    let summary = events
        .sessions(session.endpoint.clone())
        .await
        .map_err(|_| ())?
        .into_iter()
        .find(|summary| summary.session == *session)
        .ok_or(())?;
    Ok(json!({
        "sessionId":session.session_id.as_str(),
        "_meta":{
            "router":{"sessionRef":session,"approver":summary.approver},
            "sessionProfile":{"capabilities":capabilities,"historyUnavailable":history_unavailable}
        }
    }))
}

async fn current_turn_id(events: &dyn SessionEventHub, session: SessionRef) -> Option<String> {
    let attachment = events.attach(session).await.ok()?;
    attachment
        .snapshot
        .into_iter()
        .fold(None, |current, event| match event.event {
            SessionEvent::TurnStarted { turn_id, .. } => Some(turn_id),
            SessionEvent::TurnEnded { turn_id, .. } if current.as_deref() == Some(&turn_id) => None,
            _ => current,
        })
}

async fn validate_load_request(
    schema: &mut AcpSchemaCatalog,
    events: &dyn SessionEventHub,
    session: &SessionRef,
    params: &Value,
) -> Result<(), (i32, &'static str)> {
    if !schema
        .validate("LoadSessionRequest", params)
        .unwrap_or(false)
    {
        return Err((-32602, "Invalid session load parameters"));
    }
    let summaries = events
        .sessions(session.endpoint.clone())
        .await
        .map_err(|_| (-32000, "Session inventory unavailable"))?;
    let stored = summaries
        .into_iter()
        .find(|summary| summary.session == *session)
        .ok_or((-32002, "Session not found"))?;
    if params.get("cwd").and_then(Value::as_str) != stored.working_directory.to_str() {
        return Err((-32602, "Session working directory does not match"));
    }
    Ok(())
}

async fn list_sessions_result(
    id: Value,
    params: &Value,
    schema: &mut AcpSchemaCatalog,
    events: &dyn SessionEventHub,
    endpoint: SessionEndpointRef,
) -> Value {
    if !schema
        .validate("ListSessionsRequest", params)
        .unwrap_or(false)
    {
        return failure(id, -32602, "Invalid session list parameters");
    }
    match events.sessions(endpoint).await {
        Ok(summaries) => response(
            id,
            json!({
                "sessions":summaries.into_iter().map(|summary| json!({
                    "sessionId":summary.session.session_id.as_str(),
                    "cwd":summary.working_directory,
                    "title":summary.name,
                    "_meta":{"router":{"sessionRef":summary.session,"approver":summary.approver},
                        "sessionProfile":{"state":summary.state}}
                })).collect::<Vec<_>>()
            }),
        ),
        Err(_) => failure(id, -32000, "Session inventory unavailable"),
    }
}

async fn serve_provider_sessions(
    route: ProviderAcpSessionRoute,
    mut router: AcpRouterChannels,
    actor: Option<Identity>,
    supports_state: bool,
    supports_question_form: bool,
) -> io::Result<()> {
    let mut schema = AcpSchemaCatalog::load().map_err(io::Error::other)?;
    let mut prompts = JoinSet::<()>::new();
    let (interaction_sender, mut interaction_receiver) = mpsc::channel(64);
    let mut pending_interactions =
        HashMap::<String, session_event_model::PendingInteraction>::new();
    let mut observers = ProviderSessionObservers::new(
        Arc::clone(&route.events),
        router.output.clone(),
        supports_state,
        interaction_sender,
    );
    loop {
        let frame = tokio::select! {
            _ = router.closed.cancelled() => break,
            completed = prompts.join_next(), if !prompts.is_empty() => {
                if completed.is_some_and(|result| result.is_err()) {
                    return Err(io::Error::other("provider ACP prompt task failed"));
                }
                continue;
            },
            completed = observers.tasks.join_next(), if !observers.tasks.is_empty() => {
                if completed.is_some_and(|result| result.is_err()) {
                    return Err(io::Error::other("provider ACP observer task failed"));
                }
                continue;
            },
            interaction = interaction_receiver.recv() => {
                let Some((session, interaction)) = interaction else { break; };
                if let (Some(actor), Some(_broker)) = (actor.as_ref(), route.interaction_broker.as_ref())
                    && let Some(OutboundInteraction { request_id, frame, pending }) =
                        present_interaction(&session, interaction, actor, supports_question_form)
                    && !pending_interactions.contains_key(&request_id) {
                    pending_interactions.insert(request_id, pending);
                    router.output.send(frame).await?;
                }
                continue;
            },
            frame = router.input.recv() => match frame { Some(frame) => frame, None => break },
        };
        let method = frame.get("method").and_then(Value::as_str).unwrap_or("");
        let params = frame.get("params").cloned().unwrap_or_else(|| json!({}));
        let id = frame.get("id").cloned();
        if frame.get("method").is_none() {
            if let (Some(id), Some(actor), Some(broker)) = (
                id.as_ref(),
                actor.as_ref(),
                route.interaction_broker.as_ref(),
            ) && let Some(interaction) = id
                .as_str()
                .and_then(|request_id| pending_interactions.remove(request_id))
            {
                let _decision = apply_interaction_reply(broker, actor, &interaction, &frame).await;
            }
            continue;
        }
        if method == "session/cancel" && id.is_none() {
            if let (Some(actor), Ok(session)) =
                (actor.clone(), named_session(&params, &route.endpoint))
            {
                let _cancelled = route
                    .commands
                    .cancel(SessionTargetCommand {
                        target: session,
                        actor,
                    })
                    .await;
            }
            continue;
        }
        let Some(id) = id else {
            continue;
        };
        if method == "session/list" {
            router
                .output
                .send(
                    list_sessions_result(
                        id,
                        &params,
                        &mut schema,
                        route.events.as_ref(),
                        route.endpoint.clone(),
                    )
                    .await,
                )
                .await?;
            continue;
        }
        let Some(actor) = actor.clone() else {
            router
                .output
                .send(failure(id, -32602, "ACP actor identity is required"))
                .await?;
            continue;
        };
        let result = match method {
            "session/new" => {
                if !schema
                    .validate("NewSessionRequest", &params)
                    .unwrap_or(false)
                {
                    failure(id, -32602, "Invalid session creation parameters")
                } else {
                    let cwd = params.get("cwd").and_then(Value::as_str).unwrap_or("");
                    if !PathBuf::from(cwd).is_absolute() {
                        failure(id, -32602, "Session working directory must be absolute")
                    } else {
                        match route
                            .commands
                            .create(CreateSessionCommand {
                                endpoint: route.endpoint.clone(),
                                working_directory: PathBuf::from(cwd),
                                settings: SessionSettingsCommand::default(),
                                actor,
                            })
                            .await
                        {
                            Ok(session) => {
                                match observers.attach(session.clone(), false, false).await {
                                    Ok(report) => match session_result(
                                        route.events.as_ref(),
                                        &session,
                                        report,
                                        false,
                                    )
                                    .await
                                    {
                                        Ok(result) => response(id, result),
                                        Err(()) => {
                                            failure(id, -32000, "Session inventory unavailable")
                                        }
                                    },
                                    Err(()) => failure(id, -32000, "Session event hub unavailable"),
                                }
                            }
                            Err(error) => command_failure(id, error),
                        }
                    }
                }
            }
            "session/load" | "session/resume" => match named_session(&params, &route.endpoint) {
                Err(()) => failure(id, -32602, "Invalid SessionRef"),
                Ok(session) => {
                    if method == "session/load"
                        && let Err((code, message)) = validate_load_request(
                            &mut schema,
                            route.events.as_ref(),
                            &session,
                            &params,
                        )
                        .await
                    {
                        router.output.send(failure(id, code, message)).await?;
                        continue;
                    }
                    let state = route.events.state(session.clone()).await;
                    match state {
                        Err(_) => failure(id, -32002, "Session not found"),
                        Ok(state) => {
                            let activation = if state == SessionState::Unloaded {
                                let command = SessionTargetCommand {
                                    target: session.clone(),
                                    actor,
                                };
                                if method == "session/load" {
                                    route.commands.load_session(command).await
                                } else {
                                    route.commands.resume_session(command).await
                                }
                            } else {
                                Ok(())
                            };
                            match activation {
                                Err(error) => command_failure(id, error),
                                Ok(()) => {
                                    match observers
                                        .attach(session.clone(), method == "session/load", true)
                                        .await
                                    {
                                        Ok(report) => match session_result(
                                            route.events.as_ref(),
                                            &session,
                                            report,
                                            method == "session/resume",
                                        )
                                        .await
                                        {
                                            Ok(result) => response(id, result),
                                            Err(()) => {
                                                failure(id, -32000, "Session inventory unavailable")
                                            }
                                        },
                                        Err(()) => {
                                            failure(id, -32000, "Session event hub unavailable")
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            },
            "session/prompt" => {
                match (
                    named_session(&params, &route.endpoint),
                    content_blocks(&params),
                ) {
                    (Ok(session), Ok(content)) => {
                        let attachment = route.events.attach(session.clone()).await;
                        match attachment {
                            Err(_) => failure(id, -32002, "Session not found"),
                            Ok(attachment) => {
                                let command = PromptSessionCommand {
                                    target: session.clone(),
                                    content,
                                    actor,
                                };
                                match route.commands.prompt(command).await {
                                    Err(error) => command_failure(id, error),
                                    Ok(turn) => {
                                        let output = router.output.clone();
                                        let Some(observed_turn) =
                                            observers.observed_turns.get(&session).cloned()
                                        else {
                                            router
                                                .output
                                                .send(failure(
                                                    id,
                                                    -32000,
                                                    "Session observer unavailable",
                                                ))
                                                .await?;
                                            continue;
                                        };
                                        prompts.spawn(async move {
                                            stream_prompt(
                                                output,
                                                id,
                                                turn.turn_id,
                                                attachment,
                                                observed_turn,
                                            )
                                            .await;
                                        });
                                        continue;
                                    }
                                }
                            }
                        }
                    }
                    _ => failure(id, -32602, "Invalid Session prompt"),
                }
            }
            "_session/steering" => {
                match (
                    named_session(&params, &route.endpoint),
                    content_blocks(&params),
                ) {
                    (Ok(session), Ok(content)) => {
                        match current_turn_id(route.events.as_ref(), session.clone()).await {
                            None => failure(id, -32000, "No running turn to steer"),
                            Some(expected_turn_id) => match route
                                .commands
                                .steer(SteerSessionCommand {
                                    target: session,
                                    expected_turn_id,
                                    content,
                                    actor,
                                })
                                .await
                            {
                                Ok(SessionSteerOutcome::Injected { turn_id }) => {
                                    response(id, json!({"outcome":"injected","turnId":turn_id}))
                                }
                                Ok(SessionSteerOutcome::StartedNewTurn { turn_id }) => response(
                                    id,
                                    json!({"outcome":"startedNewTurn","turnId":turn_id}),
                                ),
                                Ok(SessionSteerOutcome::PromptRequired) => {
                                    response(id, json!({"outcome":"promptRequired"}))
                                }
                                Ok(SessionSteerOutcome::Failed { reason }) => {
                                    response(id, json!({"outcome":"failed","reason":reason}))
                                }
                                Err(error) => command_failure(id, error),
                            },
                        }
                    }
                    _ => failure(id, -32602, "Invalid steering request"),
                }
            }
            "_session/queue/add" => {
                match (
                    named_session(&params, &route.endpoint),
                    content_blocks(&params),
                ) {
                    (Ok(session), Ok(content)) => match route
                        .commands
                        .queue_add(QueueInputCommand {
                            target: session,
                            content,
                            actor,
                        })
                        .await
                    {
                        Ok(queued) => response(
                            id,
                            json!({"inputId":queued.input_id,"position":queued.position}),
                        ),
                        Err(error) => command_failure(id, error),
                    },
                    _ => failure(id, -32602, "Invalid queued input"),
                }
            }
            "_session/queue/list" => match named_session(&params, &route.endpoint) {
                Ok(session) => match route
                    .commands
                    .queue_list(SessionTargetCommand {
                        target: session,
                        actor,
                    })
                    .await
                {
                    Ok(items) => response(
                        id,
                        json!({"items":items.into_iter().map(|item| json!({
                        "inputId":item.input_id,"position":item.position,"preview":item.preview
                    })).collect::<Vec<_>>()}),
                    ),
                    Err(error) => command_failure(id, error),
                },
                Err(()) => failure(id, -32602, "Invalid SessionRef"),
            },
            "_session/queue/cancel" => match (
                named_session(&params, &route.endpoint),
                params.get("inputId").and_then(Value::as_str),
            ) {
                (Ok(session), Some(input_id)) => match route
                    .commands
                    .queue_cancel(
                        SessionTargetCommand {
                            target: session,
                            actor,
                        },
                        input_id.to_owned(),
                    )
                    .await
                {
                    Ok(()) => response(id, json!({"cancelled":true})),
                    Err(error) => command_failure(id, error),
                },
                _ => failure(id, -32602, "Invalid queue cancellation"),
            },
            _ => failure(id, -32601, "ACP method not supported"),
        };
        router.output.send(result).await?;
    }
    prompts.abort_all();
    while prompts.join_next().await.is_some() {}
    observers.tasks.abort_all();
    while observers.tasks.join_next().await.is_some() {}
    Ok(())
}

//! Codex app-server protocol transport for Router-owned provider sessions.
use crate::{
    CommandContent, CreateSessionCommand, HubEvent, HubSessionSummary, PromptSessionCommand,
    SessionCommandPort, SessionEventHub, SessionEventHubError, SessionSettingsCommand,
    SessionSteerOutcome, SessionTargetCommand, SteerSessionCommand,
    app_server_model_catalog::{ProviderModelEntry, render_model_list},
    private_socket_listener::PrivateSocketListener,
};
use futures_util::{SinkExt, StreamExt};
use message_board::{Identity, SessionEndpointRef, SessionRef};
use serde_json::{Value, json};
use session_event_model::{SessionEvent, StopReason, TurnOutcome};
use std::{
    collections::HashSet,
    io,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::{
    net::UnixStream,
    sync::{Semaphore, mpsc, watch},
    task::JoinSet,
};
use tokio_tungstenite::{accept_async, tungstenite::Message};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub struct RouterSessionAppServerContext {
    endpoint: SessionEndpointRef,
    actor: Identity,
    commands: Arc<dyn SessionCommandPort>,
    events: Arc<dyn SessionEventHub>,
    model_catalog: watch::Receiver<Vec<ProviderModelEntry>>,
}

impl RouterSessionAppServerContext {
    pub fn new(
        endpoint: SessionEndpointRef,
        actor: Identity,
        commands: Arc<dyn SessionCommandPort>,
        events: Arc<dyn SessionEventHub>,
    ) -> Self {
        let (_, model_catalog) = watch::channel(Vec::new());
        Self {
            endpoint,
            actor,
            commands,
            events,
            model_catalog,
        }
    }

    #[must_use]
    pub fn with_model_catalog(
        mut self,
        model_catalog: watch::Receiver<Vec<ProviderModelEntry>>,
    ) -> Self {
        self.model_catalog = model_catalog;
        self
    }
}

pub struct RouterSessionAppServerListener {
    socket: PrivateSocketListener,
    context: Arc<RouterSessionAppServerContext>,
    permits: Arc<Semaphore>,
}

impl RouterSessionAppServerListener {
    /// The caller selects a per-provider path under its private router-sessions
    /// directory. The existing socket path is never replaced.
    pub fn bind(
        socket_path: &Path,
        context: Arc<RouterSessionAppServerContext>,
    ) -> io::Result<Self> {
        Ok(Self {
            socket: PrivateSocketListener::bind(socket_path)?,
            context,
            permits: Arc::new(Semaphore::new(32)),
        })
    }

    pub async fn run(self, shutdown: CancellationToken) -> io::Result<()> {
        let mut connections = JoinSet::new();
        let result = loop {
            tokio::select! {
                _ = shutdown.cancelled() => break Ok(()),
                completed = connections.join_next(), if !connections.is_empty() => {
                    let _completed = completed;
                },
                accepted = self.socket.listener.accept() => {
                    let (stream, _) = match accepted {
                        Ok(pair) => pair,
                        Err(error) => break Err(error),
                    };
                    let Ok(permit) = Arc::clone(&self.permits).try_acquire_owned() else {
                        continue;
                    };
                    let context = Arc::clone(&self.context);
                    connections.spawn(async move {
                        let _permit = permit;
                        let _result = serve_router_session_app_server_connection(stream, context).await;
                    });
                }
            }
        };
        connections.abort_all();
        while connections.join_next().await.is_some() {}
        result
    }
}

#[derive(Debug, thiserror::Error)]
enum ThreadMethodError {
    #[error("invalid thread parameters")]
    InvalidParams,
    #[error("thread not found")]
    NotFound,
    #[error("provider session unavailable")]
    Unavailable,
}

#[derive(Debug, thiserror::Error)]
pub enum AppServerConnectionError {
    #[error("websocket connection failed")]
    WebSocket(#[from] tokio_tungstenite::tungstenite::Error),
    #[error("invalid JSON frame")]
    Json(#[from] serde_json::Error),
    #[error("session event hub unavailable")]
    HubUnavailable,
}

pub async fn serve_router_session_app_server_connection(
    stream: UnixStream,
    context: Arc<RouterSessionAppServerContext>,
) -> Result<(), AppServerConnectionError> {
    let mut websocket = accept_async(stream).await?;
    let (event_sender, mut event_receiver) =
        mpsc::channel::<(SessionRef, Result<HubEvent, ()>)>(64);
    let mut event_tasks = JoinSet::new();
    let mut attached_sessions = HashSet::new();
    loop {
        let frame = tokio::select! {
            incoming = websocket.next() => incoming,
            forwarded = event_receiver.recv(), if !event_tasks.is_empty() => {
                let Some((session, event)) = forwarded else { break; };
                let Ok(event) = event else { break; };
                if let Some(notification) = render_turn_event(&session, &event) {
                    websocket.send(Message::Text(notification.to_string().into())).await?;
                }
                continue;
            }
        };
        let Some(frame) = frame else {
            break;
        };
        let frame = frame?;
        if matches!(frame, Message::Close(_)) {
            break;
        }
        let Message::Text(text) = frame else {
            continue;
        };
        let request: Value = serde_json::from_str(&text)?;
        let Some(id) = request.get("id").cloned() else {
            continue;
        };
        let method = request.get("method").and_then(Value::as_str).unwrap_or("");
        let response = if matches!(
            method,
            "thread/start"
                | "thread/list"
                | "thread/read"
                | "thread/resume"
                | "turn/start"
                | "turn/steer"
                | "turn/interrupt"
        ) {
            let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
            let result = if method.starts_with("thread/") {
                handle_app_server_thread_request(
                    method,
                    params,
                    Arc::clone(&context.commands),
                    Arc::clone(&context.events),
                    context.endpoint.clone(),
                    context.actor.clone(),
                )
                .await
            } else {
                handle_app_server_turn_request(
                    method,
                    params,
                    Arc::clone(&context.commands),
                    Arc::clone(&context.events),
                    context.endpoint.clone(),
                    context.actor.clone(),
                )
                .await
            };
            match result {
                Ok(result) => json!({"id":id,"result":result}),
                Err(error) => {
                    let code = match error {
                        ThreadMethodError::InvalidParams => -32602,
                        ThreadMethodError::NotFound => -32002,
                        ThreadMethodError::Unavailable => -32000,
                    };
                    json!({"id":id,"error":{"code":code,"message":error.to_string()}})
                }
            }
        } else {
            handle_app_server_request(id, method, &context.model_catalog.borrow())
        };
        websocket
            .send(Message::Text(response.to_string().into()))
            .await?;
        if matches!(method, "thread/start" | "thread/read" | "thread/resume")
            && response.get("result").is_some()
            && let Some(thread_id) = response
                .pointer("/result/thread/id")
                .and_then(Value::as_str)
        {
            let session = context
                .events
                .sessions(context.endpoint.clone())
                .await
                .map_err(|_| AppServerConnectionError::HubUnavailable)?
                .into_iter()
                .find(|summary| thread_alias(&summary.session) == thread_id)
                .map(|summary| summary.session);
            if let Some(session) = session
                && attached_sessions.insert(session.clone())
            {
                let attachment = context
                    .events
                    .attach(session.clone())
                    .await
                    .map_err(|_| AppServerConnectionError::HubUnavailable)?;
                for event in attachment.snapshot {
                    if let Some(notification) = render_turn_event(&session, &event) {
                        websocket
                            .send(Message::Text(notification.to_string().into()))
                            .await?;
                    }
                }
                let mut receiver = attachment.receiver;
                let sender = event_sender.clone();
                event_tasks.spawn(async move {
                    loop {
                        let event = match receiver.recv().await {
                            Ok(event) => Ok(event),
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => Err(()),
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                        };
                        let lost_events = event.is_err();
                        if sender.send((session.clone(), event)).await.is_err() || lost_events {
                            break;
                        }
                    }
                });
            }
        }
    }
    event_tasks.abort_all();
    while event_tasks.join_next().await.is_some() {}
    Ok(())
}

fn handle_app_server_request(id: Value, method: &str, catalog: &[ProviderModelEntry]) -> Value {
    let result = match method {
        "initialize" => Some(json!({})),
        "account/read" => Some(json!({"account":null,"requiresOpenaiAuth":false})),
        "model/list" => Some(render_model_list(catalog)),
        "configRequirements/read" => Some(json!({"requirements":null})),
        "collaborationMode/list" | "hooks/list" | "skills/list" => {
            Some(json!({"data":[],"nextCursor":null}))
        }
        _ => None,
    };
    match result {
        Some(result) => json!({"id":id,"result":result}),
        None => json!({"id":id,"error":{"code":-32601,"message":"Method not found"}}),
    }
}

async fn handle_app_server_thread_request(
    method: &str,
    params: Value,
    commands: Arc<dyn SessionCommandPort>,
    events: Arc<dyn SessionEventHub>,
    endpoint: SessionEndpointRef,
    actor: Identity,
) -> Result<Value, ThreadMethodError> {
    match method {
        "thread/start" => {
            if params.get("ephemeral").and_then(Value::as_bool) == Some(true)
                || params
                    .pointer("/threadSource/feature")
                    .and_then(Value::as_str)
                    == Some("thread_title")
            {
                return Err(ThreadMethodError::InvalidParams);
            }
            let working_directory = match params.get("cwd").and_then(Value::as_str) {
                Some(cwd) => PathBuf::from(cwd),
                None => std::env::current_dir().map_err(|_| ThreadMethodError::Unavailable)?,
            };
            if !working_directory.is_absolute() {
                return Err(ThreadMethodError::InvalidParams);
            }
            let requested_model = params.get("model").and_then(Value::as_str);
            let model_override = requested_model.filter(|model| *model != "provider-default");
            let session = commands
                .create(CreateSessionCommand {
                    endpoint: endpoint.clone(),
                    working_directory,
                    settings: SessionSettingsCommand {
                        model: model_override.map(str::to_owned),
                        ..SessionSettingsCommand::default()
                    },
                    actor,
                })
                .await
                .map_err(|_| ThreadMethodError::Unavailable)?;
            let summary = events
                .sessions(endpoint)
                .await
                .map_err(|_| ThreadMethodError::Unavailable)?
                .into_iter()
                .find(|summary| summary.session == session)
                .ok_or(ThreadMethodError::Unavailable)?;
            Ok(thread_start_response(&summary))
        }
        "thread/list" => {
            let summaries = events
                .sessions(endpoint)
                .await
                .map_err(|_| ThreadMethodError::Unavailable)?;
            Ok(json!({
                "data": summaries.iter().map(render_thread).collect::<Vec<_>>(),
                "nextCursor": null, "backwardsCursor": null
            }))
        }
        "thread/read" | "thread/resume" => {
            let thread_id = params
                .get("threadId")
                .and_then(Value::as_str)
                .ok_or(ThreadMethodError::InvalidParams)?;
            let summary = events
                .sessions(endpoint)
                .await
                .map_err(|_| ThreadMethodError::Unavailable)?
                .into_iter()
                .find(|summary| thread_alias(&summary.session) == thread_id)
                .ok_or(ThreadMethodError::NotFound)?;
            if method == "thread/resume" {
                let _attachment =
                    events
                        .attach(summary.session.clone())
                        .await
                        .map_err(|error| match error {
                            SessionEventHubError::SessionNotFound => ThreadMethodError::NotFound,
                            SessionEventHubError::Unavailable => ThreadMethodError::Unavailable,
                        })?;
                Ok(thread_start_response(&summary))
            } else {
                Ok(json!({"thread":render_thread(&summary)}))
            }
        }
        _ => Err(ThreadMethodError::InvalidParams),
    }
}

fn thread_alias(session: &message_board::SessionRef) -> String {
    let native_id = session.session_id.as_str();
    if let Ok(uuid) = Uuid::parse_str(native_id) {
        return uuid.hyphenated().to_string();
    }
    let name = format!(
        "codex-router:session:{}:{}:{}",
        session.endpoint.service_id.as_str(),
        session.endpoint.endpoint_id.as_str(),
        native_id
    );
    Uuid::new_v5(&Uuid::NAMESPACE_URL, name.as_bytes())
        .hyphenated()
        .to_string()
}

fn render_thread(summary: &HubSessionSummary) -> Value {
    let status = match &summary.state {
        session_event_model::SessionState::Unloaded | session_event_model::SessionState::Closed => {
            json!({"type":"notLoaded"})
        }
        session_event_model::SessionState::Idle => json!({"type":"idle"}),
        session_event_model::SessionState::Running => {
            json!({"type":"active","activeFlags":[]})
        }
        session_event_model::SessionState::RequiresAction { pending } => {
            let flag = match pending.first().kind() {
                session_event_model::InteractionKind::Approval => "waitingOnApproval",
                session_event_model::InteractionKind::Question => "waitingOnUserInput",
            };
            json!({"type":"active","activeFlags":[flag]})
        }
        session_event_model::SessionState::AuthenticationRequired => {
            json!({"type":"systemError"})
        }
    };
    let alias = thread_alias(&summary.session);
    json!({
        "id":alias,"sessionId":alias,
        "preview":summary.preview,"ephemeral":false,
        "modelProvider":summary.session.endpoint.endpoint_id.as_str(),
        "model":summary.model,
        // Router has no creation timestamp for provider Sessions, including legacy rows.
        "createdAt":summary.updated_at_seconds,
        "updatedAt":summary.updated_at_seconds,
        "status":status,
        "cwd":summary.working_directory,
        "cliVersion":env!("CARGO_PKG_VERSION"),
        "source":"cli",
        "name":summary.name,
        "turns":[]
    })
}

fn thread_start_response(summary: &HubSessionSummary) -> Value {
    let model = summary.model.as_deref().unwrap_or("provider-default");
    json!({
        "thread":render_thread(summary),
        "model":model,
        "modelProvider":summary.session.endpoint.endpoint_id.as_str(),
        "cwd":summary.working_directory,
        "approvalPolicy":"on-request",
        "approvalsReviewer":"user",
        "sandbox":{"type":"dangerFullAccess"},
        "reasoningEffort":null,
        "serviceTier":null,
        "collaborationMode":null
    })
}

async fn handle_app_server_turn_request(
    method: &str,
    params: Value,
    commands: Arc<dyn SessionCommandPort>,
    events: Arc<dyn SessionEventHub>,
    endpoint: SessionEndpointRef,
    actor: Identity,
) -> Result<Value, ThreadMethodError> {
    let thread_id = params
        .get("threadId")
        .and_then(Value::as_str)
        .ok_or(ThreadMethodError::InvalidParams)?;
    let session = events
        .sessions(endpoint)
        .await
        .map_err(|_| ThreadMethodError::Unavailable)?
        .into_iter()
        .find(|summary| thread_alias(&summary.session) == thread_id)
        .map(|summary| summary.session)
        .ok_or(ThreadMethodError::NotFound)?;
    match method {
        "turn/start" => {
            let content = parse_turn_input(&params)?;
            let handle = commands
                .prompt(PromptSessionCommand {
                    target: session,
                    content,
                    actor,
                })
                .await
                .map_err(|_| ThreadMethodError::Unavailable)?;
            Ok(json!({"turn":{
                "id":handle.turn_id,"items":[],"status":"inProgress",
                "error":null,"startedAt":null,"completedAt":null,"durationMs":null
            }}))
        }
        "turn/steer" => {
            let expected_turn_id = params
                .get("expectedTurnId")
                .and_then(Value::as_str)
                .ok_or(ThreadMethodError::InvalidParams)?
                .to_owned();
            let content = parse_turn_input(&params)?;
            let outcome = commands
                .steer(SteerSessionCommand {
                    target: session.clone(),
                    expected_turn_id,
                    content: content.clone(),
                    actor: actor.clone(),
                })
                .await
                .map_err(|_| ThreadMethodError::Unavailable)?;
            let turn_id = match outcome {
                SessionSteerOutcome::Injected { turn_id }
                | SessionSteerOutcome::StartedNewTurn { turn_id } => turn_id,
                SessionSteerOutcome::PromptRequired => {
                    commands
                        .prompt(PromptSessionCommand {
                            target: session,
                            content,
                            actor,
                        })
                        .await
                        .map_err(|_| ThreadMethodError::Unavailable)?
                        .turn_id
                }
                SessionSteerOutcome::Failed { .. } => return Err(ThreadMethodError::Unavailable),
            };
            Ok(json!({"turnId":turn_id}))
        }
        "turn/interrupt" => {
            if params.get("turnId").and_then(Value::as_str).is_none() {
                return Err(ThreadMethodError::InvalidParams);
            }
            commands
                .cancel(SessionTargetCommand {
                    target: session,
                    actor,
                })
                .await
                .map_err(|_| ThreadMethodError::Unavailable)?;
            Ok(json!({}))
        }
        _ => Err(ThreadMethodError::InvalidParams),
    }
}

fn parse_turn_input(params: &Value) -> Result<Vec<CommandContent>, ThreadMethodError> {
    let input = params
        .get("input")
        .and_then(Value::as_array)
        .ok_or(ThreadMethodError::InvalidParams)?;
    input
        .iter()
        .map(|entry| match entry.get("type").and_then(Value::as_str) {
            Some("text") => entry
                .get("text")
                .and_then(Value::as_str)
                .map(|text| CommandContent::Text(text.to_owned()))
                .ok_or(ThreadMethodError::InvalidParams),
            _ => Err(ThreadMethodError::InvalidParams),
        })
        .collect()
}

fn render_turn_event(session: &SessionRef, event: &HubEvent) -> Option<Value> {
    let thread_id = thread_alias(session);
    let (method, turn_id, status) = match &event.event {
        SessionEvent::TurnStarted { turn_id, .. } => ("turn/started", turn_id, "inProgress"),
        SessionEvent::TurnEnded { turn_id, outcome } => {
            let status = match outcome {
                TurnOutcome::Ended {
                    stop_reason: StopReason::Cancelled,
                    ..
                } => "interrupted",
                TurnOutcome::Ended { .. } => "completed",
                TurnOutcome::Lost { .. } => "failed",
            };
            ("turn/completed", turn_id, status)
        }
        _ => return None,
    };
    Some(json!({
        "method":method,
        "params":{
            "threadId":thread_id,
            "turn":{
                "id":turn_id,"items":[],"status":status,"error":null,
                "startedAt":null,"completedAt":null,"durationMs":null
            }
        }
    }))
}

#[cfg(test)]
#[path = "router_session_app_server_test_support.rs"]
mod test_support;
#[cfg(test)]
#[allow(clippy::panic_in_result_fn)]
#[path = "router_session_app_server_tests.rs"]
mod tests;

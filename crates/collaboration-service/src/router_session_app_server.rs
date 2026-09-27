//! Codex app-server protocol transport for Router-owned provider sessions.
use crate::{
    CommandContent, CreateSessionCommand, HubEvent, HubSessionSummary, PromptSessionCommand,
    SessionCommandPort, SessionEventAttachment, SessionEventHub, SessionEventHubError,
    SessionSettingsCommand, SessionSteerOutcome, SessionTargetCommand, SetSessionSettingCommand,
    SteerSessionCommand,
    app_server_event_forwarding::{AppServerEventForwarding, historical_turns},
    app_server_model_catalog::{ProviderModelEntry, render_model_list},
    pending_snapshot_interactions::pending_snapshot_requests,
    private_socket_listener::PrivateSocketListener,
};
use futures_util::{SinkExt, StreamExt};
use message_board::{Identity, SessionEndpointRef, SessionRef};
use serde_json::{Value, json};
use session_event_model::SessionEvent;
use std::{
    collections::{HashMap, HashSet},
    io,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::{
    net::UnixStream,
    sync::{Semaphore, mpsc, watch},
    task::{AbortHandle, JoinSet},
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
    interaction_broker: Option<Arc<crate::ServiceInteractionBroker>>,
    project_trust: Option<Arc<dyn codex_native_integration::CodexProjectTrustLookup>>,
}

impl RouterSessionAppServerContext {
    pub fn new(
        endpoint: SessionEndpointRef,
        actor: Identity,
        commands: Arc<dyn SessionCommandPort>,
        events: Arc<dyn SessionEventHub>,
        model_catalog: watch::Receiver<Vec<ProviderModelEntry>>,
    ) -> Self {
        Self {
            endpoint,
            actor,
            commands,
            events,
            model_catalog,
            interaction_broker: None,
            project_trust: None,
        }
    }

    #[must_use]
    pub fn with_interaction_broker(mut self, broker: Arc<crate::ServiceInteractionBroker>) -> Self {
        self.interaction_broker = Some(broker);
        self
    }

    #[must_use]
    pub fn with_project_trust(
        mut self,
        lookup: Arc<dyn codex_native_integration::CodexProjectTrustLookup>,
    ) -> Self {
        self.project_trust = Some(lookup);
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
    #[error("launch the Codex TUI with --cd <project> to choose where this provider session works")]
    WorkingDirectoryRequired,
    #[error("image input could not be read")]
    ImageUnreadable,
    #[error("image input exceeds the 1 MiB message limit")]
    ImageTooLarge,
    #[error("turn input exceeds the 1 MiB message limit")]
    PromptTooLarge,
    #[error("image input type is unsupported")]
    ImageUnsupportedType,
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
    #[error("app-server request worker unavailable")]
    RequestWorkerUnavailable,
}

struct HandledAppServerRequest {
    method: String,
    response: Value,
    resume_attachment: Option<(SessionRef, SessionEventAttachment)>,
}

pub async fn serve_router_session_app_server_connection(
    stream: UnixStream,
    context: Arc<RouterSessionAppServerContext>,
) -> Result<(), AppServerConnectionError> {
    let mut websocket = accept_async(stream).await?;
    let (event_sender, mut event_receiver) =
        mpsc::channel::<(SessionRef, Result<HubEvent, ()>)>(64);
    let (response_sender, mut response_receiver) =
        mpsc::channel::<Result<HandledAppServerRequest, AppServerConnectionError>>(32);
    let mut event_tasks = JoinSet::new();
    let mut request_tasks = JoinSet::new();
    let mut attached_sessions = HashSet::new();
    let mut subscriptions = HashMap::<SessionRef, AbortHandle>::new();
    let mut forwarding =
        AppServerEventForwarding::new(context.actor.clone(), context.interaction_broker.clone());
    loop {
        let frame = tokio::select! {
            incoming = websocket.next() => incoming,
            completed = event_tasks.join_next(), if !event_tasks.is_empty() => {
                let _completed = completed;
                continue;
            }
            completed = request_tasks.join_next(), if !request_tasks.is_empty() => {
                let _completed = completed;
                continue;
            }
            handled = response_receiver.recv() => {
                let Some(handled) = handled else { break; };
                let mut handled = handled?;
                websocket.send(Message::Text(handled.response.to_string().into())).await?;
                if matches!(handled.method.as_str(), "thread/start" | "thread/read" | "thread/resume")
                    && handled.response.get("result").is_some()
                    && let Some(thread_id) = handled.response.pointer("/result/thread/id").and_then(Value::as_str)
                {
                    let session = if let Some((session, _)) = &handled.resume_attachment {
                        Some(session.clone())
                    } else {
                        context.events.sessions(context.endpoint.clone()).await
                            .map_err(|_| AppServerConnectionError::HubUnavailable)?
                            .into_iter()
                            .find(|summary| thread_alias(&summary.session) == thread_id)
                            .map(|summary| summary.session)
                    };
                    if let Some(session) = session
                        && attached_sessions.insert(session.clone())
                    {
                        let attachment = if let Some((_, attachment)) = handled.resume_attachment.take() {
                            attachment
                        } else {
                            context.events.attach(session.clone()).await
                                .map_err(|_| AppServerConnectionError::HubUnavailable)?
                        };
                        let subscription = attach_session_events(
                            session.clone(), attachment, &mut forwarding, &mut websocket,
                            &event_sender, &mut event_tasks,
                        ).await?;
                        subscriptions.insert(session, subscription);
                    }
                }
                continue;
            }
            forwarded = event_receiver.recv(), if !attached_sessions.is_empty() => {
                let Some((session, event)) = forwarded else { break; };
                if event.as_ref().map_or(true, |event| matches!(&event.event, SessionEvent::ResyncRequired { .. })) {
                    if let Some(subscription) = subscriptions.remove(&session) {
                        subscription.abort();
                    }
                    attached_sessions.remove(&session);
                    for notification in forwarding.reset_session(&session) {
                        websocket.send(Message::Text(notification.to_string().into())).await?;
                    }
                    match context.events.attach(session.clone()).await {
                        Ok(attachment) => {
                            let subscription = attach_session_events(
                                session.clone(), attachment, &mut forwarding, &mut websocket,
                                &event_sender, &mut event_tasks,
                            ).await?;
                            attached_sessions.insert(session.clone());
                            subscriptions.insert(session, subscription);
                        }
                        Err(error) => {
                            tracing::warn!(session_id = session.session_id.as_str(), error_kind = ?error,
                                "provider TUI Session reattach unavailable");
                        }
                    }
                } else if let Ok(event) = event {
                    for notification in forwarding.project(&session, &event) {
                        websocket.send(Message::Text(notification.to_string().into())).await?;
                    }
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
        if request.get("method").is_none() {
            for notification in forwarding.resolve_reply(&request).await {
                websocket
                    .send(Message::Text(notification.to_string().into()))
                    .await?;
            }
            continue;
        }
        if request.get("id").is_none() {
            continue;
        }
        let request_context = Arc::clone(&context);
        let response_sender = response_sender.clone();
        request_tasks.spawn(async move {
            let result = handle_app_server_rpc(request_context, request).await;
            let _sent = response_sender.send(result).await;
        });
    }
    event_tasks.abort_all();
    request_tasks.abort_all();
    while event_tasks.join_next().await.is_some() {}
    while request_tasks.join_next().await.is_some() {}
    Ok(())
}

async fn handle_app_server_rpc(
    context: Arc<RouterSessionAppServerContext>,
    request: Value,
) -> Result<HandledAppServerRequest, AppServerConnectionError> {
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let method = request
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let catalog = context.model_catalog.borrow().clone();
    let mut response = if matches!(
        method.as_str(),
        "thread/start"
            | "thread/list"
            | "thread/read"
            | "thread/resume"
            | "turn/start"
            | "turn/steer"
            | "turn/interrupt"
    ) {
        let mut params = request.get("params").cloned().unwrap_or_else(|| json!({}));
        // The connection owns the resume snapshot and its receiver.
        if method == "thread/resume"
            && let Some(object) = params.as_object_mut()
        {
            object.insert("excludeTurns".into(), Value::Bool(true));
        }
        let result = if method.starts_with("thread/") {
            handle_app_server_thread_request(
                &method,
                params,
                Arc::clone(&context.commands),
                Arc::clone(&context.events),
                context.endpoint.clone(),
                context.actor.clone(),
                &catalog,
            )
            .await
        } else {
            handle_app_server_turn_request(
                &method,
                params,
                Arc::clone(&context.commands),
                Arc::clone(&context.events),
                context.endpoint.clone(),
                context.actor.clone(),
                &catalog,
            )
            .await
        };
        match result {
            Ok(result) => json!({"id":id,"result":result}),
            Err(error) => {
                let code = match error {
                    ThreadMethodError::InvalidParams
                    | ThreadMethodError::WorkingDirectoryRequired
                    | ThreadMethodError::ImageUnreadable
                    | ThreadMethodError::ImageTooLarge
                    | ThreadMethodError::PromptTooLarge
                    | ThreadMethodError::ImageUnsupportedType => -32602,
                    ThreadMethodError::NotFound => -32002,
                    ThreadMethodError::Unavailable => -32000,
                };
                json!({"id":id,"error":{"code":code,"message":error.to_string()}})
            }
        }
    } else if method == "config/read" {
        let params = request.get("params").cloned().unwrap_or(Value::Null);
        let trust_lookup = context.project_trust.clone();
        tokio::task::spawn_blocking(move || {
            handle_app_server_request(
                id,
                "config/read",
                &params,
                &catalog,
                trust_lookup.as_deref(),
            )
        })
        .await
        .map_err(|_| AppServerConnectionError::RequestWorkerUnavailable)?
    } else {
        handle_app_server_request(
            id,
            &method,
            request.get("params").unwrap_or(&Value::Null),
            &catalog,
            context.project_trust.as_deref(),
        )
    };
    let mut resume_attachment = None;
    if method == "thread/resume"
        && response.get("result").is_some()
        && let Some(thread_id) = response
            .pointer("/result/thread/id")
            .and_then(Value::as_str)
        && let Some(session) = context
            .events
            .sessions(context.endpoint.clone())
            .await
            .map_err(|_| AppServerConnectionError::HubUnavailable)?
            .into_iter()
            .find(|summary| thread_alias(&summary.session) == thread_id)
            .map(|summary| summary.session)
    {
        let attachment = context
            .events
            .attach(session.clone())
            .await
            .map_err(|_| AppServerConnectionError::HubUnavailable)?;
        if request
            .pointer("/params/excludeTurns")
            .and_then(Value::as_bool)
            != Some(true)
        {
            let thread = response
                .pointer_mut("/result/thread")
                .and_then(Value::as_object_mut)
                .ok_or(AppServerConnectionError::RequestWorkerUnavailable)?;
            thread.insert(
                "turns".into(),
                json!(historical_turns(&session, &attachment.snapshot)),
            );
        }
        resume_attachment = Some((session, attachment));
    }
    Ok(HandledAppServerRequest {
        method,
        response,
        resume_attachment,
    })
}

async fn attach_session_events(
    session: SessionRef,
    attachment: SessionEventAttachment,
    forwarding: &mut AppServerEventForwarding,
    websocket: &mut tokio_tungstenite::WebSocketStream<UnixStream>,
    event_sender: &mpsc::Sender<(SessionRef, Result<HubEvent, ()>)>,
    event_tasks: &mut JoinSet<()>,
) -> Result<AbortHandle, AppServerConnectionError> {
    let pending_requests = pending_snapshot_requests(&attachment.snapshot);
    for event in attachment.snapshot {
        let request_still_pending = matches!(
            &event.event,
            SessionEvent::InteractionRequested { interaction }
                if pending_requests.contains(interaction.request_id())
        );
        for notification in forwarding.project(&session, &event) {
            if request_still_pending {
                websocket
                    .send(Message::Text(notification.to_string().into()))
                    .await?;
            }
        }
    }
    let mut receiver = attachment.receiver;
    let sender = event_sender.clone();
    Ok(event_tasks.spawn(async move {
        loop {
            let event = receiver.recv().await.map_err(|_| ());
            let needs_reattach = event.as_ref().map_or(true, |event| {
                matches!(&event.event, SessionEvent::ResyncRequired { .. })
            });
            if sender.send((session.clone(), event)).await.is_err() || needs_reattach {
                break;
            }
        }
    }))
}

#[path = "router_session_app_server_methods.rs"]
mod methods;
pub(crate) use methods::thread_alias;
use methods::{
    handle_app_server_request, handle_app_server_thread_request, handle_app_server_turn_request,
};

#[cfg(test)]
#[allow(clippy::panic_in_result_fn)]
#[path = "router_session_app_server_cwd_tests.rs"]
mod cwd_tests;
#[cfg(test)]
#[allow(clippy::panic_in_result_fn)]
#[path = "router_session_app_server_model_tests.rs"]
mod model_tests;
#[cfg(test)]
#[allow(clippy::panic_in_result_fn)]
#[path = "router_session_app_server_multichoice_tui_tests.rs"]
mod multichoice_tui_tests;
#[cfg(test)]
#[allow(clippy::panic_in_result_fn)]
#[path = "router_session_app_server_reattach_tests.rs"]
mod reattach_tests;
#[cfg(test)]
#[path = "router_session_app_server_test_support.rs"]
mod test_support;
#[cfg(test)]
#[allow(clippy::panic_in_result_fn)]
#[path = "router_session_app_server_tests.rs"]
mod tests;
#[cfg(test)]
#[allow(clippy::panic_in_result_fn)]
#[path = "router_session_app_server_tui_tests.rs"]
mod tui_tests;

//! Codex app-server protocol transport for Router-owned provider sessions.
use crate::{
    CommandContent, CreateSessionCommand, HubEvent, HubSessionSummary, PromptSessionCommand,
    SessionCommandPort, SessionEventHub, SessionEventHubError, SessionSettingsCommand,
    SessionSteerOutcome, SessionTargetCommand, SteerSessionCommand,
    app_server_event_forwarding::{AppServerEventForwarding, historical_turns},
    app_server_model_catalog::{ProviderModelEntry, render_model_list},
    private_socket_listener::PrivateSocketListener,
};
use futures_util::{SinkExt, StreamExt};
use message_board::{Identity, SessionEndpointRef, SessionRef};
use serde_json::{Value, json};
use session_event_model::SessionEvent;
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
    interaction_broker: Option<Arc<crate::ServiceApprovalBroker>>,
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
            interaction_broker: None,
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

    #[must_use]
    pub fn with_interaction_broker(mut self, broker: Arc<crate::ServiceApprovalBroker>) -> Self {
        self.interaction_broker = Some(broker);
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
    let mut forwarding =
        AppServerEventForwarding::new(context.actor.clone(), context.interaction_broker.clone());
    loop {
        let frame = tokio::select! {
            incoming = websocket.next() => incoming,
            forwarded = event_receiver.recv(), if !event_tasks.is_empty() => {
                let Some((session, event)) = forwarded else { break; };
                let Ok(event) = event else { break; };
                for notification in forwarding.project(&session, &event) {
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
        if request.get("method").is_none() {
            forwarding.resolve_reply(&request).await;
            continue;
        }
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
                    for notification in forwarding.project(&session, &event) {
                        if matches!(&event.event, SessionEvent::InteractionRequested { .. }) {
                            websocket
                                .send(Message::Text(notification.to_string().into()))
                                .await?;
                        }
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

#[path = "router_session_app_server_methods.rs"]
mod methods;
pub(crate) use methods::thread_alias;
use methods::{
    handle_app_server_request, handle_app_server_thread_request, handle_app_server_turn_request,
};

#[cfg(test)]
#[path = "router_session_app_server_test_support.rs"]
mod test_support;
#[cfg(test)]
#[allow(clippy::panic_in_result_fn)]
#[path = "router_session_app_server_tests.rs"]
mod tests;

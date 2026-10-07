//! Codex route admission scoped to active Codex Sessions, not the ACP socket.
use crate::acp_connection_dispatch::route_codex_sessions;
use crate::{
    AcpConnectionContext, AcpConnectionInputs, AcpOutputSender, AcpRouteFuture, AcpRouterChannels,
    AcpSessionRoute, bounded_acp_output,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    sync::Arc,
};
use tokio::{sync::mpsc, task::JoinHandle};
use tokio_util::sync::CancellationToken;

/// The Host owns the generation gate and constructs one admitted Codex view.
pub trait CodexAdmissionSource: Send + Sync {
    fn acquire(&self) -> io::Result<AcpConnectionInputs>;
}

pub fn lazy_codex_session_route(
    admission: Arc<dyn CodexAdmissionSource>,
) -> Box<dyn AcpSessionRoute> {
    Box::new(LazyCodexSessionRoute { admission })
}

struct LazyCodexSessionRoute {
    admission: Arc<dyn CodexAdmissionSource>,
}

impl AcpSessionRoute for LazyCodexSessionRoute {
    fn endpoint_id(&self) -> &str {
        "codex-local"
    }

    fn run(
        self: Box<Self>,
        router: AcpRouterChannels,
        _context: AcpConnectionContext,
    ) -> AcpRouteFuture {
        Box::pin(serve_lazy_codex_sessions(router, self.admission))
    }
}

struct ActiveGeneration {
    epoch: u64,
    input: AcpOutputSender,
    closed: CancellationToken,
    retirement: CancellationToken,
    task: JoinHandle<()>,
    session_ids: BTreeSet<String>,
    pending: BTreeMap<String, Value>,
}

impl Drop for ActiveGeneration {
    fn drop(&mut self) {
        self.closed.cancel();
        self.task.abort();
    }
}

#[derive(Clone, Copy)]
enum GenerationClosure {
    NativeRetired,
    RouteUnavailable,
}
impl GenerationClosure {
    fn pending_message(self) -> &'static str {
        match self {
            Self::NativeRetired => "Codex generation retired",
            Self::RouteUnavailable => "Codex route unavailable",
        }
    }
    fn session_message(self) -> &'static str {
        match self {
            Self::NativeRetired => "Codex session generation retired",
            Self::RouteUnavailable => "Codex session route unavailable",
        }
    }
}

enum GenerationEvent {
    Frame { epoch: u64, frame: Value },
    Retired { epoch: u64 },
    Closed { epoch: u64 },
}

fn unavailable(id: Value, reason: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32000,"message":reason}})
}

async fn close_active_generation(
    active: &mut Option<ActiveGeneration>,
    closed_sessions: &mut BTreeMap<String, GenerationClosure>,
    reason: GenerationClosure,
    output: &AcpOutputSender,
) -> io::Result<()> {
    let Some(mut current) = active.take() else {
        return Ok(());
    };
    current.closed.cancel();
    current.task.abort();
    for session_id in std::mem::take(&mut current.session_ids) {
        closed_sessions.insert(session_id, reason);
    }
    for (_, id) in std::mem::take(&mut current.pending) {
        output
            .send(unavailable(id, reason.pending_message()))
            .await?;
    }
    Ok(())
}

async fn forward_active_input(
    active: &mut Option<ActiveGeneration>,
    closed_sessions: &mut BTreeMap<String, GenerationClosure>,
    output: &AcpOutputSender,
    frame: &Value,
) -> io::Result<()> {
    let id = frame.get("id").cloned();
    let Some(current) = active.as_mut() else {
        if let Some(id) = id {
            output
                .send(unavailable(id, "Codex session unavailable"))
                .await?;
        }
        return Ok(());
    };
    if let Some(id) = id {
        current.pending.insert(id.to_string(), id);
    }
    if current.input.send(frame.clone()).await.is_err() {
        let reason = if current.retirement.is_cancelled() {
            GenerationClosure::NativeRetired
        } else {
            GenerationClosure::RouteUnavailable
        };
        // The incoming ID is already pending. Settle it with this observer's
        // other requests once; admission of later requests stays unchanged.
        close_active_generation(active, closed_sessions, reason, output).await?;
    }
    Ok(())
}

async fn serve_lazy_codex_sessions(
    mut router: AcpRouterChannels,
    admission: Arc<dyn CodexAdmissionSource>,
) -> io::Result<()> {
    let (event_sender, mut event_receiver) = mpsc::channel::<GenerationEvent>(1024);
    let mut active: Option<ActiveGeneration> = None;
    let mut closed_sessions = BTreeMap::<String, GenerationClosure>::new();
    let mut next_epoch = 0_u64;
    loop {
        tokio::select! {
            _ = router.closed.cancelled() => break,
            event = event_receiver.recv() => {
                let Some(event) = event else { break; };
                match event {
                    GenerationEvent::Frame { epoch, frame } => {
                        let Some(current) = active.as_mut().filter(|current| current.epoch == epoch) else { continue; };
                        if let Some(id) = frame.get("id") {
                            let key = id.to_string();
                            if current.pending.remove(&key).is_some()
                                && let Some(session_id) = frame.pointer("/result/sessionId").and_then(Value::as_str) {
                                closed_sessions.remove(session_id);
                                current.session_ids.insert(session_id.to_owned());
                            }
                        }
                        router.output.send(frame).await?;
                    }
                    GenerationEvent::Retired { epoch } => {
                        if active.as_ref().is_none_or(|current| current.epoch != epoch) { continue; }
                        close_active_generation(&mut active, &mut closed_sessions, GenerationClosure::NativeRetired, &router.output).await?;
                    }
                    GenerationEvent::Closed { epoch } => {
                        let Some(current) = active.as_ref().filter(|current| current.epoch == epoch) else { continue; };
                        let reason = if current.retirement.is_cancelled() { GenerationClosure::NativeRetired } else { GenerationClosure::RouteUnavailable };
                        close_active_generation(&mut active, &mut closed_sessions, reason, &router.output).await?;
                    }
                }
            }
            frame = router.input.recv() => {
                let Some(frame) = frame else { break; };
                if active.as_ref().is_some_and(|current| current.retirement.is_cancelled()) {
                    close_active_generation(&mut active, &mut closed_sessions, GenerationClosure::NativeRetired, &router.output).await?;
                }
                let method = frame.get("method").and_then(Value::as_str).unwrap_or("");
                let id = frame.get("id").cloned();
                let session_id = frame.pointer("/params/sessionId").and_then(Value::as_str);
                let admission_request = matches!(method, "session/new" | "session/load" | "session/resume" | "session/list");
                if !admission_request && let Some(reason) = session_id.and_then(|id| closed_sessions.get(id)) {
                    if let Some(id) = id { router.output.send(unavailable(id, reason.session_message())).await?; }
                    continue;
                }
                if active.is_none() && admission_request {
                    match admission.acquire() {
                        Err(error) => {
                            if let Some(id) = id { router.output.send(unavailable(id, &format!("Codex backend unavailable: {error}"))).await?; }
                            continue;
                        }
                        Ok(inputs) => {
                            next_epoch = next_epoch.saturating_add(1);
                            let epoch = next_epoch;
                            let retired = inputs.retired.clone();
                            let closed = router.closed.child_token();
                            let (input, input_receiver) = bounded_acp_output(closed.clone());
                            let (output, mut output_receiver) = bounded_acp_output(closed.clone());
                            let inner = AcpRouterChannels { input: input_receiver, output, closed: closed.clone() };
                            let sender = event_sender.clone();
                            let route_closed = closed.clone();
                            let task = tokio::spawn(async move {
                                let run = route_codex_sessions(inner, inputs);
                                let forward = async {
                                    while let Some(frame) = output_receiver.recv().await {
                                        if sender.send(GenerationEvent::Frame { epoch, frame: (*frame).clone() }).await.is_err() { break; }
                                    }
                                };
                                tokio::select! {
                                    _run_result = run => {},
                                    () = forward => {},
                                }
                                // Freeze this observer's producer before draining its bounded
                                // queue; Host actors keep running with detached output.
                                route_closed.cancel();
                                while let Ok(frame) = output_receiver.try_recv() {
                                    if sender.send(GenerationEvent::Frame { epoch, frame: (*frame).clone() }).await.is_err() { break; }
                                }
                                let _sent = sender.send(GenerationEvent::Closed { epoch }).await;
                            });
                            let sender = event_sender.clone();
                            let watcher_closed = closed.clone();
                            let retirement = retired.clone();
                            tokio::spawn(async move {
                                tokio::select! {
                                    () = retired.cancelled() => {
                                        let _sent = sender.send(GenerationEvent::Retired { epoch }).await;
                                    }
                                    () = watcher_closed.cancelled() => {}
                                }
                            });
                            active = Some(ActiveGeneration { epoch, input, closed, retirement, task, session_ids: BTreeSet::new(), pending: BTreeMap::new() });
                        }
                    }
                }
                forward_active_input(&mut active, &mut closed_sessions, &router.output, &frame).await?;
            }
        }
    }
    drop(active);
    Ok(())
}

#[cfg(test)]
#[path = "lazy_codex_session_route_tests.rs"]
mod tests;

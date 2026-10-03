//! Owner-private ACP ingress for Codex and provider Sessions.
use crate::{
    NativeGenerationGate, ProviderAcpSessionRoute, SessionCommandPort, SessionEventHub,
    private_socket_listener::PrivateSocketListener,
};
use codex_acp_adapter::{
    AcpConnectionInputs, AcpSchemaCatalog, AcpSessionRoute, AcpStoredSessions,
    CodexAdmissionSource, lazy_codex_session_route, serve_acp_router_connection,
};
use message_board::SessionEndpointRef;
use std::{io, path::Path, sync::Arc};
use tokio::{sync::Semaphore, task::JoinSet};
use tokio_util::sync::CancellationToken;

pub struct AcpChannelListener {
    socket: PrivateSocketListener,
    generations: NativeGenerationGate,
    stored_sessions: Arc<dyn AcpStoredSessions>,
    approval_broker: Arc<dyn codex_acp_adapter::ApprovalBroker>,
    holder: Arc<crate::UnmaterializedThreadHolder>,
    recorder: Arc<dyn codex_acp_adapter::ConversationOperationRecorder>,
    providers: Vec<ProviderRouteBackend>,
    interaction_broker: Option<Arc<crate::ServiceInteractionBroker>>,
    permits: Arc<Semaphore>,
    #[cfg(test)]
    fail_accept: bool,
}
impl AcpChannelListener {
    pub fn bind(
        path: &Path,
        generations: NativeGenerationGate,
        stored_sessions: Arc<dyn AcpStoredSessions>,
        approval_broker: Arc<dyn codex_acp_adapter::ApprovalBroker>,
        holder: Arc<crate::UnmaterializedThreadHolder>,
        recorder: Arc<dyn codex_acp_adapter::ConversationOperationRecorder>,
    ) -> io::Result<Self> {
        let _schema = AcpSchemaCatalog::load().map_err(io::Error::other)?;
        Ok(Self {
            socket: PrivateSocketListener::bind(path)?,
            generations,
            stored_sessions,
            approval_broker,
            holder,
            recorder,
            providers: Vec::new(),
            interaction_broker: None,
            permits: Arc::new(Semaphore::new(32)),
            #[cfg(test)]
            fail_accept: false,
        })
    }
    #[must_use]
    pub fn with_connection_budget(mut self, permits: Arc<Semaphore>) -> Self {
        self.permits = permits;
        self
    }
    #[must_use]
    pub fn with_provider_session_backend(
        mut self,
        endpoint: SessionEndpointRef,
        commands: Arc<dyn SessionCommandPort>,
        events: Arc<dyn SessionEventHub>,
    ) -> Self {
        self.providers.push(ProviderRouteBackend {
            endpoint,
            commands,
            events,
        });
        self
    }
    #[must_use]
    pub fn with_interaction_broker(mut self, broker: Arc<crate::ServiceInteractionBroker>) -> Self {
        self.interaction_broker = Some(broker);
        self
    }
    pub async fn run(self, shutdown: CancellationToken) -> io::Result<()> {
        let mut tasks = JoinSet::new();
        let result = loop {
            tokio::select! {
                _ = shutdown.cancelled() => break Ok(()),
                completed = tasks.join_next(), if !tasks.is_empty() => { let _ = completed; },
                accepted = async {
                    #[cfg(test)]
                    if self.fail_accept {
                        return Err(io::Error::other("injected ACP accept failure"));
                    }
                    self.socket.listener.accept().await
                } => {
                    let (stream, _) = match accepted { Ok(pair) => pair, Err(error) => break Err(error) };
                    let Ok(permit) = Arc::clone(&self.permits).try_acquire_owned() else { continue; };
                    let admission = GateCodexAdmissionSource {
                        generations: self.generations.clone(),
                        stored_sessions: Arc::clone(&self.stored_sessions),
                        approval_broker: Arc::clone(&self.approval_broker),
                        holder: Arc::clone(&self.holder),
                        recorder: Arc::clone(&self.recorder),
                    };
                    let mut routes: Vec<Box<dyn AcpSessionRoute>> = vec![
                        lazy_codex_session_route(Arc::new(admission)),
                    ];
                    routes.extend(self.providers.iter().map(|provider| {
                        let route = ProviderAcpSessionRoute::new(
                            provider.endpoint.clone(),
                            Arc::clone(&provider.commands),
                            Arc::clone(&provider.events),
                        );
                        let route = if let Some(broker) = &self.interaction_broker {
                            route.with_interaction_broker(Arc::clone(broker))
                        } else { route };
                        Box::new(route) as Box<dyn AcpSessionRoute>
                    }));
                    tasks.spawn(async move { let _permit = permit; serve_acp_router_connection(stream,routes).await });
                }
            }
        };
        tasks.abort_all();
        while tasks.join_next().await.is_some() {}
        if result.is_err() {
            self.generations.retire()?;
        }
        self.holder.drain_host_tasks().await;
        result
    }
}

struct ProviderRouteBackend {
    endpoint: SessionEndpointRef,
    commands: Arc<dyn SessionCommandPort>,
    events: Arc<dyn SessionEventHub>,
}

struct GateCodexAdmissionSource {
    generations: NativeGenerationGate,
    stored_sessions: Arc<dyn AcpStoredSessions>,
    approval_broker: Arc<dyn codex_acp_adapter::ApprovalBroker>,
    holder: Arc<crate::UnmaterializedThreadHolder>,
    recorder: Arc<dyn codex_acp_adapter::ConversationOperationRecorder>,
}

impl CodexAdmissionSource for GateCodexAdmissionSource {
    fn acquire(&self) -> io::Result<AcpConnectionInputs> {
        let admission = self.generations.acquire()?;
        let schemas = admission
            .schemas()
            .filter(|schemas| schemas.supports_server_messages())
            .ok_or_else(|| io::Error::other("Codex payload schemas unavailable"))?;
        Ok(AcpConnectionInputs {
            backend_path: admission.backend_path().to_owned(),
            generation: admission.generation().clone(),
            schemas,
            stored_sessions: Arc::clone(&self.stored_sessions),
            approval_broker: Arc::clone(&self.approval_broker),
            holder: Arc::clone(&self.holder)
                as Arc<dyn codex_acp_adapter::UnmaterializedBindingStore>,
            recorder: Arc::clone(&self.recorder),
            retired: admission.retirement(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_acp_adapter::{NativeStoredSessions, UnmaterializedBindingStore};
    use serde_json::json;
    use std::{os::unix::fs::DirBuilderExt, time::Duration};

    #[tokio::test]
    async fn accept_failure_retires_generation_before_draining_host_tasks() {
        let root = std::env::temp_dir().join(format!("acp-accept-failure-{}", std::process::id()));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&root)
            .unwrap();
        let gate = NativeGenerationGate::default();
        gate.activate(
            serde_json::from_value(
                json!({"serviceEpoch":"00000000-0000-4000-8000-000000000001","generation":1}),
            )
            .unwrap(),
            root.join("backend.sock"),
            None,
        )
        .unwrap();
        let retirement = gate.acquire().unwrap().retirement();
        let holder = Arc::new(crate::UnmaterializedThreadHolder::new());
        holder
            .host_tasks()
            .spawn(async move { retirement.cancelled().await });
        let mut listener = AcpChannelListener::bind(
            &root.join("acp.sock"),
            gate.clone(),
            Arc::new(NativeStoredSessions::new(root.clone(), "fixture".into())),
            Arc::new(codex_acp_adapter::RejectingApprovalBroker),
            holder,
            Arc::new(crate::UnavailableConversationOperationRecorder),
        )
        .unwrap();
        listener.fail_accept = true;
        let result = tokio::time::timeout(
            Duration::from_secs(2),
            listener.run(CancellationToken::new()),
        )
        .await
        .expect("listener failure should reach Host before tracked drain blocks");
        assert!(result.is_err());
        assert!(
            gate.acquire().is_err(),
            "failed listener must retire native admission"
        );
        std::fs::remove_dir(root).unwrap();
    }
}

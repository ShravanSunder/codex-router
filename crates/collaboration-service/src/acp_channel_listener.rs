//! Owner-private ACP ingress bound to one admitted native backend generation.
use crate::{NativeGenerationGate, private_socket_listener::PrivateSocketListener};
use codex_acp_adapter::{
    AcpConnectionInputs, AcpSchemaCatalog, AcpStoredSessions, serve_acp_connection,
};
use std::{io, path::Path, sync::Arc};
use tokio::{sync::Semaphore, task::JoinSet};
use tokio_util::sync::CancellationToken;

pub struct AcpChannelListener {
    socket: PrivateSocketListener,
    generations: NativeGenerationGate,
    stored_sessions: Arc<dyn AcpStoredSessions>,
    permits: Arc<Semaphore>,
}
impl AcpChannelListener {
    pub fn bind(
        path: &Path,
        generations: NativeGenerationGate,
        stored_sessions: Arc<dyn AcpStoredSessions>,
    ) -> io::Result<Self> {
        let _schema = AcpSchemaCatalog::load().map_err(io::Error::other)?;
        Ok(Self {
            socket: PrivateSocketListener::bind(path)?,
            generations,
            stored_sessions,
            permits: Arc::new(Semaphore::new(32)),
        })
    }
    #[must_use]
    pub fn with_connection_budget(mut self, permits: Arc<Semaphore>) -> Self {
        self.permits = permits;
        self
    }
    pub async fn run(self, shutdown: CancellationToken) -> io::Result<()> {
        let mut tasks = JoinSet::new();
        let result = loop {
            tokio::select! {
                _ = shutdown.cancelled() => break Ok(()),
                completed = tasks.join_next(), if !tasks.is_empty() => { let _ = completed; },
                accepted = self.socket.listener.accept() => {
                    let (stream, _) = match accepted { Ok(pair) => pair, Err(error) => break Err(error) };
                    let Ok(permit) = Arc::clone(&self.permits).try_acquire_owned() else { continue; };
                    let Ok(admission) = self.generations.acquire() else { continue; };
                    let Some(schemas) = admission.schemas().filter(|s| s.supports_server_messages()) else { continue; };
                    let inputs = AcpConnectionInputs { backend_path:admission.backend_path().to_owned(), generation:admission.generation().clone(), schemas, stored_sessions:Arc::clone(&self.stored_sessions), retired:admission.retirement() };
                    tasks.spawn(async move { let _permit = permit; serve_acp_connection(stream,inputs).await });
                }
            }
        };
        tasks.abort_all();
        while tasks.join_next().await.is_some() {}
        result
    }
}

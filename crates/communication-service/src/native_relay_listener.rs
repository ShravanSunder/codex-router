//! Stable native frontend with generation-scoped admission and connection cleanup.
use crate::private_socket_listener::PrivateSocketListener;
use crate::{NativeGenerationGate, connect_native_relay};
use std::{io, path::Path, sync::Arc};
use tokio::{sync::Semaphore, task::JoinSet};
use tokio_util::sync::CancellationToken;

pub struct NativeRelayListener {
    socket: PrivateSocketListener,
    generations: NativeGenerationGate,
    permits: Arc<Semaphore>,
}
impl NativeRelayListener {
    pub fn bind(path: &Path, generations: NativeGenerationGate) -> io::Result<Self> {
        Ok(Self {
            socket: PrivateSocketListener::bind(path)?,
            generations,
            permits: Arc::new(Semaphore::new(32)),
        })
    }
    #[must_use]
    pub fn with_connection_budget(mut self, permits: Arc<Semaphore>) -> Self {
        self.permits = permits;
        self
    }
    pub async fn run(self, shutdown: CancellationToken) -> io::Result<()> {
        let permits = Arc::clone(&self.permits);
        let mut tasks = JoinSet::new();
        let result = loop {
            tokio::select! {
                _ = shutdown.cancelled()=>break Ok(()),
                completed = tasks.join_next(), if !tasks.is_empty()=>{let _ = completed;}
                accepted = self.socket.listener.accept()=>{
                    let (stream,_)=match accepted {Ok(value)=>value,Err(error)=>break Err(error)};
                    let Ok(permit)=Arc::clone(&permits).try_acquire_owned() else {continue;};
                    let Ok(admission)=self.generations.acquire() else {continue;};
                    tasks.spawn(async move {
                        let _permit=permit;
                        connect_native_relay(stream,admission.backend_path(),admission.retirement()).await
                    });
                }
            }
        };
        tasks.abort_all();
        while tasks.join_next().await.is_some() {}
        result
    }
}

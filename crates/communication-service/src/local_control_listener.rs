//! A private, retained Control listener; never binds or cleans the native backend socket.
use crate::{ServiceIdentity, serve_control_connection};
use std::io;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

pub struct LocalControlService {
    socket: crate::private_socket_listener::PrivateSocketListener,
    identity: ServiceIdentity,
    permits: Arc<Semaphore>,
}
impl LocalControlService {
    pub fn bind(socket_path: &Path, identity: ServiceIdentity) -> io::Result<Self> {
        Ok(Self {
            socket: crate::private_socket_listener::PrivateSocketListener::bind(socket_path)?,
            identity,
            permits: Arc::new(Semaphore::new(32)),
        })
    }
    /// Shutdown closes only this listener and its accepted Control connections.
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
                _ = shutdown.cancelled() => break Ok(()),
                completed = tasks.join_next(), if !tasks.is_empty() => { let _ = completed; }
                accepted = self.socket.listener.accept() => {
                    let (stream,_) = match accepted { Ok(value)=>value,Err(error)=>break Err(error) };
                    let Ok(permit)=Arc::clone(&permits).try_acquire_owned() else { drop(stream);continue; };
                    let identity=self.identity.clone();
                    tasks.spawn(async move {
                        let _permit=permit;
                        serve_control_connection(stream,identity).await
                    });
                }
            }
        };
        tasks.abort_all();
        while tasks.join_next().await.is_some() {}
        result
    }
}

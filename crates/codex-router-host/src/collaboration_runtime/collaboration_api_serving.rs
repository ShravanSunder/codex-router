//! The collaboration API on the Host's localhost listener.
//!
//! The listener is bound first, so its URL can be handed to provider launches and the manifest
//! before the Router's dependencies are composed; it starts serving once the application over
//! those dependencies exists.
use collaboration_mcp::{
    COLLABORATION_API_PATH, CollaborationApiConfig, CollaborationApiListener, LoopbackBindAddress,
};
use std::{io, net::SocketAddr};
use tokio::{net::TcpListener, task::JoinHandle};

/// The bound localhost listener, not yet serving.
pub(super) struct BoundCollaborationApi {
    listener: TcpListener,
    local_address: SocketAddr,
}

impl BoundCollaborationApi {
    pub(super) async fn bind(address: SocketAddr) -> io::Result<Self> {
        let address = LoopbackBindAddress::new(address).map_err(io::Error::other)?;
        let listener = TcpListener::bind(address.socket_addr()).await?;
        let local_address = listener.local_addr()?;
        Ok(Self {
            listener,
            local_address,
        })
    }

    pub(super) fn url(&self) -> String {
        format!("http://{}{COLLABORATION_API_PATH}", self.local_address)
    }

    /// Serves the API until the configuration's shutdown token is cancelled.
    pub(super) fn serve(self, config: &CollaborationApiConfig) -> ServedCollaborationApi {
        let api = collaboration_mcp::collaboration_api_router(
            config,
            CollaborationApiListener::LoopbackTcp(self.local_address),
        );
        ServedCollaborationApi {
            task: Some(tokio::spawn(collaboration_mcp::serve_collaboration_api(
                self.listener,
                api,
                config.shutdown.clone(),
            ))),
        }
    }
}

/// The serving task; it ends only when shutdown is cancelled or the listener fails.
pub(super) struct ServedCollaborationApi {
    task: Option<JoinHandle<io::Result<()>>>,
}

impl ServedCollaborationApi {
    /// Resolves only when the listener stops on its own.
    pub(super) async fn failure(&mut self) -> io::Error {
        let Some(task) = &mut self.task else {
            return std::future::pending().await;
        };
        let result = task.await;
        self.task = None;
        match result {
            Ok(Err(error)) => error,
            Ok(Ok(())) => io::Error::other("collaboration API listener stopped unexpectedly"),
            Err(error) => io::Error::other(error.to_string()),
        }
    }

    /// Waits for the listener to stop after shutdown and for its cancelled calls to settle.
    pub(super) async fn stopped(&mut self) -> io::Result<()> {
        let Some(task) = self.task.take() else {
            return Ok(());
        };
        task.await
            .map_err(|error| io::Error::other(error.to_string()))?
    }
}

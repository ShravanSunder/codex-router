//! The collaboration API on the Host's two listeners: localhost TCP for models and the
//! owner-only `control.sock` for the CLIs.
//!
//! The TCP listener is bound first, so its URL can be handed to provider launches and the
//! manifest before the Router's dependencies are composed; the socket is bound before the
//! manifest is published. Both start serving once the application over those dependencies
//! exists.
use collaboration_mcp::{
    COLLABORATION_API_PATH, CollaborationApiConfig, CollaborationApiListener, LoopbackBindAddress,
};
use collaboration_service::{OwnerOnlySocket, SocketCleanup};
use std::{io, net::SocketAddr, path::Path};
use tokio::{
    net::{TcpListener, UnixListener},
    task::JoinHandle,
};

/// The bound listeners, not yet serving.
pub(super) struct BoundCollaborationApi {
    listener: TcpListener,
    local_address: SocketAddr,
    service_socket: Option<(UnixListener, SocketCleanup)>,
}

impl BoundCollaborationApi {
    pub(super) async fn bind(address: SocketAddr) -> io::Result<Self> {
        let address = LoopbackBindAddress::new(address).map_err(io::Error::other)?;
        let listener = TcpListener::bind(address.socket_addr()).await?;
        let local_address = listener.local_addr()?;
        Ok(Self {
            listener,
            local_address,
            service_socket: None,
        })
    }

    /// Binds the owner-only service socket the CLIs reach the API on.
    pub(super) fn with_service_socket(mut self, path: &Path) -> io::Result<Self> {
        self.service_socket = Some(OwnerOnlySocket::bind(path)?.into_parts());
        Ok(self)
    }

    pub(super) fn url(&self) -> String {
        format!("http://{}{COLLABORATION_API_PATH}", self.local_address)
    }

    /// Serves the API on every bound listener until the configuration's shutdown token is
    /// cancelled.
    pub(super) fn serve(self, config: &CollaborationApiConfig) -> ServedCollaborationApi {
        let loopback = collaboration_mcp::collaboration_api_router(
            config,
            CollaborationApiListener::LoopbackTcp(self.local_address),
        );
        let mut tasks = vec![tokio::spawn(collaboration_mcp::serve_collaboration_api(
            self.listener,
            loopback,
            config.shutdown.clone(),
        ))];
        let mut cleanup = None;
        if let Some((listener, socket_cleanup)) = self.service_socket {
            let socket = collaboration_mcp::collaboration_api_router(
                config,
                CollaborationApiListener::UnixSocket,
            );
            tasks.push(tokio::spawn(collaboration_mcp::serve_collaboration_api(
                listener,
                socket,
                config.shutdown.clone(),
            )));
            cleanup = Some(socket_cleanup);
        }
        ServedCollaborationApi {
            tasks,
            _socket_cleanup: cleanup,
        }
    }
}

/// The serving tasks; they end only when shutdown is cancelled or a listener fails.
pub(super) struct ServedCollaborationApi {
    tasks: Vec<JoinHandle<io::Result<()>>>,
    _socket_cleanup: Option<SocketCleanup>,
}

impl ServedCollaborationApi {
    /// Resolves only when a listener stops on its own.
    pub(super) async fn failure(&mut self) -> io::Error {
        if self.tasks.is_empty() {
            return std::future::pending().await;
        }
        let (result, index, _) = futures_util::future::select_all(self.tasks.iter_mut()).await;
        drop(self.tasks.remove(index));
        match result {
            Ok(Err(error)) => error,
            Ok(Ok(())) => io::Error::other("collaboration API listener stopped unexpectedly"),
            Err(error) => io::Error::other(error.to_string()),
        }
    }

    /// Waits for the listeners to stop after shutdown and for their cancelled calls to settle.
    pub(super) async fn stopped(&mut self) -> io::Result<()> {
        let mut first_failure = None;
        for task in self.tasks.drain(..) {
            let result = task
                .await
                .map_err(|error| io::Error::other(error.to_string()))
                .and_then(|result| result);
            if let Err(error) = result {
                first_failure.get_or_insert(error);
            }
        }
        first_failure.map_or(Ok(()), Err)
    }
}

#[cfg(test)]
mod tests {
    use super::ServedCollaborationApi;
    use std::{io, time::Duration};
    use tokio_util::sync::CancellationToken;

    #[tokio::test]
    async fn reported_listener_failure_stops_without_repolling_the_failed_task() {
        // Arrange: one listener fails at once; the other serves until shutdown.
        let shutdown = CancellationToken::new();
        let serving = shutdown.clone();
        let mut served = ServedCollaborationApi {
            tasks: vec![
                tokio::spawn(async { Err(io::Error::other("listener failed")) }),
                tokio::spawn(async move {
                    serving.cancelled().await;
                    Ok(())
                }),
            ],
            _socket_cleanup: None,
        };

        // Act
        let failure = tokio::time::timeout(Duration::from_secs(1), served.failure())
            .await
            .expect("the failed listener is reported");
        shutdown.cancel();
        let stopped = tokio::time::timeout(Duration::from_secs(1), served.stopped())
            .await
            .expect("stopping does not wait on the failed task again");

        // Assert
        assert_eq!(failure.to_string(), "listener failed");
        assert!(stopped.is_ok(), "{stopped:?}");
        assert!(served.tasks.is_empty());
    }
}

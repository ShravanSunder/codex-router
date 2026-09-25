use crate::mcp_server::CollaborationMcpServer;
use bytes::Bytes;
use http::Request;
use http_body_util::{BodyExt, Empty};
use hyper::{body::Incoming, service::service_fn};
use hyper_util::rt::{TokioExecutor, TokioIo};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService,
    session::{SessionManager, local::LocalSessionManager},
};
use std::{
    io,
    net::SocketAddr,
    path::PathBuf,
    str::FromStr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{net::TcpListener, task::JoinSet};
use tokio_util::sync::CancellationToken;

const MCP_ENDPOINT_PATH: &str = "/mcp";
// rmcp drains active handlers for up to five seconds after transport close;
// retain a distinct outer bound so its supported drain can finish first.
const SESSION_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoopbackBindAddress(SocketAddr);

#[derive(Debug, thiserror::Error, Eq, PartialEq)]
pub enum LoopbackBindAddressError {
    #[error("MCP listener address must be loopback")]
    NonLoopback,
    #[error("invalid MCP listener address")]
    InvalidAddress,
}

impl LoopbackBindAddress {
    pub fn new(address: SocketAddr) -> Result<Self, LoopbackBindAddressError> {
        if !address.ip().is_loopback() {
            return Err(LoopbackBindAddressError::NonLoopback);
        }
        Ok(Self(address))
    }
    pub fn parse(value: &str) -> Result<Self, LoopbackBindAddressError> {
        let address =
            SocketAddr::from_str(value).map_err(|_| LoopbackBindAddressError::InvalidAddress)?;
        Self::new(address)
    }

    #[must_use]
    pub const fn socket_addr(&self) -> SocketAddr {
        self.0
    }
}

#[derive(Clone, Debug)]
pub struct CollaborationMcpListenerConfig {
    pub bind_address: LoopbackBindAddress,
    pub service_directory: PathBuf,
    pub allowed_origins: Vec<String>,
}

pub struct CollaborationMcpListener {
    local_address: SocketAddr,
    shutdown: CancellationToken,
    session_manager: Arc<LocalSessionManager>,
    active_services: Arc<AtomicUsize>,
    accept_task: Option<tokio::task::JoinHandle<io::Result<()>>>,
}

impl CollaborationMcpListener {
    pub async fn start(config: CollaborationMcpListenerConfig) -> io::Result<Self> {
        let listener = TcpListener::bind(config.bind_address.socket_addr()).await?;
        let local_address = listener.local_addr()?;
        let shutdown = CancellationToken::new();
        let mut allowed_origins = config.allowed_origins;
        allowed_origins.extend([
            format!("http://{local_address}"),
            format!("http://localhost:{}", local_address.port()),
        ]);
        let service_config = StreamableHttpServerConfig::default()
            .with_allowed_hosts([local_address.ip().to_string(), "localhost".to_owned()])
            .with_allowed_origins(allowed_origins)
            .enforce_origin_validation()
            .with_cancellation_token(shutdown.clone());
        let session_manager = Arc::new(LocalSessionManager::default());
        let active_services = Arc::new(AtomicUsize::new(0));
        let service_lifecycle = Arc::clone(&active_services);
        let service: StreamableHttpService<CollaborationMcpServer, LocalSessionManager> =
            StreamableHttpService::new(
                move || {
                    Ok(CollaborationMcpServer::with_lifecycle(
                        config.service_directory.clone(),
                        Arc::clone(&service_lifecycle),
                    ))
                },
                Arc::clone(&session_manager),
                service_config,
            );
        let accept_shutdown = shutdown.clone();
        let accept_task = tokio::spawn(async move {
            let mut connection_tasks = JoinSet::new();
            loop {
                tokio::select! {
                    biased;
                    _ = accept_shutdown.cancelled() => break,
                    accepted = listener.accept() => match accepted {
                        Ok((stream, _)) => {
                            let request_service = service.clone();
                            let connection_shutdown = accept_shutdown.clone();
                            connection_tasks.spawn(async move {
                                let service = service_fn(move |request: Request<Incoming>| {
                                    let mut request_service = request_service.clone();
                                    async move {
                                        if request.uri().path() != MCP_ENDPOINT_PATH {
                                            let mut response = http::Response::new(
                                                Empty::<Bytes>::new().boxed(),
                                            );
                                            *response.status_mut() = http::StatusCode::NOT_FOUND;
                                            return Ok(response);
                                        }
                                        tower_service::Service::call(&mut request_service, request)
                                            .await
                                    }
                                });
                                let connection_builder =
                                    hyper_util::server::conn::auto::Builder::new(
                                        TokioExecutor::new(),
                                    );
                                let connection = connection_builder
                                    .serve_connection_with_upgrades(TokioIo::new(stream), service);
                                tokio::select! {
                                    _ = connection_shutdown.cancelled() => {},
                                    _ = connection => {},
                                }
                            });
                        }
                        Err(error) => return Err(error),
                    },
                    Some(_) = connection_tasks.join_next(), if !connection_tasks.is_empty() => {},
                }
            }
            connection_tasks.abort_all();
            while connection_tasks.join_next().await.is_some() {}
            Ok(())
        });
        Ok(Self {
            local_address,
            shutdown,
            session_manager,
            active_services,
            accept_task: Some(accept_task),
        })
    }

    #[must_use]
    pub const fn local_address(&self) -> SocketAddr {
        self.local_address
    }

    #[must_use]
    pub fn local_url(&self) -> String {
        format!("http://{}{}", self.local_address, MCP_ENDPOINT_PATH)
    }

    pub async fn shutdown(mut self) -> io::Result<()> {
        self.shutdown.cancel();
        let listener_result = match self.accept_task.take() {
            Some(task) => match task.await {
                Ok(result) => result,
                Err(error) => Err(io::Error::other(error.to_string())),
            },
            None => Ok(()),
        };
        let retirement_result = retire_sessions(
            Arc::clone(&self.session_manager),
            Arc::clone(&self.active_services),
        )
        .await;
        listener_result.and(retirement_result)
    }

    pub async fn listener_failure(&mut self) -> io::Error {
        let Some(task) = &mut self.accept_task else {
            return io::Error::other("MCP listener task missing");
        };
        let result = task.await;
        self.accept_task.take();
        self.shutdown.cancel();
        let listener_error = match result {
            Ok(Err(error)) => error,
            Ok(Ok(())) => io::Error::other("collaboration MCP listener stopped unexpectedly"),
            Err(error) => io::Error::other(error.to_string()),
        };
        match retire_sessions(
            Arc::clone(&self.session_manager),
            Arc::clone(&self.active_services),
        )
        .await
        {
            Ok(()) => listener_error,
            Err(retirement_error) => io::Error::other(format!(
                "{listener_error}; MCP session retirement also failed: {retirement_error}"
            )),
        }
    }
}

impl Drop for CollaborationMcpListener {
    fn drop(&mut self) {
        self.shutdown.cancel();
        if let Some(task) = self.accept_task.take() {
            task.abort();
        }
        let session_manager = Arc::clone(&self.session_manager);
        let active_services = Arc::clone(&self.active_services);
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let _retired = retire_sessions(session_manager, active_services).await;
            });
        }
    }
}

async fn retire_sessions(
    session_manager: Arc<LocalSessionManager>,
    active_services: Arc<AtomicUsize>,
) -> io::Result<()> {
    let session_ids = session_manager
        .sessions
        .read()
        .await
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    let mut first_error = None;
    for session_id in session_ids {
        let result = tokio::time::timeout(
            SESSION_SHUTDOWN_TIMEOUT,
            session_manager.close_session(&session_id),
        )
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "MCP session shutdown timed out"))
        .and_then(|result| result.map_err(|error| io::Error::other(error.to_string())));
        if first_error.is_none() {
            first_error = result.err();
        }
    }
    if first_error.is_none() {
        let settled = tokio::time::timeout(SESSION_SHUTDOWN_TIMEOUT, async {
            while active_services.load(Ordering::SeqCst) != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await;
        if settled.is_err() {
            first_error = Some(io::Error::new(
                io::ErrorKind::TimedOut,
                "MCP services did not settle after session close",
            ));
        }
    }
    first_error.map_or(Ok(()), Err)
}
#[cfg(test)]
#[path = "mcp_http_listener_tests.rs"]
mod mcp_http_listener_tests;

#[cfg(test)]
#[path = "mcp_remediation_tests.rs"]
mod mcp_remediation_tests;

#[cfg(test)]
#[path = "codex_conversation_cancel_http_tests.rs"]
mod codex_conversation_cancel_http_tests;
#[cfg(test)]
#[path = "provider_conversation_http_tests.rs"]
mod provider_conversation_http_tests;

#[cfg(test)]
#[path = "conversation_cancellation_http_tests.rs"]
mod conversation_cancellation_http_tests;

#[cfg(test)]
#[path = "provider_conversation_live_http_tests.rs"]
mod provider_conversation_live_http_tests;

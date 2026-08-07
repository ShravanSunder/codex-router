//! Permanent child-process fixtures for shared-host lifecycle proof.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use codex_router_core::router_compatibility::RouterCompatibility;
use futures_util::SinkExt;
use futures_util::StreamExt;
use serde_json::Value;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::net::UnixListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;

/// Signal behavior selected for a fixture child.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SignalFixtureMode {
    /// Record one SIGTERM and exit successfully.
    ExitOnTerminate,
    /// Record SIGTERM and remain alive until externally killed.
    IgnoreTerminate,
}

/// Runs a child fixture until its selected signal outcome.
pub async fn run_signal_fixture(mode: SignalFixtureMode, event_file: &Path) -> std::io::Result<()> {
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    std::fs::write(event_file, "ready\n")?;
    let _signal = terminate.recv().await;
    append_event(event_file, "sigterm\n")?;
    match mode {
        SignalFixtureMode::ExitOnTerminate => Ok(()),
        SignalFixtureMode::IgnoreTerminate => std::future::pending::<std::io::Result<()>>().await,
    }
}

fn append_event(event_file: &Path, event: &str) -> std::io::Result<()> {
    use std::io::Write;

    let mut file = std::fs::OpenOptions::new().append(true).open(event_file)?;
    file.write_all(event.as_bytes())
}

/// One bounded loopback router-health response.
#[derive(Clone, Copy)]
pub enum RouterHealthFixtureResponse {
    /// Exact compatible static schema.
    Compatible,
    /// Compatible identity with local model authentication required.
    AuthenticationRequired,
    /// Foreign product identity.
    Incompatible,
    /// Non-HTTP bytes from a socket squatter.
    Malformed,
}

/// Multi-request compatible router fixture stopped explicitly by its owner.
pub struct PersistentRouterHealthFixture {
    address: std::net::SocketAddr,
    request_count: Arc<AtomicUsize>,
    stop: oneshot::Sender<()>,
    task: JoinHandle<std::io::Result<()>>,
}

impl PersistentRouterHealthFixture {
    /// Starts a compatible loopback router until `finish` is called.
    pub async fn start() -> std::io::Result<Self> {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
        let address = listener.local_addr()?;
        let request_count = Arc::new(AtomicUsize::new(0));
        let task_request_count = Arc::clone(&request_count);
        let (stop, mut stop_receiver) = oneshot::channel();
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut stop_receiver => return Ok(()),
                    accepted = listener.accept() => {
                        let (mut stream, _peer) = accepted?;
                        task_request_count.fetch_add(1, Ordering::Relaxed);
                        let mut request = [0_u8; 1024];
                        let _read_bytes = stream.read(&mut request).await?;
                        let response = render_router_health_response(
                            RouterHealthFixtureResponse::Compatible,
                        )?;
                        stream.write_all(&response).await?;
                        stream.shutdown().await?;
                    }
                }
            }
        });
        Ok(Self {
            address,
            request_count,
            stop,
            task,
        })
    }

    /// Returns the kernel-assigned loopback address.
    #[must_use]
    pub const fn address(&self) -> std::net::SocketAddr {
        self.address
    }

    /// Returns the number of bounded health requests served.
    #[must_use]
    pub fn request_count(&self) -> usize {
        self.request_count.load(Ordering::Relaxed)
    }

    /// Stops the fixture and waits for task completion.
    pub async fn finish(self) -> Result<(), Box<dyn std::error::Error>> {
        let _stop_result = self.stop.send(());
        self.task.await??;
        Ok(())
    }
}

/// One-request loopback fixture for router compatibility probing.
pub struct RouterHealthFixture {
    address: std::net::SocketAddr,
    task: JoinHandle<std::io::Result<()>>,
}

impl RouterHealthFixture {
    /// Binds a kernel-assigned loopback port and accepts one request.
    pub async fn start(response: RouterHealthFixtureResponse) -> std::io::Result<Self> {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
        let address = listener.local_addr()?;
        let task = tokio::spawn(async move {
            let (mut stream, _peer) = listener.accept().await?;
            let mut request = [0_u8; 1024];
            let _read_bytes = stream.read(&mut request).await?;
            let response_bytes = render_router_health_response(response)?;
            stream.write_all(&response_bytes).await?;
            stream.shutdown().await
        });
        Ok(Self { address, task })
    }

    /// Returns the kernel-assigned loopback address.
    #[must_use]
    pub const fn address(&self) -> std::net::SocketAddr {
        self.address
    }

    /// Waits for the one-request fixture to complete.
    pub async fn finish(self) -> Result<(), Box<dyn std::error::Error>> {
        self.task.await??;
        Ok(())
    }
}

fn render_router_health_response(
    response: RouterHealthFixtureResponse,
) -> std::io::Result<Vec<u8>> {
    if matches!(response, RouterHealthFixtureResponse::Malformed) {
        return Ok(b"foreign-socket\n".to_vec());
    }
    let compatibility = match response {
        RouterHealthFixtureResponse::Compatible => RouterCompatibility::current(false),
        RouterHealthFixtureResponse::AuthenticationRequired => RouterCompatibility::current(true),
        RouterHealthFixtureResponse::Incompatible => RouterCompatibility {
            product: "foreign-router".to_owned(),
            ..RouterCompatibility::current(false)
        },
        RouterHealthFixtureResponse::Malformed => {
            return Err(std::io::Error::other("malformed response handled above"));
        }
    };
    let body = serde_json::to_vec(&compatibility).map_err(std::io::Error::other)?;
    let headers = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        body.len()
    );
    let mut encoded = headers.into_bytes();
    encoded.extend(body);
    Ok(encoded)
}

/// Runs one native app-server websocket fixture until SIGTERM.
pub async fn run_native_app_server_fixture(
    socket_path: &Path,
    running_version: &str,
    process_log: Option<&Path>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let _stale_cleanup = std::fs::remove_file(socket_path);
    let listener = UnixListener::bind(socket_path)?;
    if let Some(process_log) = process_log {
        append_event(process_log, &format!("{}\n", std::process::id()))?;
    }
    let (stream, _peer) = listener.accept().await?;
    let mut websocket = tokio_tungstenite::accept_async(stream).await?;

    let initialize = read_fixture_json(&mut websocket).await?;
    let initialize_id = initialize
        .get("id")
        .and_then(Value::as_u64)
        .ok_or("initialize request id is missing")?;
    websocket
        .send(Message::Text(
            serde_json::json!({
                "id": initialize_id,
                "result": { "userAgent": format!("codex-cli/{running_version}") },
            })
            .to_string()
            .into(),
        ))
        .await?;
    let initialized = read_fixture_json(&mut websocket).await?;
    if initialized.get("method").and_then(Value::as_str) != Some("initialized") {
        return Err("initialized notification is missing".into());
    }
    let remote_status = read_fixture_json(&mut websocket).await?;
    let remote_status_id = remote_status
        .get("id")
        .and_then(Value::as_u64)
        .ok_or("Remote Control request id is missing")?;
    websocket
        .send(Message::Text(
            serde_json::json!({
                "id": remote_status_id,
                "result": {
                    "status": "connected",
                    "serverName": "shared-host-fixture",
                    "environmentId": "fixture-environment",
                },
            })
            .to_string()
            .into(),
        ))
        .await?;
    let _close_frame = websocket.next().await;

    let _signal = terminate.recv().await;
    drop(listener);
    let _socket_cleanup = std::fs::remove_file(socket_path);
    Ok(())
}

/// Runs a compatible fixed-address router fixture until SIGTERM.
pub async fn run_persistent_router_health_fixture(
    address: std::net::SocketAddr,
    process_log: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let listener = TcpListener::bind(address).await?;
    append_event(process_log, &format!("{}\n", std::process::id()))?;
    loop {
        tokio::select! {
            _ = terminate.recv() => return Ok(()),
            accepted = listener.accept() => {
                let (mut stream, _peer) = accepted?;
                let mut request = [0_u8; 1024];
                let _read_bytes = stream.read(&mut request).await?;
                let response = render_router_health_response(
                    RouterHealthFixtureResponse::Compatible,
                )?;
                stream.write_all(&response).await?;
                stream.shutdown().await?;
            }
        }
    }
}

async fn read_fixture_json(
    websocket: &mut tokio_tungstenite::WebSocketStream<tokio::net::UnixStream>,
) -> Result<Value, Box<dyn std::error::Error>> {
    loop {
        let message = websocket
            .next()
            .await
            .ok_or("native websocket closed unexpectedly")??;
        if let Message::Text(text) = message {
            return Ok(serde_json::from_str(&text)?);
        }
    }
}

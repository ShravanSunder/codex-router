//! Bounded static router compatibility probing without external PID discovery.

use std::net::SocketAddr;
use std::time::Duration;

use codex_router_core::router_compatibility::ROUTER_COMPATIBILITY_REVISION;
use codex_router_core::router_compatibility::RouterCompatibility;
use thiserror::Error;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

use crate::ProcessGroupChild;
use crate::ProcessGroupError;

const MAX_ROUTER_HEALTH_RESPONSE_BYTES: u64 = 8 * 1024;
/// Repository-owned SIGTERM-only router shutdown bound.
pub const ROUTER_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

/// Whether the compatible router is external or retained by this host.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RouterOwnership {
    /// Compatible process observed only through its health endpoint.
    External,
    /// Exact child handle retained by this host.
    Owned,
}

/// Terminal owned-router shutdown result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RouterShutdownOutcome {
    /// Exact retained child exited after SIGTERM or was already exited.
    Graceful,
    /// Ten-second bound elapsed; child remains retained and receives no force signal.
    TimedOutStillRunning,
}

/// Retained router child with SIGTERM progression.
pub struct RouterChild {
    process: ProcessGroupChild,
    term_sent: bool,
}

impl RouterChild {
    /// Spawns the existing router command in an isolated process group.
    pub fn spawn(command: &mut tokio::process::Command) -> Result<Self, ProcessGroupError> {
        Ok(Self {
            process: ProcessGroupChild::spawn(command)?,
            term_sent: false,
        })
    }

    /// Returns the exact retained router PID.
    #[must_use]
    pub fn process_id(&self) -> u32 {
        self.process.process_id()
    }

    /// Sends one SIGTERM and waits without any SIGKILL policy.
    pub async fn shutdown(&mut self) -> Result<RouterShutdownOutcome, RouterShutdownError> {
        if self.process.try_wait()?.is_some() {
            return Ok(RouterShutdownOutcome::Graceful);
        }
        if self.term_sent {
            return Ok(RouterShutdownOutcome::TimedOutStillRunning);
        }
        self.term_sent = true;
        self.process.send_terminate()?;
        match tokio::time::timeout(ROUTER_SHUTDOWN_TIMEOUT, self.process.wait()).await {
            Ok(result) => {
                let _status = result?;
                Ok(RouterShutdownOutcome::Graceful)
            }
            Err(_elapsed) => Ok(RouterShutdownOutcome::TimedOutStillRunning),
        }
    }

    /// Waits for the retained router child to exit unexpectedly or intentionally.
    pub async fn wait_for_exit(&mut self) -> Result<std::process::ExitStatus, ProcessGroupError> {
        self.process.wait().await
    }
}

/// Exact owned-router stop failure.
#[derive(Debug, Error)]
pub enum RouterShutdownError {
    /// Process signal or wait failed.
    #[error(transparent)]
    Process(#[from] ProcessGroupError),
}

/// Static router compatibility classification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RouterProbeResult {
    /// Exact same-release router with tokenless model routes.
    Compatible,
    /// Expected router identity requires unsupported local model authentication.
    AuthenticationRequired,
    /// A listener exists but does not satisfy the exact static contract.
    Incompatible,
    /// No listener became reachable within the probe bound.
    Unavailable,
}

/// Unexpected local I/O failure while probing a connected router listener.
#[derive(Debug, Error)]
pub enum RouterProbeError {
    /// Writing the bounded health request failed.
    #[error("failed writing router compatibility request: {0}")]
    Write(#[source] std::io::Error),
    /// Reading the bounded health response failed.
    #[error("failed reading router compatibility response: {0}")]
    Read(#[source] std::io::Error),
}

/// Observes one static `GET /healthz` response within a caller-owned deadline.
pub async fn probe_router(
    endpoint: SocketAddr,
    deadline: Duration,
) -> Result<RouterProbeResult, RouterProbeError> {
    let deadline_at = tokio::time::Instant::now() + deadline;
    let mut stream = match tokio::time::timeout_at(deadline_at, TcpStream::connect(endpoint)).await
    {
        Ok(Ok(stream)) => stream,
        Ok(Err(_)) | Err(_) => return Ok(RouterProbeResult::Unavailable),
    };
    let request = format!("GET /healthz HTTP/1.1\r\nhost: {endpoint}\r\nconnection: close\r\n\r\n");
    match tokio::time::timeout_at(deadline_at, stream.write_all(request.as_bytes())).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => return Err(RouterProbeError::Write(error)),
        Err(_) => return Ok(RouterProbeResult::Incompatible),
    }

    let mut response_bytes = Vec::new();
    let mut limited_stream = stream.take(MAX_ROUTER_HEALTH_RESPONSE_BYTES + 1);
    match tokio::time::timeout_at(deadline_at, limited_stream.read_to_end(&mut response_bytes))
        .await
    {
        Ok(Ok(_read_bytes)) => {}
        Ok(Err(error)) => return Err(RouterProbeError::Read(error)),
        Err(_) => return Ok(RouterProbeResult::Incompatible),
    }
    if u64::try_from(response_bytes.len()).unwrap_or(u64::MAX) > MAX_ROUTER_HEALTH_RESPONSE_BYTES {
        return Ok(RouterProbeResult::Incompatible);
    }

    Ok(classify_health_response(&response_bytes))
}

fn classify_health_response(response_bytes: &[u8]) -> RouterProbeResult {
    let mut headers = [httparse::EMPTY_HEADER; 16];
    let mut response = httparse::Response::new(&mut headers);
    let Ok(httparse::Status::Complete(body_offset)) = response.parse(response_bytes) else {
        return RouterProbeResult::Incompatible;
    };
    if response.code != Some(200) {
        return RouterProbeResult::Incompatible;
    }
    let Some(body) = response_bytes.get(body_offset..) else {
        return RouterProbeResult::Incompatible;
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(body) else {
        return RouterProbeResult::Incompatible;
    };
    if value.as_object().map(serde_json::Map::len) != Some(4) {
        return RouterProbeResult::Incompatible;
    }
    let Ok(compatibility) = serde_json::from_value::<RouterCompatibility>(value) else {
        return RouterProbeResult::Incompatible;
    };
    if compatibility.product != "codex-router"
        || compatibility.compatibility_revision != ROUTER_COMPATIBILITY_REVISION
        || compatibility.binary_version != env!("CARGO_PKG_VERSION")
    {
        return RouterProbeResult::Incompatible;
    }
    if compatibility.local_model_authentication_required {
        RouterProbeResult::AuthenticationRequired
    } else {
        RouterProbeResult::Compatible
    }
}

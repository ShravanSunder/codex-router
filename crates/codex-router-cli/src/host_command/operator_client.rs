//! Bounded client-side exchanges with the shared host operator socket.

use std::path::Path;
use std::time::Duration;

use codex_router_host::HostProgress;
use codex_router_host::MAX_OPERATOR_FRAME_BYTES;
use codex_router_host::OperatorFrame;
use codex_router_host::OperatorProtocolError;
use codex_router_host::OperatorRequest;
use codex_router_host::decode_operator_frame;
use codex_router_host::encode_operator_request;
use thiserror::Error;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;
use tokio::net::UnixStream;

/// Bounded operator-client transport or protocol failure.
#[derive(Debug, Error)]
pub enum OperatorClientError {
    /// No foreground owner has published the private operator socket.
    #[error("no shared Codex host is running; run `codex-router host`")]
    HostNotRunning,
    /// Connecting to the owner-only operator socket failed.
    #[error("failed connecting to shared Codex host: {0}")]
    Connect(#[source] std::io::Error),
    /// Writing or half-closing the one request failed.
    #[error("failed writing shared Codex host request: {0}")]
    Write(#[source] std::io::Error),
    /// Reading the bounded response failed.
    #[error("failed reading shared Codex host response: {0}")]
    Read(#[source] std::io::Error),
    /// The complete request/response exchange exceeded its caller-owned bound.
    #[error("shared Codex host request timed out")]
    Timeout,
    /// The host returned an invalid or incomplete response stream.
    #[error(transparent)]
    Protocol(#[from] OperatorProtocolError),
    /// The connection ended without exactly one terminal frame.
    #[error("shared Codex host response omitted its terminal frame")]
    MissingTerminal,
    /// The connection returned data after its terminal frame.
    #[error("shared Codex host response continued after its terminal frame")]
    FramesAfterTerminal,
}

/// Runs one bounded operator request/response exchange over the private socket.
#[allow(dead_code)]
pub(crate) async fn send_operator_request(
    socket: &Path,
    request: OperatorRequest,
    deadline: Duration,
) -> Result<Vec<OperatorFrame>, OperatorClientError> {
    send_operator_request_with_connect_retry(socket, request, deadline, false, |_| {}).await
}

/// Runs one operator exchange while delivering each decoded frame immediately.
pub(crate) async fn send_operator_request_streaming<F>(
    socket: &Path,
    request: OperatorRequest,
    deadline: Duration,
    on_frame: F,
) -> Result<Vec<OperatorFrame>, OperatorClientError>
where
    F: FnMut(&OperatorFrame),
{
    send_operator_request_with_connect_retry(socket, request, deadline, false, on_frame).await
}

/// Runs the post-reexec exchange, retrying only while the replacement publishes its socket.
pub(super) async fn send_replacement_operator_request(
    socket: &Path,
    request: OperatorRequest,
    deadline: Duration,
) -> Result<Vec<OperatorFrame>, OperatorClientError> {
    send_operator_request_with_connect_retry(socket, request, deadline, true, |_| {}).await
}

async fn send_operator_request_with_connect_retry(
    socket: &Path,
    request: OperatorRequest,
    deadline: Duration,
    retry_unpublished_socket: bool,
    mut on_frame: impl FnMut(&OperatorFrame),
) -> Result<Vec<OperatorFrame>, OperatorClientError> {
    let deadline_at = tokio::time::Instant::now() + deadline;
    let mut stream = loop {
        match tokio::time::timeout_at(deadline_at, UnixStream::connect(socket)).await {
            Ok(Ok(stream)) => break stream,
            Ok(Err(error))
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
                ) =>
            {
                if !retry_unpublished_socket {
                    return Err(OperatorClientError::HostNotRunning);
                }
                if tokio::time::Instant::now() >= deadline_at {
                    return Err(OperatorClientError::Timeout);
                }
                tokio::time::sleep_until(
                    deadline_at.min(tokio::time::Instant::now() + Duration::from_millis(20)),
                )
                .await;
            }
            Ok(Err(error)) => return Err(OperatorClientError::Connect(error)),
            Err(_elapsed) => return Err(OperatorClientError::Timeout),
        }
    };
    let request_bytes = encode_operator_request(&request)?;
    tokio::time::timeout_at(deadline_at, stream.write_all(&request_bytes))
        .await
        .map_err(|_elapsed| OperatorClientError::Timeout)?
        .map_err(OperatorClientError::Write)?;
    tokio::time::timeout_at(deadline_at, stream.shutdown())
        .await
        .map_err(|_elapsed| OperatorClientError::Timeout)?
        .map_err(OperatorClientError::Write)?;

    let mut frames = Vec::new();
    let mut terminal_seen = false;
    let mut response_bytes = 0usize;
    let mut reader = BufReader::new(stream);
    loop {
        let mut response_line = Vec::new();
        let read =
            tokio::time::timeout_at(deadline_at, reader.read_until(b'\n', &mut response_line))
                .await
                .map_err(|_elapsed| OperatorClientError::Timeout)?
                .map_err(OperatorClientError::Read)?;
        if read == 0 {
            break;
        }
        response_bytes = response_bytes.saturating_add(response_line.len());
        if response_bytes > MAX_OPERATOR_FRAME_BYTES {
            return Err(OperatorClientError::Protocol(
                OperatorProtocolError::FrameTooLarge,
            ));
        }
        if response_line == b"\n" {
            continue;
        }
        if terminal_seen {
            return Err(OperatorClientError::FramesAfterTerminal);
        }
        let frame = decode_operator_frame(&response_line)?;
        terminal_seen = matches!(frame, OperatorFrame::Terminal(_));
        on_frame(&frame);
        frames.push(frame);
    }
    if !terminal_seen
        && matches!(
            frames.last(),
            Some(OperatorFrame::Progress(HostProgress::ReplacementStarting))
        )
    {
        return Ok(frames);
    }
    if !terminal_seen {
        return Err(OperatorClientError::MissingTerminal);
    }
    Ok(frames)
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_router_host::{
        AppServerCondition, ExecutableRelation, HostOperation, HostPhase, HostSnapshot,
        HostSnapshotDimensions, HostTerminalResponse, LifecycleOutcome,
        LifecycleOutcomeClassification, RecoveryBudget, RemoteControlCondition, RouterCondition,
        TerminalClassification, encode_operator_frame,
    };
    use tokio::io::AsyncReadExt;
    use tokio::io::AsyncWriteExt;
    use tokio::sync::oneshot;

    #[tokio::test]
    async fn first_connect_reports_missing_host_without_waiting_for_lifecycle_deadline() {
        let socket = std::env::temp_dir().join(format!(
            "codex-router-missing-host-{}-{}.sock",
            std::process::id(),
            tokio::time::Instant::now().elapsed().as_nanos()
        ));
        let started_at = tokio::time::Instant::now();

        let result =
            send_operator_request(&socket, OperatorRequest::Status, Duration::from_secs(40)).await;

        assert!(matches!(result, Err(OperatorClientError::HostNotRunning)));
        assert!(started_at.elapsed() < Duration::from_secs(1));
    }

    #[tokio::test]
    async fn streaming_client_delivers_progress_before_terminal_frame()
    -> Result<(), Box<dyn std::error::Error>> {
        let socket = std::env::temp_dir().join(format!(
            "codex-router-streaming-{}.sock",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&socket);
        let listener = tokio::net::UnixListener::bind(&socket)?;
        let (progress_seen_tx, progress_seen_rx) = oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await?;
            let request = {
                let mut bytes = Vec::new();
                stream.read_to_end(&mut bytes).await?;
                bytes
            };
            assert!(!request.is_empty());
            stream
                .write_all(
                    &encode_operator_frame(&OperatorFrame::Progress(HostProgress::RouterReady))
                        .map_err(std::io::Error::other)?,
                )
                .await?;
            progress_seen_rx.await.map_err(std::io::Error::other)?;
            let snapshot = HostSnapshot::new(HostSnapshotDimensions {
                phase: HostPhase::Steady,
                router: RouterCondition::ExternalReachable,
                app_server: AppServerCondition::Absent,
                remote_control: RemoteControlCondition::Unavailable,
                remote_control_identity: None,
                executable_relation: ExecutableRelation::Unknown,
                recovery_budget: RecoveryBudget::Available,
                last_lifecycle_outcome: Some(LifecycleOutcome {
                    operation: HostOperation::Status,
                    classification: LifecycleOutcomeClassification::Succeeded,
                }),
            });
            stream
                .write_all(
                    &encode_operator_frame(&OperatorFrame::terminal(HostTerminalResponse::new(
                        OperatorRequest::Status,
                        TerminalClassification::Succeeded,
                        snapshot,
                        "ok".to_owned(),
                    )))
                    .map_err(std::io::Error::other)?,
                )
                .await?;
            Ok::<_, std::io::Error>(())
        });
        let mut progress_seen_tx = Some(progress_seen_tx);
        let frames = send_operator_request_streaming(
            &socket,
            OperatorRequest::Status,
            Duration::from_secs(2),
            |frame| {
                if matches!(frame, OperatorFrame::Progress(HostProgress::RouterReady)) {
                    let _ = progress_seen_tx
                        .take()
                        .expect("progress callback once")
                        .send(());
                }
            },
        )
        .await?;
        assert!(matches!(
            frames.first(),
            Some(OperatorFrame::Progress(HostProgress::RouterReady))
        ));
        server.await??;
        let _ = std::fs::remove_file(&socket);
        Ok(())
    }
}

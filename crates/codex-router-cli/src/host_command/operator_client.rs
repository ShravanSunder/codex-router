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
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;

/// Bounded operator-client transport or protocol failure.
#[derive(Debug, Error)]
pub enum OperatorClientError {
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
pub(crate) async fn send_operator_request(
    socket: &Path,
    request: OperatorRequest,
    deadline: Duration,
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

    let response_bytes = read_bounded_to_end(&mut stream, deadline_at)
        .await
        .map_err(|error| match error {
            BoundedReadError::Io(error) => OperatorClientError::Read(error),
            BoundedReadError::Timeout => OperatorClientError::Timeout,
            BoundedReadError::TooLarge => {
                OperatorClientError::Protocol(OperatorProtocolError::FrameTooLarge)
            }
        })?;
    let mut frames = Vec::new();
    let mut terminal_seen = false;
    for response_line in response_bytes.split_inclusive(|byte| *byte == b'\n') {
        if response_line.is_empty() {
            continue;
        }
        if terminal_seen {
            return Err(OperatorClientError::FramesAfterTerminal);
        }
        let frame = decode_operator_frame(response_line)?;
        terminal_seen = matches!(frame, OperatorFrame::Terminal(_));
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

enum BoundedReadError {
    Timeout,
    TooLarge,
    Io(std::io::Error),
}

async fn read_bounded_to_end(
    stream: &mut UnixStream,
    deadline_at: tokio::time::Instant,
) -> Result<Vec<u8>, BoundedReadError> {
    let mut payload = Vec::new();
    let mut limited = stream.take(u64::try_from(MAX_OPERATOR_FRAME_BYTES).unwrap_or(u64::MAX) + 1);
    tokio::time::timeout_at(deadline_at, limited.read_to_end(&mut payload))
        .await
        .map_err(|_elapsed| BoundedReadError::Timeout)?
        .map_err(BoundedReadError::Io)?;
    if payload.len() > MAX_OPERATOR_FRAME_BYTES {
        return Err(BoundedReadError::TooLarge);
    }
    Ok(payload)
}

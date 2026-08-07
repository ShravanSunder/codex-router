//! Bounded same-version operator request framing.

use std::path::Path;
use std::time::Duration;

use serde::Deserialize;
use serde::Serialize;
use thiserror::Error;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;

use crate::HostOperation;
use crate::HostSnapshot;

/// Current private operator protocol version.
pub const OPERATOR_PROTOCOL_VERSION: u16 = 1;
/// Maximum bytes accepted for one newline-delimited request.
const MAX_OPERATOR_FRAME_BYTES: usize = 64 * 1024;

/// One request accepted by the host owner.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperatorRequest {
    /// Observe current state.
    Status,
    /// Wait for this host lifetime's startup terminal state.
    AwaitHostStart,
    /// Restart the retained app-server.
    RestartAppServer,
    /// Run the conditional managed Codex update.
    UpdateCodex,
    /// Restart the retained router child.
    RestartRouter,
}

impl OperatorRequest {
    /// Returns whether the request needs exclusive mutation ownership.
    #[must_use]
    pub const fn is_mutating(self) -> bool {
        matches!(
            self,
            Self::RestartAppServer | Self::UpdateCodex | Self::RestartRouter
        )
    }
}

/// Nonterminal progress emitted only while the same connection remains valid.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HostProgress {
    /// A changed update is beginning ordered child teardown before re-exec.
    ReplacementStarting,
}

/// Terminal classification independent of presentation text.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalClassification {
    /// Requested readiness fully converged.
    Ready,
    /// Native operation converged while Remote Control remains degraded.
    LocalReadyRemoteDegraded,
    /// A required local boundary is unavailable.
    Unavailable,
    /// Requested mutation completed.
    Succeeded,
    /// Requested operation failed.
    Failed,
    /// Another mutation already owns serialization.
    Busy,
}

/// One terminal response carrying every live status dimension.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HostTerminalResponse {
    request: OperatorRequest,
    classification: TerminalClassification,
    snapshot: HostSnapshot,
    message: String,
}

impl HostTerminalResponse {
    /// Creates one terminal response from lifecycle-owned state.
    #[must_use]
    pub const fn new(
        request: OperatorRequest,
        classification: TerminalClassification,
        snapshot: HostSnapshot,
        message: String,
    ) -> Self {
        Self {
            request,
            classification,
            snapshot,
            message,
        }
    }

    /// Returns the terminal classification.
    #[must_use]
    pub const fn classification(&self) -> TerminalClassification {
        self.classification
    }

    /// Returns the live snapshot captured with this terminal result.
    #[must_use]
    pub const fn snapshot(&self) -> &HostSnapshot {
        &self.snapshot
    }
}

/// One progress or terminal frame on the operator connection.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "frame", content = "payload", rename_all = "snake_case")]
pub enum OperatorFrame {
    /// Nonterminal operation progress.
    Progress(HostProgress),
    /// Exactly one terminal result.
    Terminal(HostTerminalResponse),
}

impl OperatorFrame {
    /// Creates the exactly-one terminal result for an operator exchange.
    #[must_use]
    pub const fn terminal(response: HostTerminalResponse) -> Self {
        Self::Terminal(response)
    }

    /// Creates an immediate busy terminal result for a rejected mutation.
    #[must_use]
    pub const fn busy(request: OperatorRequest, snapshot: HostSnapshot, message: String) -> Self {
        Self::Terminal(HostTerminalResponse {
            request,
            classification: TerminalClassification::Busy,
            snapshot,
            message,
        })
    }
}

/// Admission decision made synchronously from current mutation ownership.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MutationAdmission {
    /// Read-only work remains available during lifecycle mutation.
    ReadOnly,
    /// The request acquires lifecycle mutation ownership.
    StartMutation(HostOperation),
    /// Another lifecycle mutation already owns serialization.
    Busy,
}

/// Classifies one request without awaiting or mutating runtime state.
#[must_use]
pub const fn classify_operator_request(
    active_mutation: Option<HostOperation>,
    request: OperatorRequest,
) -> MutationAdmission {
    if !request.is_mutating() {
        return MutationAdmission::ReadOnly;
    }
    if active_mutation.is_some() {
        return MutationAdmission::Busy;
    }
    let operation = match request {
        OperatorRequest::RestartAppServer => HostOperation::RestartAppServer,
        OperatorRequest::UpdateCodex => HostOperation::UpdateCodex,
        OperatorRequest::RestartRouter => HostOperation::RestartRouter,
        OperatorRequest::Status | OperatorRequest::AwaitHostStart => HostOperation::Status,
    };
    MutationAdmission::StartMutation(operation)
}

/// Bounded operator request decoding failure.
#[derive(Debug, Error)]
pub enum OperatorProtocolError {
    /// Request exceeded the 64 KiB protocol limit.
    #[error("operator request exceeds the 64 KiB limit")]
    FrameTooLarge,
    /// One connection attempted more than one request.
    #[error("operator connection must contain exactly one request")]
    MultipleRequests,
    /// Request came from a different installed protocol version.
    #[error("operator protocol version mismatch: expected {expected}, received {actual}")]
    VersionMismatch {
        /// Version understood by this binary.
        expected: u16,
        /// Version provided by the caller.
        actual: u16,
    },
    /// JSON encoding or decoding failed.
    #[error("operator protocol JSON failed: {0}")]
    Json(#[from] serde_json::Error),
}

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

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RequestEnvelope {
    protocol_version: u16,
    request: OperatorRequest,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct FrameEnvelope {
    protocol_version: u16,
    frame: OperatorFrame,
}

/// Encodes one request as a newline-delimited same-version frame.
pub fn encode_operator_request(
    request: &OperatorRequest,
) -> Result<Vec<u8>, OperatorProtocolError> {
    let mut encoded = serde_json::to_vec(&RequestEnvelope {
        protocol_version: OPERATOR_PROTOCOL_VERSION,
        request: *request,
    })?;
    encoded.push(b'\n');
    Ok(encoded)
}

/// Decodes one complete connection payload and rejects additional requests.
pub fn decode_operator_request(payload: &[u8]) -> Result<OperatorRequest, OperatorProtocolError> {
    let frame = single_frame(payload)?;
    let envelope = serde_json::from_slice::<RequestEnvelope>(frame)?;
    if envelope.protocol_version != OPERATOR_PROTOCOL_VERSION {
        return Err(OperatorProtocolError::VersionMismatch {
            expected: OPERATOR_PROTOCOL_VERSION,
            actual: envelope.protocol_version,
        });
    }
    Ok(envelope.request)
}

/// Encodes one progress or terminal response as a versioned line.
pub fn encode_operator_frame(frame: &OperatorFrame) -> Result<Vec<u8>, OperatorProtocolError> {
    let mut encoded = serde_json::to_vec(&FrameEnvelope {
        protocol_version: OPERATOR_PROTOCOL_VERSION,
        frame: frame.clone(),
    })?;
    encoded.push(b'\n');
    Ok(encoded)
}

/// Decodes one complete response frame.
pub fn decode_operator_frame(payload: &[u8]) -> Result<OperatorFrame, OperatorProtocolError> {
    let frame = single_frame(payload)?;
    let envelope = serde_json::from_slice::<FrameEnvelope>(frame)?;
    if envelope.protocol_version != OPERATOR_PROTOCOL_VERSION {
        return Err(OperatorProtocolError::VersionMismatch {
            expected: OPERATOR_PROTOCOL_VERSION,
            actual: envelope.protocol_version,
        });
    }
    Ok(envelope.frame)
}

/// Runs one bounded operator request/response exchange over the private socket.
pub async fn send_operator_request(
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
    if !terminal_seen {
        return Err(OperatorClientError::MissingTerminal);
    }
    Ok(frames)
}

pub(crate) async fn read_request_from_stream(
    stream: &mut UnixStream,
    deadline_at: tokio::time::Instant,
) -> Result<OperatorRequest, OperatorConnectionError> {
    let payload = read_bounded_to_end(stream, deadline_at)
        .await
        .map_err(|error| match error {
            BoundedReadError::Timeout => OperatorConnectionError::Timeout,
            BoundedReadError::TooLarge => {
                OperatorConnectionError::Protocol(OperatorProtocolError::FrameTooLarge)
            }
            BoundedReadError::Io(error) => OperatorConnectionError::Io(error),
        })?;
    decode_operator_request(&payload).map_err(OperatorConnectionError::Protocol)
}

pub(crate) async fn write_frames_to_stream(
    stream: &mut UnixStream,
    frames: &[OperatorFrame],
    deadline_at: tokio::time::Instant,
) -> Result<(), OperatorConnectionError> {
    for frame in frames {
        let encoded = encode_operator_frame(frame)?;
        tokio::time::timeout_at(deadline_at, stream.write_all(&encoded))
            .await
            .map_err(|_elapsed| OperatorConnectionError::Timeout)?
            .map_err(OperatorConnectionError::Io)?;
    }
    tokio::time::timeout_at(deadline_at, stream.shutdown())
        .await
        .map_err(|_elapsed| OperatorConnectionError::Timeout)?
        .map_err(OperatorConnectionError::Io)
}

#[derive(Debug, Error)]
pub(crate) enum OperatorConnectionError {
    #[error("operator connection timed out")]
    Timeout,
    #[error("operator connection I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Protocol(#[from] OperatorProtocolError),
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

fn single_frame(payload: &[u8]) -> Result<&[u8], OperatorProtocolError> {
    if payload.len() > MAX_OPERATOR_FRAME_BYTES {
        return Err(OperatorProtocolError::FrameTooLarge);
    }
    let frame = payload.strip_suffix(b"\n").unwrap_or(payload);
    if frame.contains(&b'\n') {
        return Err(OperatorProtocolError::MultipleRequests);
    }
    Ok(frame)
}

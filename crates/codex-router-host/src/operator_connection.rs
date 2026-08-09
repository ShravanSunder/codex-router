//! Bounded host-side operator connection I/O.

use thiserror::Error;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;

use crate::operator_messages::MAX_OPERATOR_FRAME_BYTES;
use crate::operator_messages::OperatorFrame;
use crate::operator_messages::OperatorProtocolError;
use crate::operator_messages::OperatorRequest;
use crate::operator_messages::decode_operator_request;
use crate::operator_messages::encode_operator_frame;

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

pub(crate) async fn write_frame_to_stream(
    stream: &mut UnixStream,
    frame: &OperatorFrame,
    deadline_at: tokio::time::Instant,
) -> Result<(), OperatorConnectionError> {
    let encoded = encode_operator_frame(frame)?;
    tokio::time::timeout_at(deadline_at, stream.write_all(&encoded))
        .await
        .map_err(|_elapsed| OperatorConnectionError::Timeout)?
        .map_err(OperatorConnectionError::Io)
}

pub(crate) async fn shutdown_operator_stream(
    stream: &mut UnixStream,
    deadline_at: tokio::time::Instant,
) -> Result<(), OperatorConnectionError> {
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

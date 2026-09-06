//! Bidirectional native carrier forwarding with generation-owned cancellation.
use futures_util::{SinkExt, StreamExt};
use std::io;
use tokio::net::UnixStream;
use tokio_tungstenite::{WebSocketStream, tungstenite::Message};
use tokio_util::sync::CancellationToken;

pub const MAX_NATIVE_MESSAGE_BYTES: usize = 64 * 1024 * 1024;

/// Forwards established channels without decoding application payloads or replaying work.
/// Each direction retains at most one message while awaiting its destination.
pub async fn relay_native_channels(
    frontend: WebSocketStream<UnixStream>,
    backend: WebSocketStream<UnixStream>,
    retired: CancellationToken,
) -> io::Result<()> {
    let (mut frontend_write, mut frontend_read) = frontend.split();
    let (mut backend_write, mut backend_read) = backend.split();
    let to_backend = async {
        while let Some(frame) = frontend_read.next().await {
            let message = frame.map_err(|_| io::Error::other("native frontend read failed"))?;
            check_message_size(&message)?;
            let closed = matches!(message, Message::Close(_));
            backend_write
                .send(message)
                .await
                .map_err(|_| io::Error::other("native backend write failed"))?;
            if closed {
                break;
            }
        }
        Ok(())
    };
    let to_frontend = async {
        while let Some(frame) = backend_read.next().await {
            let message = frame.map_err(|_| io::Error::other("native backend read failed"))?;
            check_message_size(&message)?;
            let closed = matches!(message, Message::Close(_));
            frontend_write
                .send(message)
                .await
                .map_err(|_| io::Error::other("native frontend write failed"))?;
            if closed {
                break;
            }
        }
        Ok(())
    };
    // Cancelling the losing futures drops both transport halves, including blocked writes.
    tokio::select! {
        _ = retired.cancelled() => Ok(()),
        result = to_backend => result,
        result = to_frontend => result,
    }
}
fn check_message_size(message: &Message) -> io::Result<()> {
    if message.len() > MAX_NATIVE_MESSAGE_BYTES {
        Err(io::Error::other("native message exceeds carrier limit"))
    } else {
        Ok(())
    }
}

/// Establishes a generation-scoped relay pair; no native initialize is manufactured.
pub async fn connect_native_relay(
    frontend: UnixStream,
    backend_path: &std::path::Path,
    retired: CancellationToken,
) -> io::Result<()> {
    let establish = async {
        let backend = UnixStream::connect(backend_path).await?;
        let config = || {
            tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
                .read_buffer_size(8192)
                .max_message_size(Some(MAX_NATIVE_MESSAGE_BYTES))
                .max_frame_size(Some(MAX_NATIVE_MESSAGE_BYTES))
        };
        let (backend, _) =
            tokio_tungstenite::client_async_with_config("ws://localhost/", backend, Some(config()))
                .await
                .map_err(|_| io::Error::other("native backend upgrade failed"))?;
        let frontend = tokio_tungstenite::accept_async_with_config(frontend, Some(config()))
            .await
            .map_err(|_| io::Error::other("native frontend upgrade failed"))?;
        Ok::<_, io::Error>((frontend, backend))
    };
    let pair = tokio::select! {
        _ = retired.cancelled() => return Ok(()),
        result = tokio::time::timeout(std::time::Duration::from_secs(30),establish) => {
            result.map_err(|_|io::Error::other("native relay upgrade timed out"))??
        }
    };
    relay_native_channels(pair.0, pair.1, retired).await
}

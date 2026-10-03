use super::*;

pub(crate) fn router_websocket_config() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(None)
        .max_frame_size(None)
}
pub(super) fn is_reset_without_closing_handshake(error: &tungstenite::Error) -> bool {
    matches!(
        error,
        tungstenite::Error::Protocol(ProtocolError::ResetWithoutClosingHandshake)
    )
}

pub(crate) fn is_normal_websocket_cleanup_close(error: &tungstenite::Error) -> bool {
    matches!(
        error,
        tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed
    ) || matches!(
        error,
        tungstenite::Error::Io(source)
            if matches!(
                source.kind(),
                std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::BrokenPipe
            )
    ) || is_reset_without_closing_handshake(error)
}

pub(super) async fn close_websocket_stream_best_effort<Stream>(
    websocket: &mut WebSocketStream<Stream>,
) -> Result<(), WebSocketTunnelError>
where
    Stream: AsyncRead + AsyncWrite + Unpin,
{
    match websocket.close(None).await {
        Ok(()) => Ok(()),
        Err(error) if is_normal_websocket_cleanup_close(&error) => Ok(()),
        Err(error) => Err(WebSocketTunnelError::Transport(error)),
    }
}

pub(super) async fn close_websocket_sink_best_effort<Stream>(
    websocket: &mut SplitSink<WebSocketStream<Stream>, Message>,
) -> Result<(), WebSocketTunnelError>
where
    Stream: AsyncRead + AsyncWrite + Unpin,
{
    match websocket.close().await {
        Ok(()) => Ok(()),
        Err(error) if is_normal_websocket_cleanup_close(&error) => Ok(()),
        Err(error) => Err(WebSocketTunnelError::Transport(error)),
    }
}

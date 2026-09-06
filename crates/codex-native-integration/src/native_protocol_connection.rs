//! Native envelope correlation with bounded event retention and no request replay.
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{collections::VecDeque, path::Path, time::Duration};
use tokio::net::UnixStream;
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{Message, protocol::WebSocketConfig},
};

const MESSAGE_LIMIT: usize = 64 * 1024 * 1024;
#[derive(Debug, thiserror::Error)]
pub enum NativeConnectionError {
    #[error("invalid native operation input")]
    InvalidInput,
    #[error("native connection unavailable before dispatch")]
    Unavailable,
    #[error("native request outcome is unknown")]
    OutcomeUnknown,
    #[error("native connection protocol violation")]
    Protocol,
    #[error("native request rejected with code {code}")]
    Rejected { code: i64 },
}
pub struct NativeProtocolConnection {
    socket: WebSocketStream<UnixStream>,
    messages: VecDeque<(Value, usize)>,
    buffered_bytes: usize,
    next_id: u64,
    failed: bool,
    last_rejection: Option<Value>,
}
impl NativeProtocolConnection {
    /// Caller owns callback correlation. Success means socket submission, not native approval
    /// acceptance: another eligible native client may have answered first.
    pub async fn submit_callback_response(
        &mut self,
        id: Value,
        result: Value,
    ) -> Result<(), NativeConnectionError> {
        if self.failed {
            return Err(NativeConnectionError::Unavailable);
        }
        if !(id.is_string() || id.as_i64().is_some()) {
            return Err(NativeConnectionError::InvalidInput);
        }
        let text = json!({"id":id,"result":result}).to_string();
        if text.len() > MESSAGE_LIMIT {
            return Err(NativeConnectionError::InvalidInput);
        }
        self.failed = true;
        tokio::time::timeout(
            Duration::from_secs(30),
            self.socket.send(Message::Text(text.into())),
        )
        .await
        .map_err(|_| NativeConnectionError::OutcomeUnknown)?
        .map_err(|_| NativeConnectionError::OutcomeUnknown)?;
        self.failed = false;
        Ok(())
    }
    pub(crate) fn retire(&mut self) {
        self.failed = true;
    }
    /// Opens the native carrier and negotiates it as this integration's client.
    pub async fn connect(path: &Path) -> Result<Self, NativeConnectionError> {
        let connection = async {
            let stream = UnixStream::connect(path)
                .await
                .map_err(|_| NativeConnectionError::Unavailable)?;
            let config = WebSocketConfig::default()
                .read_buffer_size(8192)
                .max_message_size(Some(MESSAGE_LIMIT))
                .max_frame_size(Some(MESSAGE_LIMIT));
            let (socket, _) = tokio_tungstenite::client_async_with_config(
                "ws://localhost/",
                stream,
                Some(config),
            )
            .await
            .map_err(|_| NativeConnectionError::Unavailable)?;
            Ok::<_, NativeConnectionError>(socket)
        };
        let socket = tokio::time::timeout(Duration::from_secs(30), connection)
            .await
            .map_err(|_| NativeConnectionError::Unavailable)??;
        let mut client = Self::from_websocket(socket);
        client.request("initialize",json!({"clientInfo":{"name":"agent_sessions","title":"Agent Sessions","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}})).await?;
        client
            .socket
            .send(Message::Text(
                json!({"method":"initialized"}).to_string().into(),
            ))
            .await
            .map_err(|_| NativeConnectionError::Unavailable)?;
        Ok(client)
    }
    /// Wraps an established carrier; callers retain responsibility for initialization.
    #[must_use]
    pub fn from_websocket(socket: WebSocketStream<UnixStream>) -> Self {
        Self {
            socket,
            messages: VecDeque::new(),
            buffered_bytes: 0,
            next_id: 0,
            failed: false,
            last_rejection: None,
        }
    }
    /// Consume the last native error object for explicit caller diagnostics.
    /// May contain sensitive backend text; never included in Display or automatic logs.
    pub fn take_last_rejection(&mut self) -> Option<Value> {
        self.last_rejection.take()
    }
    pub async fn request(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<Value, NativeConnectionError> {
        self.last_rejection = None;
        if self.failed {
            return Err(NativeConnectionError::Unavailable);
        }
        let id = format!("agent-sessions-{}", self.next_id);
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or(NativeConnectionError::Protocol)?;
        let text = json!({"id":id,"method":method,"params":params}).to_string();
        if text.len() > MESSAGE_LIMIT {
            return Err(NativeConnectionError::Protocol);
        }
        // Set before awaiting: cancellation by the caller cannot leave a reusable unknown exchange.
        self.failed = true;
        let exchange = async {
            self.socket
                .send(Message::Text(text.into()))
                .await
                .map_err(|_| NativeConnectionError::OutcomeUnknown)?;
            loop {
                let (message, size) = self
                    .read_message()
                    .await
                    .map_err(|_| NativeConnectionError::OutcomeUnknown)?;
                if message.get("method").is_some() {
                    if self.messages.len() >= 1024
                        || size > MESSAGE_LIMIT.saturating_sub(self.buffered_bytes)
                    {
                        return Err(NativeConnectionError::OutcomeUnknown);
                    }
                    self.buffered_bytes += size;
                    self.messages.push_back((message, size));
                    continue;
                }
                if message.get("id") != Some(&json!(id)) {
                    return Err(NativeConnectionError::OutcomeUnknown);
                }
                return match (message.get("result"), message.get("error")) {
                    (Some(result), None) => Ok(result.clone()),
                    (None, Some(error)) => {
                        let code = error
                            .get("code")
                            .and_then(Value::as_i64)
                            .ok_or(NativeConnectionError::Protocol)?;
                        self.last_rejection = Some(error.clone());
                        Err(NativeConnectionError::Rejected { code })
                    }
                    _ => Err(NativeConnectionError::Protocol),
                };
            }
        };
        let result = tokio::time::timeout(Duration::from_secs(30), exchange)
            .await
            .map_err(|_| NativeConnectionError::OutcomeUnknown)?;
        if result.is_ok() || matches!(result, Err(NativeConnectionError::Rejected { .. })) {
            self.failed = false;
        }
        result
    }
    pub fn take_buffered_message(&mut self) -> Option<Value> {
        self.messages.pop_front().map(|(message, size)| {
            self.buffered_bytes -= size;
            message
        })
    }
    pub async fn next_message(&mut self) -> Result<Value, NativeConnectionError> {
        if let Some((message, size)) = self.messages.pop_front() {
            self.buffered_bytes -= size;
            return Ok(message);
        }
        if self.failed {
            return Err(NativeConnectionError::Unavailable);
        }
        match self.read_message().await {
            Ok((message, _)) => Ok(message),
            Err(error) => {
                self.failed = true;
                Err(error)
            }
        }
    }
    async fn read_message(&mut self) -> Result<(Value, usize), NativeConnectionError> {
        loop {
            match self.socket.next().await {
                Some(Ok(Message::Text(text))) => {
                    let size = text.len();
                    let value: Value =
                        serde_json::from_str(&text).map_err(|_| NativeConnectionError::Protocol)?;
                    if !value.is_object() {
                        return Err(NativeConnectionError::Protocol);
                    }
                    return Ok((value, size));
                }
                Some(Ok(Message::Ping(_))) => {
                    self.socket
                        .flush()
                        .await
                        .map_err(|_| NativeConnectionError::Unavailable)?;
                }
                Some(Ok(Message::Pong(_))) => {}
                _ => return Err(NativeConnectionError::Unavailable),
            }
        }
    }
}

//! Bounded native app-server control-protocol exchange.

use std::path::Path;
use std::time::Duration;

use futures_util::SinkExt;
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;
use tokio::net::UnixStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::client_async_with_config;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

use crate::remote_control_observation;
use crate::remote_control_observation::RemoteControlObservation;

pub(crate) const CONTROL_RESPONSE_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_PROTOCOL_MESSAGE_BYTES: usize = 64 * 1024;
const INITIALIZE_REQUEST_ID: u64 = 1;

/// Native app-server observation used by host readiness derivation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppServerObservation {
    running_version: String,
    remote_control: RemoteControlObservation,
}

impl AppServerObservation {
    /// Returns the version reported by native initialize.
    #[must_use]
    pub fn running_version(&self) -> &str {
        &self.running_version
    }

    /// Returns the separately observed Remote Control state.
    #[must_use]
    pub const fn remote_control(&self) -> &RemoteControlObservation {
        &self.remote_control
    }
}

/// Bounded native protocol failure.
#[derive(Debug, Error)]
pub enum CodexProtocolError {
    /// Unix socket connection failed.
    #[error("failed connecting to native app-server socket: {0}")]
    Connect(#[source] std::io::Error),
    /// WebSocket transport failed.
    #[error("native app-server websocket failed: {0}")]
    WebSocket(#[from] tokio_tungstenite::tungstenite::Error),
    /// JSON encoding or decoding failed.
    #[error("native app-server JSON failed: {0}")]
    Json(#[from] serde_json::Error),
    /// A bounded protocol stage did not converge.
    #[error("native app-server {stage} timed out")]
    Timeout {
        /// Low-cardinality protocol stage.
        stage: &'static str,
    },
    /// The server closed before returning the requested result.
    #[error("native app-server closed during {stage}")]
    Closed {
        /// Low-cardinality protocol stage.
        stage: &'static str,
    },
    /// A response violated the pinned protocol contract.
    #[error("native app-server returned an invalid {stage} response")]
    InvalidResponse {
        /// Low-cardinality protocol stage.
        stage: &'static str,
    },
    /// Initialize user agent did not contain a version.
    #[error("native app-server initialize user agent omitted its version")]
    InvalidUserAgent,
}

/// Observes native readiness and one bounded Remote Control convergence window.
pub async fn observe_app_server(
    socket_path: &Path,
    native_readiness_wait: Duration,
    remote_control_wait: Duration,
) -> Result<AppServerObservation, CodexProtocolError> {
    let mut exchange =
        tokio::time::timeout(native_readiness_wait, initialize_app_server(socket_path))
            .await
            .map_err(|_elapsed| CodexProtocolError::Timeout {
                stage: "native readiness",
            })??;

    let remote_control = match tokio::time::timeout(
        remote_control_wait,
        remote_control_observation::observe(&mut exchange, remote_control_wait),
    )
    .await
    {
        Ok(result) => result?,
        Err(_elapsed) => RemoteControlObservation::Connecting {
            server_name: "unknown".to_owned(),
            environment_id: None,
        },
    };
    let _close_result = exchange.close().await;

    Ok(AppServerObservation {
        running_version: exchange.running_version().to_owned(),
        remote_control,
    })
}

/// One initialized native protocol exchange handed to Remote Control observation.
pub(crate) struct InitializedControlExchange {
    websocket: WebSocketStream<UnixStream>,
    running_version: String,
}

impl InitializedControlExchange {
    fn running_version(&self) -> &str {
        &self.running_version
    }

    pub(crate) async fn close(&mut self) -> Result<(), tokio_tungstenite::tungstenite::Error> {
        self.websocket.close(None).await
    }

    pub(crate) async fn send_json(&mut self, value: &Value) -> Result<(), CodexProtocolError> {
        self.websocket
            .send(Message::Text(serde_json::to_string(value)?.into()))
            .await?;
        Ok(())
    }

    pub(crate) async fn read_response(
        &mut self,
        expected_id: u64,
        stage: &'static str,
    ) -> Result<Value, CodexProtocolError> {
        loop {
            let value = self.read_json(CONTROL_RESPONSE_TIMEOUT, stage).await?;
            if value.get("id").and_then(Value::as_u64) != Some(expected_id) {
                continue;
            }
            return value
                .get("result")
                .cloned()
                .ok_or(CodexProtocolError::InvalidResponse { stage });
        }
    }

    pub(crate) async fn read_json(
        &mut self,
        deadline: Duration,
        stage: &'static str,
    ) -> Result<Value, CodexProtocolError> {
        loop {
            let frame = tokio::time::timeout(deadline, self.websocket.next())
                .await
                .map_err(|_elapsed| CodexProtocolError::Timeout { stage })?
                .ok_or(CodexProtocolError::Closed { stage })??;
            if let Message::Text(text) = frame {
                return serde_json::from_str(&text).map_err(CodexProtocolError::Json);
            }
        }
    }
}

async fn initialize_app_server(
    socket_path: &Path,
) -> Result<InitializedControlExchange, CodexProtocolError> {
    let stream = tokio::time::timeout(CONTROL_RESPONSE_TIMEOUT, UnixStream::connect(socket_path))
        .await
        .map_err(|_elapsed| CodexProtocolError::Timeout { stage: "connect" })?
        .map_err(CodexProtocolError::Connect)?;
    let websocket_config = WebSocketConfig::default()
        .read_buffer_size(MAX_PROTOCOL_MESSAGE_BYTES)
        .max_message_size(Some(MAX_PROTOCOL_MESSAGE_BYTES))
        .max_frame_size(Some(MAX_PROTOCOL_MESSAGE_BYTES));
    let (websocket, _response) = tokio::time::timeout(
        CONTROL_RESPONSE_TIMEOUT,
        client_async_with_config("ws://localhost/", stream, Some(websocket_config)),
    )
    .await
    .map_err(|_elapsed| CodexProtocolError::Timeout {
        stage: "websocket upgrade",
    })??;
    let mut exchange = InitializedControlExchange {
        websocket,
        running_version: String::new(),
    };

    exchange
        .send_json(&serde_json::json!({
            "id": INITIALIZE_REQUEST_ID,
            "method": "initialize",
            "params": {
                "clientInfo": {
                    "name": "codex_router_host",
                    "title": "Codex Router Host",
                    "version": env!("CARGO_PKG_VERSION"),
                },
                "capabilities": { "experimentalApi": true },
            },
        }))
        .await?;
    let initialize_result = exchange
        .read_response(INITIALIZE_REQUEST_ID, "initialize")
        .await
        .and_then(|result| {
            serde_json::from_value::<InitializeResult>(result).map_err(CodexProtocolError::Json)
        })?;
    exchange.running_version = parse_user_agent_version(&initialize_result.user_agent)?;

    Ok(exchange)
}

fn parse_user_agent_version(user_agent: &str) -> Result<String, CodexProtocolError> {
    let (_originator, version_and_suffix) = user_agent
        .split_once('/')
        .ok_or(CodexProtocolError::InvalidUserAgent)?;
    version_and_suffix
        .split_whitespace()
        .next()
        .filter(|version| !version.is_empty())
        .map(str::to_owned)
        .ok_or(CodexProtocolError::InvalidUserAgent)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InitializeResult {
    user_agent: String,
}

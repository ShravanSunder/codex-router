//! Bounded native app-server readiness and Remote Control observation.

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

const CONTROL_RESPONSE_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_PROTOCOL_MESSAGE_BYTES: usize = 64 * 1024;
const INITIALIZE_REQUEST_ID: u64 = 1;
const REMOTE_STATUS_REQUEST_ID: u64 = 2;

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

/// Remote Control state without pairing credentials or protocol payloads.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RemoteControlObservation {
    /// Relay connection is ready.
    Connected {
        /// Upstream-provided server display name.
        server_name: String,
        /// Upstream environment identity when assigned.
        environment_id: Option<String>,
    },
    /// Relay connection remains in progress after the readiness bound.
    Connecting {
        /// Upstream-provided server display name.
        server_name: String,
        /// Upstream environment identity when assigned.
        environment_id: Option<String>,
    },
    /// Upstream reports a relay error.
    Errored {
        /// Upstream-provided server display name.
        server_name: String,
        /// Upstream environment identity when assigned.
        environment_id: Option<String>,
    },
    /// Remote Control is disabled.
    Disabled {
        /// Upstream-provided server display name.
        server_name: String,
        /// Upstream environment identity when assigned.
        environment_id: Option<String>,
    },
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
    remote_control_wait: Duration,
) -> Result<AppServerObservation, CodexProtocolError> {
    let stream = tokio::time::timeout(CONTROL_RESPONSE_TIMEOUT, UnixStream::connect(socket_path))
        .await
        .map_err(|_elapsed| CodexProtocolError::Timeout { stage: "connect" })?
        .map_err(CodexProtocolError::Connect)?;
    let websocket_config = WebSocketConfig::default()
        .read_buffer_size(MAX_PROTOCOL_MESSAGE_BYTES)
        .max_message_size(Some(MAX_PROTOCOL_MESSAGE_BYTES))
        .max_frame_size(Some(MAX_PROTOCOL_MESSAGE_BYTES));
    let (mut websocket, _response) = tokio::time::timeout(
        CONTROL_RESPONSE_TIMEOUT,
        client_async_with_config("ws://localhost/", stream, Some(websocket_config)),
    )
    .await
    .map_err(|_elapsed| CodexProtocolError::Timeout {
        stage: "websocket upgrade",
    })??;

    send_json(
        &mut websocket,
        &serde_json::json!({
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
        }),
    )
    .await?;
    let initialize_result = read_response(&mut websocket, INITIALIZE_REQUEST_ID, "initialize")
        .await
        .and_then(|result| {
            serde_json::from_value::<InitializeResult>(result).map_err(CodexProtocolError::Json)
        })?;
    let running_version = parse_user_agent_version(&initialize_result.user_agent)?;

    send_json(
        &mut websocket,
        &serde_json::json!({ "method": "initialized" }),
    )
    .await?;
    send_json(
        &mut websocket,
        &serde_json::json!({
            "id": REMOTE_STATUS_REQUEST_ID,
            "method": "remoteControl/status/read",
        }),
    )
    .await?;
    let status_result = read_response(
        &mut websocket,
        REMOTE_STATUS_REQUEST_ID,
        "Remote Control status",
    )
    .await
    .and_then(|result| {
        serde_json::from_value::<RemoteStatus>(result).map_err(CodexProtocolError::Json)
    })?;
    let remote_control = if status_result.status == RemoteStatusKind::Connecting {
        wait_for_remote_status_change(&mut websocket, status_result, remote_control_wait).await?
    } else {
        status_result.into_observation()
    };
    let _close_result = websocket.close(None).await;

    Ok(AppServerObservation {
        running_version,
        remote_control,
    })
}

async fn send_json(
    websocket: &mut WebSocketStream<UnixStream>,
    value: &Value,
) -> Result<(), CodexProtocolError> {
    websocket
        .send(Message::Text(serde_json::to_string(value)?.into()))
        .await?;
    Ok(())
}

async fn read_response(
    websocket: &mut WebSocketStream<UnixStream>,
    expected_id: u64,
    stage: &'static str,
) -> Result<Value, CodexProtocolError> {
    loop {
        let value = read_json(websocket, CONTROL_RESPONSE_TIMEOUT, stage).await?;
        if value.get("id").and_then(Value::as_u64) != Some(expected_id) {
            continue;
        }
        return value
            .get("result")
            .cloned()
            .ok_or(CodexProtocolError::InvalidResponse { stage });
    }
}

async fn read_json(
    websocket: &mut WebSocketStream<UnixStream>,
    deadline: Duration,
    stage: &'static str,
) -> Result<Value, CodexProtocolError> {
    loop {
        let frame = tokio::time::timeout(deadline, websocket.next())
            .await
            .map_err(|_elapsed| CodexProtocolError::Timeout { stage })?
            .ok_or(CodexProtocolError::Closed { stage })??;
        if let Message::Text(text) = frame {
            return serde_json::from_str(&text).map_err(CodexProtocolError::Json);
        }
    }
}

async fn wait_for_remote_status_change(
    websocket: &mut WebSocketStream<UnixStream>,
    initial: RemoteStatus,
    remote_control_wait: Duration,
) -> Result<RemoteControlObservation, CodexProtocolError> {
    let changed = tokio::time::timeout(remote_control_wait, async {
        loop {
            let value = read_json(
                websocket,
                CONTROL_RESPONSE_TIMEOUT,
                "Remote Control status change",
            )
            .await?;
            if value.get("method").and_then(Value::as_str) != Some("remoteControl/status/changed") {
                continue;
            }
            let params =
                value
                    .get("params")
                    .cloned()
                    .ok_or(CodexProtocolError::InvalidResponse {
                        stage: "Remote Control status change",
                    })?;
            let status = serde_json::from_value::<RemoteStatus>(params)?;
            return Ok::<RemoteStatus, CodexProtocolError>(status);
        }
    })
    .await;

    match changed {
        Ok(result) => result.map(RemoteStatus::into_observation),
        Err(_elapsed) => Ok(initial.into_observation()),
    }
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

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteStatus {
    status: RemoteStatusKind,
    server_name: String,
    environment_id: Option<String>,
}

impl RemoteStatus {
    fn into_observation(self) -> RemoteControlObservation {
        match self.status {
            RemoteStatusKind::Connected => RemoteControlObservation::Connected {
                server_name: self.server_name,
                environment_id: self.environment_id,
            },
            RemoteStatusKind::Connecting => RemoteControlObservation::Connecting {
                server_name: self.server_name,
                environment_id: self.environment_id,
            },
            RemoteStatusKind::Errored => RemoteControlObservation::Errored {
                server_name: self.server_name,
                environment_id: self.environment_id,
            },
            RemoteStatusKind::Disabled => RemoteControlObservation::Disabled {
                server_name: self.server_name,
                environment_id: self.environment_id,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
enum RemoteStatusKind {
    Disabled,
    Connecting,
    Connected,
    Errored,
}

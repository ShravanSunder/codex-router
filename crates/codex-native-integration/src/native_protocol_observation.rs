//! Bounded native app-server control-protocol exchange.

use std::future::Future;
use std::io;
use std::path::Path;
use std::time::Duration;

use futures_util::SinkExt;
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;
use tokio::io::AsyncRead;
use tokio::io::AsyncWrite;
use tokio::net::UnixStream;
use tokio::time::Instant;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::client_async_with_config;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

use crate::app_server_probe_action::AppServerProbeAction;
use crate::native_observation_stage::NativeObservationStage;
use crate::native_observation_validation::AppServerObservationValidationError;
use crate::native_observation_validation::MAX_NATIVE_PROTOCOL_EVIDENCE_BYTES;
use crate::native_observation_validation::validate_recorded_observation;
use crate::remote_control_observation;
use crate::remote_control_observation::RemoteControlObservation;

pub(crate) const CONTROL_RESPONSE_TIMEOUT: Duration = Duration::from_secs(2);
const INITIALIZE_REQUEST_ID: u64 = 1;

/// Native app-server observation used by host readiness derivation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppServerObservation {
    running_version: String,
    remote_control: RemoteControlObservation,
}

impl AppServerObservation {
    /// Reconstructs a captured native observation without performing a new observation.
    pub fn from_recorded_parts(
        running_version: String,
        remote_control: RemoteControlObservation,
    ) -> Result<Self, AppServerObservationValidationError> {
        validate_recorded_observation(&running_version, &remote_control)?;
        Ok(Self {
            running_version,
            remote_control,
        })
    }

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
        stage: NativeObservationStage,
    },
    /// The server closed before returning the requested result.
    #[error("native app-server closed during {stage}")]
    Closed {
        /// Low-cardinality protocol stage.
        stage: NativeObservationStage,
    },
    /// A response violated the pinned protocol contract.
    #[error("native app-server returned an invalid {stage} response")]
    InvalidResponse {
        /// Low-cardinality protocol stage.
        stage: NativeObservationStage,
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
    run_app_server_probe(
        AppServerProbeAction::Observe,
        native_readiness_wait,
        remote_control_wait,
        || UnixStream::connect(socket_path),
    )
    .await
}

/// Runs one designed native app-server action over the caller's async transport connector.
pub async fn run_app_server_probe<TTransport, TConnector, TConnectFuture>(
    action: AppServerProbeAction,
    native_readiness_wait: Duration,
    remote_control_wait: Duration,
    mut connector: TConnector,
) -> Result<AppServerObservation, CodexProtocolError>
where
    TTransport: AsyncRead + AsyncWrite + Unpin,
    TConnector: FnMut() -> TConnectFuture,
    TConnectFuture: Future<Output = io::Result<TTransport>>,
{
    let mut exchange = match action {
        AppServerProbeAction::WaitForReady => {
            initialize_with_ready_budget(native_readiness_wait, &mut connector).await?
        }
        AppServerProbeAction::Observe | AppServerProbeAction::EnableAndObserve => {
            tokio::time::timeout(
                native_readiness_wait,
                connect_and_initialize(&mut connector),
            )
            .await
            .map_err(|_elapsed| CodexProtocolError::Timeout {
                stage: NativeObservationStage::NativeReadiness,
            })??
        }
    };

    let remote_control = match tokio::time::timeout(
        remote_control_wait,
        remote_control_observation::observe(&mut exchange, action, remote_control_wait),
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
pub(crate) struct InitializedControlExchange<TTransport>
where
    TTransport: AsyncRead + AsyncWrite + Unpin,
{
    websocket: WebSocketStream<TTransport>,
    running_version: String,
}

impl<TTransport> InitializedControlExchange<TTransport>
where
    TTransport: AsyncRead + AsyncWrite + Unpin,
{
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
        stage: NativeObservationStage,
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
        stage: NativeObservationStage,
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

async fn connect_and_initialize<TTransport, TConnector, TConnectFuture>(
    connector: &mut TConnector,
) -> Result<InitializedControlExchange<TTransport>, CodexProtocolError>
where
    TTransport: AsyncRead + AsyncWrite + Unpin,
    TConnector: FnMut() -> TConnectFuture,
    TConnectFuture: Future<Output = io::Result<TTransport>>,
{
    let stream = connect_transport(connector, CONTROL_RESPONSE_TIMEOUT).await?;
    initialize_app_server(stream).await
}

async fn initialize_with_ready_budget<TTransport, TConnector, TConnectFuture>(
    native_readiness_wait: Duration,
    connector: &mut TConnector,
) -> Result<InitializedControlExchange<TTransport>, CodexProtocolError>
where
    TTransport: AsyncRead + AsyncWrite + Unpin,
    TConnector: FnMut() -> TConnectFuture,
    TConnectFuture: Future<Output = io::Result<TTransport>>,
{
    let started_at = Instant::now();
    loop {
        let Some(remaining_budget) = native_readiness_wait.checked_sub(started_at.elapsed()) else {
            return Err(CodexProtocolError::Timeout {
                stage: NativeObservationStage::Connect,
            });
        };
        if remaining_budget.is_zero() {
            return Err(CodexProtocolError::Timeout {
                stage: NativeObservationStage::Connect,
            });
        }

        let attempt_budget = CONTROL_RESPONSE_TIMEOUT.min(remaining_budget);
        match connect_transport(connector, attempt_budget).await {
            Ok(stream) => {
                let Some(remaining_budget) =
                    native_readiness_wait.checked_sub(started_at.elapsed())
                else {
                    return Err(CodexProtocolError::Timeout {
                        stage: NativeObservationStage::NativeReadiness,
                    });
                };
                if remaining_budget.is_zero() {
                    return Err(CodexProtocolError::Timeout {
                        stage: NativeObservationStage::NativeReadiness,
                    });
                }
                return tokio::time::timeout(remaining_budget, initialize_app_server(stream))
                    .await
                    .map_err(|_elapsed| CodexProtocolError::Timeout {
                        stage: NativeObservationStage::NativeReadiness,
                    })?;
            }
            Err(CodexProtocolError::Connect(_)) => {}
            Err(CodexProtocolError::Timeout {
                stage: NativeObservationStage::Connect,
            }) if remaining_budget > CONTROL_RESPONSE_TIMEOUT => {}
            Err(error) => return Err(error),
        }

        let Some(remaining_budget) = native_readiness_wait.checked_sub(started_at.elapsed()) else {
            return Err(CodexProtocolError::Timeout {
                stage: NativeObservationStage::Connect,
            });
        };
        if remaining_budget.is_zero() {
            return Err(CodexProtocolError::Timeout {
                stage: NativeObservationStage::Connect,
            });
        }
        tokio::time::sleep(Duration::from_millis(20).min(remaining_budget)).await;
    }
}

async fn connect_transport<TTransport, TConnector, TConnectFuture>(
    connector: &mut TConnector,
    timeout_budget: Duration,
) -> Result<TTransport, CodexProtocolError>
where
    TTransport: AsyncRead + AsyncWrite + Unpin,
    TConnector: FnMut() -> TConnectFuture,
    TConnectFuture: Future<Output = io::Result<TTransport>>,
{
    tokio::time::timeout(timeout_budget, connector())
        .await
        .map_err(|_elapsed| CodexProtocolError::Timeout {
            stage: NativeObservationStage::Connect,
        })?
        .map_err(CodexProtocolError::Connect)
}

async fn initialize_app_server<TTransport>(
    stream: TTransport,
) -> Result<InitializedControlExchange<TTransport>, CodexProtocolError>
where
    TTransport: AsyncRead + AsyncWrite + Unpin,
{
    let websocket_config = WebSocketConfig::default()
        .read_buffer_size(MAX_NATIVE_PROTOCOL_EVIDENCE_BYTES)
        .max_message_size(Some(MAX_NATIVE_PROTOCOL_EVIDENCE_BYTES))
        .max_frame_size(Some(MAX_NATIVE_PROTOCOL_EVIDENCE_BYTES));
    let (websocket, _response) = tokio::time::timeout(
        CONTROL_RESPONSE_TIMEOUT,
        client_async_with_config("ws://localhost/", stream, Some(websocket_config)),
    )
    .await
    .map_err(|_elapsed| CodexProtocolError::Timeout {
        stage: NativeObservationStage::WebSocketUpgrade,
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
        .read_response(INITIALIZE_REQUEST_ID, NativeObservationStage::Initialize)
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

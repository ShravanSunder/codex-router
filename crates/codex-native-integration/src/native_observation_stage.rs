use std::fmt;

use serde::Deserialize;
use serde::Serialize;

/// Closed native app-server observation stage.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum NativeObservationStage {
    /// Opening the native app-server Unix socket.
    Connect,
    /// Upgrading the native socket to WebSocket.
    WebSocketUpgrade,
    /// Waiting for native initialize readiness.
    NativeReadiness,
    /// Sending and receiving the native initialize exchange.
    Initialize,
    /// Reading the Remote Control status response.
    RemoteControlStatus,
    /// Waiting for a Remote Control status change notification.
    RemoteControlStatusChange,
    /// Enabling Remote Control in the native app-server.
    RemoteControlEnable,
}

impl fmt::Display for NativeObservationStage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Connect => "connect",
            Self::WebSocketUpgrade => "websocket upgrade",
            Self::NativeReadiness => "native readiness",
            Self::Initialize => "initialize",
            Self::RemoteControlStatus => "Remote Control status",
            Self::RemoteControlStatusChange => "Remote Control status change",
            Self::RemoteControlEnable => "Remote Control enable",
        })
    }
}

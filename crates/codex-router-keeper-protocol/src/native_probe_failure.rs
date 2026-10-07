use codex_native_integration::{CodexProtocolError, NativeObservationStage};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum NativeProbeStage {
    Connect,
    WebSocketUpgrade,
    NativeReadiness,
    Initialize,
    RemoteControlStatus,
    RemoteControlStatusChange,
    RemoteControlEnable,
}

impl From<NativeObservationStage> for NativeProbeStage {
    fn from(value: NativeObservationStage) -> Self {
        match value {
            NativeObservationStage::Connect => Self::Connect,
            NativeObservationStage::WebSocketUpgrade => Self::WebSocketUpgrade,
            NativeObservationStage::NativeReadiness => Self::NativeReadiness,
            NativeObservationStage::Initialize => Self::Initialize,
            NativeObservationStage::RemoteControlStatus => Self::RemoteControlStatus,
            NativeObservationStage::RemoteControlStatusChange => Self::RemoteControlStatusChange,
            NativeObservationStage::RemoteControlEnable => Self::RemoteControlEnable,
        }
    }
}

impl From<NativeProbeStage> for NativeObservationStage {
    fn from(value: NativeProbeStage) -> Self {
        match value {
            NativeProbeStage::Connect => Self::Connect,
            NativeProbeStage::WebSocketUpgrade => Self::WebSocketUpgrade,
            NativeProbeStage::NativeReadiness => Self::NativeReadiness,
            NativeProbeStage::Initialize => Self::Initialize,
            NativeProbeStage::RemoteControlStatus => Self::RemoteControlStatus,
            NativeProbeStage::RemoteControlStatusChange => Self::RemoteControlStatusChange,
            NativeProbeStage::RemoteControlEnable => Self::RemoteControlEnable,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum NativeProbeFailure {
    Connect,
    WebSocket,
    Json,
    Timeout { stage: NativeProbeStage },
    Closed { stage: NativeProbeStage },
    InvalidResponse { stage: NativeProbeStage },
    InvalidUserAgent,
}

impl From<CodexProtocolError> for NativeProbeFailure {
    fn from(value: CodexProtocolError) -> Self {
        match value {
            CodexProtocolError::Connect(_source) => Self::Connect,
            CodexProtocolError::WebSocket(_source) => Self::WebSocket,
            CodexProtocolError::Json(_source) => Self::Json,
            CodexProtocolError::Timeout { stage } => Self::Timeout {
                stage: stage.into(),
            },
            CodexProtocolError::Closed { stage } => Self::Closed {
                stage: stage.into(),
            },
            CodexProtocolError::InvalidResponse { stage } => Self::InvalidResponse {
                stage: stage.into(),
            },
            CodexProtocolError::InvalidUserAgent => Self::InvalidUserAgent,
        }
    }
}

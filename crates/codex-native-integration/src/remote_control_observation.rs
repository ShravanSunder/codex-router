//! Bounded Remote Control status observation over an initialized native exchange.

use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;

use super::native_protocol_observation::CONTROL_RESPONSE_TIMEOUT;
use super::native_protocol_observation::CodexProtocolError;
use super::native_protocol_observation::InitializedControlExchange;
use crate::app_server_probe_action::AppServerProbeAction;
use crate::native_observation_stage::NativeObservationStage;

const REMOTE_STATUS_REQUEST_ID: u64 = 2;

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

pub(crate) async fn observe<TTransport>(
    exchange: &mut InitializedControlExchange<TTransport>,
    action: AppServerProbeAction,
    remote_control_wait: Duration,
) -> Result<RemoteControlObservation, CodexProtocolError>
where
    TTransport: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    exchange
        .send_json(&serde_json::json!({ "method": "initialized" }))
        .await?;
    let response_stage = match action {
        AppServerProbeAction::Observe | AppServerProbeAction::WaitForReady => {
            exchange
                .send_json(&serde_json::json!({
                    "id": REMOTE_STATUS_REQUEST_ID,
                    "method": "remoteControl/status/read",
                }))
                .await?;
            NativeObservationStage::RemoteControlStatus
        }
        AppServerProbeAction::EnableAndObserve => {
            exchange
                .send_json(&serde_json::json!({
                    "id": REMOTE_STATUS_REQUEST_ID,
                    "method": "remoteControl/enable",
                    "params": { "ephemeral": true },
                }))
                .await?;
            NativeObservationStage::RemoteControlEnable
        }
    };
    let status_result = exchange
        .read_response(REMOTE_STATUS_REQUEST_ID, response_stage)
        .await
        .and_then(|result| {
            serde_json::from_value::<RemoteStatus>(result).map_err(CodexProtocolError::Json)
        })?;
    if status_result.status == RemoteStatusKind::Connecting {
        wait_for_remote_status_change(exchange, status_result, remote_control_wait).await
    } else {
        Ok(status_result.into_observation())
    }
}

async fn wait_for_remote_status_change<TTransport>(
    exchange: &mut InitializedControlExchange<TTransport>,
    initial: RemoteStatus,
    remote_control_wait: Duration,
) -> Result<RemoteControlObservation, CodexProtocolError>
where
    TTransport: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let changed = tokio::time::timeout(remote_control_wait, async {
        loop {
            let value = exchange
                .read_json(
                    CONTROL_RESPONSE_TIMEOUT,
                    NativeObservationStage::RemoteControlStatusChange,
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
                        stage: NativeObservationStage::RemoteControlStatusChange,
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

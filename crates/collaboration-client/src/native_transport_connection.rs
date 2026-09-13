//! Native Codex carrier discovery and connection; no initialization or replay is performed.

use crate::ControlClient;
use collaboration_protocol::{ChannelDescription, EndpointAvailability, EndpointId, EndpointRef};
use std::{io, path::Path, time::Duration};
use tokio::net::UnixStream;
use tokio_tungstenite::{
    WebSocketStream, client_async_with_config, tungstenite::protocol::WebSocketConfig,
};

pub const NATIVE_WEBSOCKET_FRAME_LIMIT: usize = 64 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum NativeTransportError {
    #[error("service unavailable")]
    ServiceUnavailable,
    #[error("endpoint discovery unavailable")]
    EndpointDiscoveryUnavailable,
    #[error("endpoint not found")]
    EndpointNotFound,
    #[error("native endpoint unavailable")]
    EndpointUnavailable,
    #[error("native channel unsupported")]
    ChannelUnsupported,
    #[error("invalid native channel path")]
    InvalidChannelPath,
    #[error("native channel escapes service directory")]
    ChannelEscapesServiceDirectory,
    #[error(transparent)]
    PathResolution(#[from] io::Error),
    #[error("native connect timed out")]
    ConnectTimedOut,
    #[error("native channel upgrade failed")]
    UpgradeFailed,
    #[error("discovery close failed")]
    DiscoveryCloseFailed,
}

pub struct NativeTransportConnection {
    pub endpoint: EndpointRef,
    pub stream: WebSocketStream<UnixStream>,
}

impl NativeTransportConnection {
    /// Opens only the advertised native carrier. It sends no native protocol messages.
    pub async fn connect(
        directory: &Path,
        endpoint_id: EndpointId,
    ) -> Result<Self, NativeTransportError> {
        let mut control = ControlClient::connect(
            directory,
            "agent-collaboration-native",
            env!("CARGO_PKG_VERSION"),
        )
        .await
        .map_err(|_| NativeTransportError::ServiceUnavailable)?;
        let inventory = control
            .list_endpoints()
            .await
            .map_err(|_| NativeTransportError::EndpointDiscoveryUnavailable)?;
        let description = inventory
            .endpoints
            .into_iter()
            .find(|item| {
                item.endpoint.endpoint_id == endpoint_id
                    && item.endpoint.service_id == control.identity().service_id
            })
            .ok_or(NativeTransportError::EndpointNotFound)?;
        if !matches!(
            description.availability,
            EndpointAvailability::Available { .. }
        ) {
            return Err(NativeTransportError::EndpointUnavailable);
        }
        let channel_path = description
            .channels
            .into_iter()
            .find_map(|channel| match channel {
                ChannelDescription::NativeCodex { path, .. } => Some(String::from(path)),
                _ => None,
            })
            .ok_or(NativeTransportError::ChannelUnsupported)?;
        let relative_path = Path::new(&channel_path);
        if relative_path.is_absolute()
            || relative_path
                .components()
                .any(|part| !matches!(part, std::path::Component::Normal(_)))
        {
            return Err(NativeTransportError::InvalidChannelPath);
        }
        let service_root = std::fs::canonicalize(directory)?;
        let socket_path = std::fs::canonicalize(service_root.join(relative_path))?;
        if !socket_path.starts_with(&service_root) {
            return Err(NativeTransportError::ChannelEscapesServiceDirectory);
        }

        let establish = async {
            let stream = UnixStream::connect(socket_path).await?;
            client_async_with_config(
                "ws://localhost/",
                stream,
                Some(
                    WebSocketConfig::default()
                        .max_message_size(Some(NATIVE_WEBSOCKET_FRAME_LIMIT))
                        .max_frame_size(Some(NATIVE_WEBSOCKET_FRAME_LIMIT)),
                ),
            )
            .await
            .map_err(|_| NativeTransportError::UpgradeFailed)
        };
        let (socket, _) = tokio::time::timeout(Duration::from_secs(30), establish)
            .await
            .map_err(|_| NativeTransportError::ConnectTimedOut)??;
        control
            .close()
            .await
            .map_err(|_| NativeTransportError::DiscoveryCloseFailed)?;
        Ok(Self {
            endpoint: description.endpoint,
            stream: socket,
        })
    }
}

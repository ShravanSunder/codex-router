//! Owner-local ACP carrier selection; the caller owns negotiation, requests and callbacks.
use crate::{ClientError, ControlClient};
use communication_protocol::{
    ChannelDescription, EndpointAvailability, EndpointId, EndpointRef, SchemaDigest,
};
use std::{path::Path, time::Duration};
use tokio::net::UnixStream;

pub struct AcpTransportConnection {
    pub endpoint: EndpointRef,
    pub schema_digest: SchemaDigest,
    pub stream: UnixStream,
}
impl AcpTransportConnection {
    /// Opens only the advertised transport. It sends no ACP initialization or agent input.
    pub async fn connect(directory: &Path, endpoint_id: EndpointId) -> Result<Self, ClientError> {
        let mut control =
            ControlClient::connect(directory, "acp-transport", env!("CARGO_PKG_VERSION")).await?;
        let endpoint = control
            .list_endpoints()
            .await?
            .endpoints
            .into_iter()
            .find(|endpoint| {
                endpoint.endpoint.endpoint_id == endpoint_id
                    && endpoint.endpoint.service_id == control.identity().service_id
            })
            .ok_or(ClientError::Protocol("ACP endpoint missing"))?;
        if !matches!(
            endpoint.availability,
            EndpointAvailability::Available { .. }
        ) {
            return Err(ClientError::Protocol("ACP endpoint unavailable"));
        }
        let (path, schema_digest) = endpoint
            .channels
            .into_iter()
            .find_map(|channel| match channel {
                ChannelDescription::Acp {
                    path,
                    schema_digest,
                    ..
                } => Some((String::from(path), schema_digest)),
                _ => None,
            })
            .ok_or(ClientError::UnsupportedCapability("ACP channel"))?;
        let relative = Path::new(&path);
        if relative.is_absolute()
            || relative
                .components()
                .any(|component| !matches!(component, std::path::Component::Normal(_)))
        {
            return Err(ClientError::Protocol("invalid ACP selector"));
        }
        let root = std::fs::canonicalize(directory)?;
        let socket = std::fs::canonicalize(root.join(relative))?;
        if socket.parent() != Some(root.as_path()) {
            return Err(ClientError::Protocol("ACP selector escaped service"));
        }
        let stream = tokio::time::timeout(Duration::from_secs(30), UnixStream::connect(socket))
            .await
            .map_err(|_| ClientError::Timeout)??;
        control.close().await?;
        Ok(Self {
            endpoint: endpoint.endpoint,
            schema_digest,
            stream,
        })
    }
}

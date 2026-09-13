//! Explicit native attachment and buffered observation, independent of terminal UI.
use crate::{ClientError, ControlClient};
use codex_native_integration::NativeProtocolConnection;
use collaboration_protocol::{
    ChannelDescription, CodexGeneration, EndpointAvailability, EndpointId, EndpointRef, SessionId,
    SessionRef,
};
use serde_json::Value;
use std::path::Path;

pub struct NativeObservation {
    connection: NativeProtocolConnection,
    target: SessionRef,
    generation: CodexGeneration,
}
impl NativeObservation {
    /// Binds the endpoint to the discovered service before attaching the native session ID.
    pub async fn attach_by_ids(
        directory: &Path,
        endpoint_id: EndpointId,
        session_id: SessionId,
    ) -> Result<Self, ClientError> {
        let control = Self::connect_control(directory).await?;
        let target = SessionRef {
            endpoint: EndpointRef {
                service_id: control.identity().service_id.clone(),
                endpoint_id,
            },
            session_id,
        };
        Self::attach_with_control(directory, control, target).await
    }

    /// May load the target through native resume; readiness is returned only after attachment.
    pub async fn attach(directory: &Path, target: SessionRef) -> Result<Self, ClientError> {
        let control = Self::connect_control(directory).await?;
        Self::attach_with_control(directory, control, target).await
    }

    async fn connect_control(directory: &Path) -> Result<ControlClient, ClientError> {
        ControlClient::connect(
            directory,
            "agent-collaboration-observer",
            env!("CARGO_PKG_VERSION"),
        )
        .await
    }

    async fn attach_with_control(
        directory: &Path,
        mut control: ControlClient,
        target: SessionRef,
    ) -> Result<Self, ClientError> {
        let inventory = control.list_endpoints().await?;
        let endpoint = inventory
            .endpoints
            .into_iter()
            .find(|e| e.endpoint == target.endpoint)
            .ok_or(ClientError::Protocol("observation endpoint missing"))?;
        if !matches!(
            endpoint.availability,
            EndpointAvailability::Available { .. }
        ) {
            return Err(ClientError::Protocol("observation endpoint unavailable"));
        }
        let (path, generation) = endpoint
            .channels
            .into_iter()
            .find_map(|c| match c {
                ChannelDescription::NativeCodex {
                    path,
                    generation: Some(g),
                    ..
                } => Some((String::from(path), g)),
                _ => None,
            })
            .ok_or(ClientError::Protocol("native observation unsupported"))?;
        let relative = Path::new(&path);
        if relative.is_absolute()
            || relative
                .components()
                .any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            return Err(ClientError::Protocol("invalid native observation path"));
        }
        let root = std::fs::canonicalize(directory)?;
        let socket = std::fs::canonicalize(root.join(relative))?;
        if socket.parent() != Some(root.as_path()) {
            return Err(ClientError::Protocol("observation path escaped service"));
        }
        let mut connection = NativeProtocolConnection::connect(&socket)
            .await
            .map_err(|_| ClientError::Protocol("native observation connection failed"))?;
        connection
            .resume_thread(&String::from(target.session_id.clone()))
            .await
            .map_err(|_| ClientError::Protocol("native attachment failed or uncertain"))?;
        let current = control.list_endpoints().await?;
        let unchanged = current.endpoints.iter().find(|e| e.endpoint == target.endpoint).is_some_and(|e| e.channels.iter().any(|c| matches!(c, ChannelDescription::NativeCodex { generation:Some(g),.. } if g == &generation)));
        control.close().await?;
        if !unchanged {
            return Err(ClientError::Protocol("backend changed during attachment"));
        }
        Ok(Self {
            connection,
            target,
            generation,
        })
    }
    #[must_use]
    pub fn target(&self) -> &SessionRef {
        &self.target
    }
    #[must_use]
    pub fn generation(&self) -> &CodexGeneration {
        &self.generation
    }
    /// Returns native messages already buffered during attachment before reading new frames.
    /// Dropping observation never interrupts the native turn or answers an approval callback.
    pub async fn next_message(&mut self) -> Result<Value, ClientError> {
        self.connection
            .next_message()
            .await
            .map_err(|_| ClientError::Protocol("native observation closed"))
    }
}

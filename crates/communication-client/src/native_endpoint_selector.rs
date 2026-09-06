//! Public endpoint resolution immediately before native TUI launch.
use crate::ControlClient;
use communication_protocol::{ChannelDescription, EndpointAvailability};
use std::{
    io,
    path::{Path, PathBuf},
};

pub fn resolve_public_native(directory: &Path) -> io::Result<PathBuf> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let mut client = ControlClient::connect(
            directory,
            "agent-sessions-launch",
            env!("CARGO_PKG_VERSION"),
        )
        .await
        .map_err(|_| io::Error::other("communication service unavailable"))?;
        let inventory = client
            .list_endpoints()
            .await
            .map_err(|_| io::Error::other("endpoint discovery failed"))?;
        let endpoint = inventory
            .endpoints
            .into_iter()
            .find(|e| {
                String::from(e.endpoint.endpoint_id.clone()) == "codex-local"
                    && e.endpoint.service_id == client.identity().service_id
            })
            .ok_or_else(|| io::Error::other("Codex endpoint not found"))?;
        if !matches!(
            endpoint.availability,
            EndpointAvailability::Available { .. }
        ) {
            return Err(io::Error::other("Codex endpoint unavailable"));
        }
        let path = endpoint
            .channels
            .into_iter()
            .find_map(|c| match c {
                ChannelDescription::NativeCodex {
                    path,
                    generation: Some(_),
                    ..
                } => Some(String::from(path)),
                _ => None,
            })
            .ok_or_else(|| io::Error::other("native channel unavailable"))?;
        let relative = Path::new(&path);
        if relative.is_absolute()
            || relative
                .components()
                .any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            return Err(io::Error::other("invalid native selector"));
        }
        let root = std::fs::canonicalize(directory)?;
        let socket = std::fs::canonicalize(root.join(relative))?;
        if socket.parent() != Some(root.as_path()) {
            return Err(io::Error::other(
                "native selector escaped service directory",
            ));
        }
        client
            .close()
            .await
            .map_err(|_| io::Error::other("discovery connection close failed"))?;
        Ok(socket)
    })
}

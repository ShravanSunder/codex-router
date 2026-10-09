//! Public endpoint resolution immediately before native TUI launch.
use crate::{CollaborationClient, NativeTransportError};
use collaboration_protocol::{ChannelDescription, EndpointAvailability, EndpointRef};
use std::{
    io,
    path::{Path, PathBuf},
};

pub fn resolve_public_native(directory: &Path) -> io::Result<PathBuf> {
    resolve_native_endpoint(directory, None)
}

/// Resolves a source-affine native route, rejecting another service or endpoint.
pub fn resolve_public_native_for_endpoint(
    directory: &Path,
    expected_endpoint: &EndpointRef,
) -> io::Result<PathBuf> {
    resolve_native_endpoint(directory, Some(expected_endpoint))
}

fn resolve_native_endpoint(
    directory: &Path,
    expected_endpoint: Option<&EndpointRef>,
) -> io::Result<PathBuf> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let client = CollaborationClient::connect(
            directory,
            "agent-collaboration-launch",
            env!("CARGO_PKG_VERSION"),
        )
        .await
        .map_err(|error| {
            error.permission_diagnostic().map_or_else(
                || io::Error::other("communication service unavailable"),
                |diagnostic| io::Error::other(NativeTransportError::PermissionDenied(diagnostic)),
            )
        })?;
        if let Some(expected) = expected_endpoint
            && expected.service_id != client.identity().service_id
        {
            return Err(io::Error::other(
                "selected session source does not match Router service",
            ));
        }
        let inventory = client.list_endpoints().await.map_err(|error| {
            match NativeTransportError::from_discovery(error) {
                overloaded @ NativeTransportError::Overloaded { .. } => {
                    io::Error::other(overloaded)
                }
                _ => io::Error::other("endpoint discovery failed"),
            }
        })?;
        let endpoint = inventory
            .endpoints
            .into_iter()
            .find(|e| {
                e.endpoint.service_id == client.identity().service_id
                    && expected_endpoint.map_or_else(
                        || String::from(e.endpoint.endpoint_id.clone()) == "codex-local",
                        |expected| e.endpoint == *expected,
                    )
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
        Ok(socket)
    })
}

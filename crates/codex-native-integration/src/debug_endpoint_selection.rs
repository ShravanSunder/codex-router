//! Debug profile selection never implies permission to use the normal backend socket.
use crate::CodexPaths;
use std::path::{Component, Path, PathBuf};

pub struct AppServerEndpointSelection<'a> {
    pub paths: &'a CodexPaths,
    pub debug_defaults: bool,
    pub requested_socket: Option<&'a str>,
}

pub fn select_app_server_endpoint(
    selection: AppServerEndpointSelection<'_>,
) -> Result<PathBuf, &'static str> {
    let normal_socket = selection.paths.app_server_socket();
    if !selection.debug_defaults {
        return Ok(normal_socket);
    }
    let socket = selection
        .requested_socket
        .filter(|value| !value.is_empty())
        .ok_or("debug hosted launch requires CODEX_ROUTER_DEBUG_APP_SERVER_SOCKET")?;
    let socket = PathBuf::from(socket);
    validate_debug_endpoint(&socket, &normal_socket)?;
    Ok(socket)
}

pub fn validate_debug_endpoint(socket: &Path, normal_socket: &Path) -> Result<(), &'static str> {
    if !socket.is_absolute() {
        return Err("debug socket must be absolute");
    }
    std::os::unix::net::SocketAddr::from_pathname(socket)
        .map_err(|_| "debug socket path exceeds native Unix socket limits")?;
    let socket = resolved_endpoint_path(socket)?;
    let normal_socket = resolved_endpoint_path(normal_socket)?;
    let parent = socket
        .parent()
        .ok_or("debug socket needs a dedicated directory")?;
    let temporary_root = resolved_endpoint_path(&std::env::temp_dir())?;
    let slash_tmp = resolved_endpoint_path(Path::new("/tmp"))?;
    if parent == Path::new("/") || parent == temporary_root || parent == slash_tmp {
        return Err("debug socket needs a dedicated directory");
    }
    let normal_parent = normal_socket
        .parent()
        .ok_or("normal endpoint has no directory")?;
    validate_debug_directory(parent, normal_parent)?;
    Ok(())
}

/// Prevent a debug runtime directory from aliasing, containing, or entering protected state.
pub fn validate_debug_directory(directory: &Path, protected: &Path) -> Result<(), &'static str> {
    if !directory.is_absolute() {
        return Err("debug runtime directory must be absolute");
    }
    let directory = resolved_endpoint_path(directory)?;
    let protected = resolved_endpoint_path(protected)?;
    if directory.starts_with(&protected)
        || protected.starts_with(&directory)
        || directory == resolved_endpoint_path(&std::env::temp_dir())?
        || directory == resolved_endpoint_path(Path::new("/tmp"))?
    {
        return Err("debug runtime directory must be dedicated and separate from normal state");
    }
    Ok(())
}

fn resolved_endpoint_path(path: &Path) -> Result<PathBuf, &'static str> {
    // Resolve existing ancestors before comparing not-yet-created endpoint paths.
    let mut ancestor = path.to_owned();
    let mut remaining = Vec::new();
    let base = loop {
        match std::fs::canonicalize(&ancestor) {
            Ok(base) => break base,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if std::fs::symlink_metadata(&ancestor).is_ok() {
                    return Err("debug socket has an unresolved filesystem alias");
                }
                let part = ancestor
                    .file_name()
                    .ok_or("debug socket path cannot be resolved")?;
                remaining.push(part.to_owned());
                if !ancestor.pop() {
                    return Err("debug socket path cannot be resolved");
                }
            }
            Err(_) => return Err("debug socket path cannot be resolved"),
        }
    };
    let mut resolved = base;
    for part in remaining.into_iter().rev() {
        resolved.push(part);
    }
    let mut normalized = PathBuf::new();
    for component in resolved.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    Ok(normalized)
}

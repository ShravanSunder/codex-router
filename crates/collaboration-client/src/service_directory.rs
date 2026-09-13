//! Explicit service-directory policy without process-global environment reads.

use std::{ffi::OsString, path::PathBuf};

pub const COLLABORATION_SERVICE_DIRECTORY_NAME: &str = "agent-communication";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServiceDirectoryOptions {
    pub explicit_directory: Option<PathBuf>,
    pub debug_defaults: bool,
    pub use_home_default: bool,
    pub debug_router_root: Option<OsString>,
    pub home_directory: Option<OsString>,
}

#[derive(Debug, thiserror::Error, Eq, PartialEq)]
pub enum ServiceDirectoryError {
    #[error("service directory must be absolute")]
    RelativeExplicitDirectory,
    #[error("debug Router root must be absolute")]
    RelativeDebugRouterRoot,
    #[error("absolute HOME required")]
    AbsoluteHomeRequired,
}

pub fn resolve_service_directory(
    options: ServiceDirectoryOptions,
) -> Result<PathBuf, ServiceDirectoryError> {
    if let Some(directory) = options.explicit_directory {
        return if directory.is_absolute() {
            Ok(directory)
        } else {
            Err(ServiceDirectoryError::RelativeExplicitDirectory)
        };
    }

    let use_debug_directory = options.debug_defaults && !options.use_home_default;
    if use_debug_directory && let Some(root) = options.debug_router_root {
        let root = PathBuf::from(root);
        if !root.is_absolute() {
            return Err(ServiceDirectoryError::RelativeDebugRouterRoot);
        }
        return Ok(root.join(COLLABORATION_SERVICE_DIRECTORY_NAME));
    }

    let home = options
        .home_directory
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or(ServiceDirectoryError::AbsoluteHomeRequired)?;
    Ok(home
        .join(if use_debug_directory {
            ".codex-router-debug"
        } else {
            ".codex-router"
        })
        .join(COLLABORATION_SERVICE_DIRECTORY_NAME))
}

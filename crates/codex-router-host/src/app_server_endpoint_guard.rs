//! Fail-closed foreign app-server endpoint exclusion.

use std::path::Path;
use std::time::Duration;

use thiserror::Error;

/// Fails closed when another process already answers on the native endpoint.
pub async fn require_unowned_app_server_endpoint(
    socket_path: &Path,
    deadline: Duration,
) -> Result<(), AppServerEndpointError> {
    match tokio::time::timeout(deadline, tokio::net::UnixStream::connect(socket_path)).await {
        Ok(Ok(_stream)) => Err(AppServerEndpointError::OwnershipConflict),
        Ok(Err(error))
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) =>
        {
            Ok(())
        }
        Ok(Err(error)) => Err(AppServerEndpointError::Inspect(error)),
        Err(_elapsed) => Err(AppServerEndpointError::InspectionTimeout),
    }
}

/// Pre-spawn native endpoint ownership failure.
#[derive(Debug, Error)]
pub enum AppServerEndpointError {
    /// Another process already answers on the conventional endpoint.
    #[error("native app-server endpoint is already owned by another process")]
    OwnershipConflict,
    /// Endpoint ownership inspection failed.
    #[error("failed inspecting native app-server endpoint: {0}")]
    Inspect(#[source] std::io::Error),
    /// Endpoint ownership did not converge within its bound.
    #[error("native app-server endpoint inspection timed out")]
    InspectionTimeout,
}

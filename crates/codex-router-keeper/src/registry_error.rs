//! Errors at retained singleton/listener authority boundaries.
use codex_router_descriptor_boundary::BoundaryError;
#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("shared Codex host is already running")]
    AlreadyRunning,
    #[error("listener or lock path must be absolute in an existing private directory")]
    PrivatePathRequired,
    #[error("TCP listener must be loopback")]
    LoopbackRequired,
    #[error("listener kind is already associated with a different address")]
    AssociationConflict,
    #[error("listener kind does not match its endpoint address")]
    KindAddressMismatch,
    #[error("listener kind has not been bound")]
    ListenerAbsent,
    #[error("operator pathname is occupied by another endpoint")]
    OperatorOccupied,
    #[error("operator socket must be host.sock beside the retained singleton lock")]
    OperatorPathMismatch,
    #[error("singleton acquisition failed: {0}")]
    Singleton(#[source] std::io::Error),
    #[error("private listener filesystem operation failed: {0}")]
    Filesystem(#[source] std::io::Error),
    #[error("listener descriptor operation failed: {0}")]
    Descriptor(#[from] BoundaryError),
}

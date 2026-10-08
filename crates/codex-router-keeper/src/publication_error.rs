//! Failures before atomic publication; a committed rename has no fallible follow-up.
#[derive(Debug, thiserror::Error)]
pub enum PublicationError {
    #[error("effective endpoint is occupied")]
    EndpointOccupied,
    #[error("endpoint symlink ownership changed")]
    OwnershipLost,
    #[error("generation alias must resolve through a symlink to a socket")]
    AliasNotSocket,
    #[error("invalid publication path: {0}")]
    Path(#[from] codex_router_keeper_protocol::EndpointPathError),
    #[error("private parent validation failed: {0}")]
    PrivateParent(#[from] crate::RegistryError),
    #[error("retaining singleton descriptor failed: {0}")]
    Descriptor(#[from] codex_router_descriptor_boundary::BoundaryError),
    #[error("publication filesystem operation failed: {0}")]
    Filesystem(#[from] std::io::Error),
}

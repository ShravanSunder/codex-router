#[derive(Debug, thiserror::Error)]
pub enum ImageError {
    #[error("image filesystem operation failed: {0}")]
    Filesystem(#[from] std::io::Error),
    #[error("image work failed to join: {0}")]
    Join(#[from] tokio::task::JoinError),
    #[error("owner-private nonsymlink directory required")]
    PrivateDirectory,
    #[error("image is not a stable regular executable")]
    InvalidExecutable,
    #[error("retained image identity or content changed")]
    ImageUnavailable,
    #[error("unrecognized or replaced image node preserved")]
    ForeignNode,
    #[error("recorded image invalid: {0}")]
    Record(#[from] codex_router_keeper_protocol::SlotImageError),
    #[error("image subprocess failed: {0}")]
    Process(#[from] crate::GroupStopError),
    #[error("image warmup exceeded the prepare deadline")]
    WarmupTimedOut,
    #[error("image warmup stdout exceeded the frame bound")]
    WarmupTooLarge,
    #[error("image warmup exited unsuccessfully")]
    WarmupExit,
    #[error("image warmup returned invalid build-info: {0}")]
    BuildInfoJson(#[from] serde_json::Error),
    #[error("image warmup build-info differs from all expected values")]
    BuildInfoMismatch,
    #[error("image warmup group cleanup did not become empty")]
    CleanupIncomplete,
    #[error("candidate image state is invalid for this release")]
    CandidateState,
    #[error("warmup budget must be positive and no larger than prepare deadline")]
    InvalidBudget,
}

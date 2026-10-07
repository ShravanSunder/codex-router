#[derive(Debug, thiserror::Error)]
pub enum GroupStopError {
    #[error("gated child spawn failed: {0}")]
    Spawn(#[from] codex_router_descriptor_boundary::BoundaryError),
    #[error("spawned child identity unavailable or invalid")]
    InvalidIdentity,
    #[error("PID is not an exclusively owned child")]
    NotOwnedChild,
    #[error("owned child is not the recorded process-group leader")]
    GroupMismatch,
    #[error("owned child nonblocking wait failed: {0}")]
    Wait(#[source] rustix::io::Errno),
    #[error("owned child pipe conversion failed: {0}")]
    Stdio(#[source] std::io::Error),
    #[error("retained child cleanup failed: {0}")]
    Reap(#[from] std::io::Error),
    #[error("post-spawn child cleanup did not converge")]
    CleanupTimedOut,
    #[error("owned group signal failed: {0}")]
    Signal(#[source] rustix::io::Errno),
    #[error("owned group existence probe failed: {0}")]
    Probe(#[source] rustix::io::Errno),
    #[error("stop timing must be positive and within its production profile")]
    InvalidTiming,
    #[error("stop profile/timers cannot change after stop begins")]
    StopAlreadyRequested,
    #[error("stop must be requested before waiting")]
    StopNotRequested,
    #[error("monotonic observation precedes signal time")]
    TimeWentBackwards,
    #[error("stop waiter cancelled; retained owner still holds progress")]
    Cancelled,
}
